//! Minimal Chromium DevTools Protocol session for deterministic captures.

use std::fs;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::manifest::Viewport;

/// One cell's browser evidence before it is written to the manifest root.
#[derive(Debug)]
pub(crate) struct CapturedCell {
    /// Full-page PNG bytes.
    pub(crate) full_page_png: Vec<u8>,
    /// Viewport-only PNG bytes.
    pub(crate) viewport_png: Vec<u8>,
    /// DOM snapshot returned by Chromium.
    pub(crate) dom_snapshot: Value,
    /// Accessibility tree returned by Chromium.
    pub(crate) accessibility: Value,
    /// Readiness and layout facts captured after navigation.
    pub(crate) readiness: Value,
    /// Deterministic DOM/layout observations collected after readiness.
    pub(crate) deterministic: Value,
    /// Console, exception, and failed-request events observed during capture.
    pub(crate) console: Vec<Value>,
}

/// A persistent headless Chromium process connected through CDP's pipe mode.
pub(crate) struct BrowserSession {
    child: Child,
    input: ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
    profile: TempDir,
    next_id: u64,
    session_id: String,
    events: Vec<Value>,
    browser_product: String,
}

impl BrowserSession {
    /// Starts one browser process and attaches one page target.
    pub(crate) fn start(browser: &Path, browser_args: &[String]) -> Result<Self> {
        let profile = tempfile::tempdir().context("create Chromium profile")?;
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "exec 3<&0; exec 4>&1; exec \"$@\"",
                "visual-rubric-browser",
            ])
            .arg(browser)
            .args([
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--hide-scrollbars",
                "--remote-debugging-pipe",
                "--no-first-run",
                "--no-default-browser-check",
            ])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .args(browser_args)
            .arg("about:blank")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Chromium diagnostics are captured through CDP events below. Do not
            // leave an undrained stderr pipe that can block a long capture run.
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .with_context(|| format!("start Chromium browser {}", browser.display()))?;
        let input = child.stdin.take().context("capture Chromium CDP input")?;
        let output = child.stdout.take().context("capture Chromium CDP output")?;
        let mut session = Self {
            child,
            input,
            output: std::io::BufReader::new(output),
            profile,
            next_id: 0,
            session_id: String::new(),
            events: Vec::new(),
            browser_product: String::new(),
        };

        let version = session.command("Browser.getVersion", None, None)?;
        session.browser_product = version
            .get("product")
            .and_then(Value::as_str)
            .unwrap_or("Chromium")
            .to_owned();
        let target = session.command(
            "Target.createTarget",
            Some(json!({"url": "about:blank"})),
            None,
        )?;
        let target_id = target
            .get("targetId")
            .and_then(Value::as_str)
            .context("Chromium did not return a target id")?;
        let attached = session.command(
            "Target.attachToTarget",
            Some(json!({"targetId": target_id, "flatten": true})),
            None,
        )?;
        session.session_id = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .context("Chromium did not return a CDP session id")?
            .to_owned();
        let session_id = session.session_id.clone();
        for method in [
            "Page.enable",
            "Runtime.enable",
            "Log.enable",
            "Network.enable",
        ] {
            session.command(method, None, Some(&session_id))?;
        }
        session.events.clear();
        Ok(session)
    }

    /// Chromium product string recorded in capture provenance.
    pub(crate) fn browser_product(&self) -> &str {
        &self.browser_product
    }

    /// Captures one URL using the existing browser session.
    pub(crate) fn capture(
        &mut self,
        url: &str,
        viewport: &Viewport,
        theme: &str,
        locale: &str,
        expected_controls: &[String],
    ) -> Result<CapturedCell> {
        let session_id = self.session_id.clone();
        self.command(
            "Emulation.setDeviceMetricsOverride",
            Some(json!({
                "width": viewport.width,
                "height": viewport.height,
                "deviceScaleFactor": viewport.dpr,
                "mobile": false,
                "screenWidth": viewport.width,
                "screenHeight": viewport.height,
            })),
            Some(&session_id),
        )?;
        self.command(
            "Emulation.setEmulatedMedia",
            Some(json!({
                "features": if matches!(theme, "light" | "dark") {
                    json!([{"name": "prefers-color-scheme", "value": theme}])
                } else {
                    json!([])
                },
            })),
            Some(&session_id),
        )?;
        self.command(
            "Emulation.setLocaleOverride",
            Some(json!({"locale": locale})),
            Some(&session_id),
        )?;
        self.command(
            "Page.navigate",
            Some(json!({"url": url})),
            Some(&session_id),
        )?;
        let readiness = self.evaluate(
            r#"(async()=>{await (document.fonts?.ready ?? Promise.resolve()); let previous=null; let stable=0; for(let attempt=0; attempt<30; attempt++){await new Promise(requestAnimationFrame); await new Promise(resolve=>setTimeout(resolve,100)); const text=document.body?.innerText ?? ''; if(text===previous){stable+=1;}else{previous=text; stable=0;} if(text.length>0 && !/\bloading\b/i.test(text) && stable>=1){break;}} for(let attempt=0; attempt<30; attempt++){const images=[...document.images]; if(images.every(image=>image.complete)){await Promise.all(images.filter(image=>image.naturalWidth>0).map(image=>image.decode?.().catch(()=>{}))); await new Promise(requestAnimationFrame); break;} await new Promise(resolve=>setTimeout(resolve,100));} const root=document.documentElement; const body=document.body; const bodyText=body?.innerText ?? ''; return {readyState:document.readyState,title:document.title,fonts:document.fonts?.status ?? 'unavailable',innerWidth:window.innerWidth,innerHeight:window.innerHeight,scrollWidth:root?.scrollWidth ?? 0,scrollHeight:root?.scrollHeight ?? 0,bodyTextBytes:bodyText.length,activeElement:document.activeElement?.tagName ?? null,loadingText:/\bloading\b/i.test(bodyText),visualReady:document.querySelector('[data-visual-ready="true"]') !== null}})()"#,
            &session_id,
        )?;
        if readiness.get("readyState").and_then(Value::as_str) != Some("complete") {
            bail!("page {url} did not reach document.readyState=complete");
        }

        let deterministic = self.evaluate(
            &deterministic_script(expected_controls, theme)?,
            &session_id,
        )?;

        let layout = self.command("Page.getLayoutMetrics", None, Some(&session_id))?;
        let width = layout
            .get("cssContentSize")
            .and_then(|value| value.get("width"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(f64::from(viewport.width));
        let height = layout
            .get("cssContentSize")
            .and_then(|value| value.get("height"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(f64::from(viewport.height));
        let full_page_png = self.screenshot(
            Some(json!({
                "x": 0,
                "y": 0,
                "width": width,
                "height": height,
                "scale": 1,
            })),
            true,
            &session_id,
        )?;
        let viewport_png = self.screenshot(None, false, &session_id)?;
        let dom_snapshot = self.command(
            "DOMSnapshot.captureSnapshot",
            Some(json!({
                "computedStyles": ["display", "visibility", "color", "background-color", "font-size", "line-height"],
                "includePaintOrder": true,
                "includeTextColorOpacities": true,
            })),
            Some(&session_id),
        )?;
        let accessibility = self.command(
            "Accessibility.getFullAXTree",
            Some(json!({})),
            Some(&session_id),
        )?;
        let console = self
            .events
            .drain(..)
            .filter(|event| {
                event
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(|method| {
                        method.starts_with("Runtime.")
                            || method.starts_with("Log.")
                            || method == "Network.loadingFailed"
                    })
            })
            .collect();

        Ok(CapturedCell {
            full_page_png,
            viewport_png,
            dom_snapshot,
            accessibility,
            readiness,
            deterministic,
            console,
        })
    }

    fn evaluate(&mut self, expression: &str, session_id: &str) -> Result<Value> {
        let response = self.command(
            "Runtime.evaluate",
            Some(json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
            })),
            Some(session_id),
        )?;
        Ok(response
            .get("result")
            .and_then(|result| result.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn screenshot(
        &mut self,
        clip: Option<Value>,
        capture_beyond_viewport: bool,
        session_id: &str,
    ) -> Result<Vec<u8>> {
        let mut params = json!({
            "format": "png",
            "fromSurface": true,
            "captureBeyondViewport": capture_beyond_viewport,
        });
        if let Some(clip) = clip {
            params["clip"] = clip;
        }
        let response = self.command("Page.captureScreenshot", Some(params), Some(session_id))?;
        let encoded = response
            .get("data")
            .and_then(Value::as_str)
            .context("Chromium did not return screenshot data")?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("decode Chromium screenshot")
    }

    fn command(
        &mut self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({"id": id, "method": method});
        if let Some(params) = params {
            request["params"] = params;
        }
        if let Some(session_id) = session_id {
            request["sessionId"] = Value::String(session_id.to_owned());
        }
        let encoded = serde_json::to_vec(&request)?;
        self.input.write_all(&encoded)?;
        self.input.write_all(&[0])?;
        self.input.flush()?;

        loop {
            let mut encoded = Vec::new();
            let read = self.output.read_until(0, &mut encoded)?;
            if read == 0 {
                bail!("Chromium CDP pipe closed while waiting for {method}");
            }
            if encoded.last() == Some(&0) {
                encoded.pop();
            }
            if encoded.is_empty() {
                continue;
            }
            let response: Value = serde_json::from_slice(&encoded)
                .with_context(|| format!("parse Chromium CDP response for {method}"))?;
            if response.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = response.get("error") {
                    return Err(anyhow!("Chromium CDP {method} failed: {error}"));
                }
                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            }
            self.events.push(response);
        }
    }
}

fn deterministic_script(expected_controls: &[String], theme: &str) -> Result<String> {
    let expected_controls = serde_json::to_string(expected_controls)
        .context("serialize deterministic control contract")?;
    let expected_theme = serde_json::to_string(theme).context("serialize deterministic theme")?;
    Ok(DETERMINISTIC_SCRIPT
        .replace("__EXPECTED_CONTROLS__", &expected_controls)
        .replace("__EXPECTED_THEME__", &expected_theme))
}

const DETERMINISTIC_SCRIPT: &str = r#"(()=>{
  const expectedControls = __EXPECTED_CONTROLS__;
  const expectedTheme = __EXPECTED_THEME__;
  const observations = [];
  const seen = new Set();
  const push = (kind, summary) => {
    if (!seen.has(kind)) {
      seen.add(kind);
      observations.push({kind, summary});
    }
  };
  const visible = element => {
    const style = getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && style.display !== 'none' &&
      style.visibility !== 'hidden' && Number(style.opacity) !== 0;
  };
  const elements = [...document.querySelectorAll('*')].filter(visible);
  const textElements = elements.filter(element =>
    element.children.length === 0 && element.textContent.trim().length > 0);
  const describe = element => {
    const label = element.getAttribute('aria-label') || element.textContent.trim();
    return `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ''}${label ? ` (${label.slice(0, 80)})` : ''}`;
  };
  const rect = element => {
    const value = element.getBoundingClientRect();
    return {left:value.left, top:value.top, right:value.right, bottom:value.bottom,
      width:value.width, height:value.height};
  };
  const parseColor = value => {
    const match = value.match(/rgba?\(([^)]+)\)/i);
    if (!match) return null;
    const parts = match[1].split(',').map(part => Number.parseFloat(part.trim()));
    if (parts.length < 3 || parts.some(Number.isNaN)) return null;
    return {r:parts[0], g:parts[1], b:parts[2], a:parts.length > 3 && !Number.isNaN(parts[3]) ? parts[3] : 1};
  };
  const luminance = color => {
    const channel = value => {
      const normalized = value / 255;
      return normalized <= .03928 ? normalized / 12.92 : Math.pow((normalized + .055) / 1.055, 2.4);
    };
    return .2126 * channel(color.r) + .7152 * channel(color.g) + .0722 * channel(color.b);
  };
  const contrast = (foreground, background) => {
    const light = Math.max(luminance(foreground), luminance(background));
    const dark = Math.min(luminance(foreground), luminance(background));
    return (light + .05) / (dark + .05);
  };
  const backgroundFor = element => {
    for (let current = element; current && current !== document.documentElement; current = current.parentElement) {
      const color = parseColor(getComputedStyle(current).backgroundColor);
      if (color && color.a > 0) return color;
    }
    return parseColor(getComputedStyle(document.body).backgroundColor);
  };

  const root = document.documentElement;
  if (root.scrollWidth > window.innerWidth + 1) {
    push('overflow', `document scroll width ${root.scrollWidth} exceeds viewport ${window.innerWidth}`);
  }
  for (const element of elements) {
    const style = getComputedStyle(element);
    if ((style.overflowX === 'hidden' || style.overflowX === 'clip' ||
         style.overflow === 'hidden' || style.overflow === 'clip') &&
        element.scrollWidth > element.clientWidth + 1) {
      push('clipping', `${describe(element)} clips ${element.scrollWidth - element.clientWidth}px of content`);
      break;
    }
    if (style.display === 'flex' && style.flexWrap === 'nowrap' &&
        element.children.length > 1 && element.scrollWidth > element.clientWidth + 1) {
      push('responsive-layout', `${describe(element)} keeps a non-wrapping row wider than its container`);
    }
  }

  const overlapCandidates = elements.filter(element =>
    ['BUTTON','A','INPUT','SELECT','TEXTAREA'].includes(element.tagName) ||
    (element.children.length === 0 && element.textContent.trim().length > 0));
  for (let leftIndex = 0; leftIndex < overlapCandidates.length; leftIndex += 1) {
    const left = overlapCandidates[leftIndex];
    const leftRect = left.getBoundingClientRect();
    for (let rightIndex = leftIndex + 1; rightIndex < overlapCandidates.length; rightIndex += 1) {
      const right = overlapCandidates[rightIndex];
      if (left.parentElement !== right.parentElement) continue;
      const rightRect = right.getBoundingClientRect();
      const width = Math.min(leftRect.right, rightRect.right) - Math.max(leftRect.left, rightRect.left);
      const height = Math.min(leftRect.bottom, rightRect.bottom) - Math.max(leftRect.top, rightRect.top);
      if (width > 2 && height > 2 && width * height > Math.min(leftRect.width * leftRect.height, rightRect.width * rightRect.height) * .1) {
        push('overlap', `${describe(left)} overlaps ${describe(right)} at ${JSON.stringify(rect(left))}`);
        leftIndex = overlapCandidates.length;
        break;
      }
    }
  }

  for (const element of elements) {
    const label = element.getAttribute('aria-label');
    const area = element.getBoundingClientRect().width * element.getBoundingClientRect().height;
    if (label && area > 1000 &&
        ['DIV','SECTION','ARTICLE','ASIDE','MAIN'].includes(element.tagName) &&
        !element.textContent.trim() && !element.querySelector('img,svg,canvas')) {
      push('blank-content', `${describe(element)} has no visible content in a ${Math.round(area)}px² region`);
      break;
    }
  }

  let previousHeading = 0;
  for (const heading of document.querySelectorAll('h1,h2,h3,h4,h5,h6')) {
    const level = Number(heading.tagName.slice(1));
    if (previousHeading && level > previousHeading + 1) {
      push('wrong-hierarchy', `heading level jumps from h${previousHeading} to h${level}`);
      break;
    }
    previousHeading = level;
  }

  for (const element of textElements) {
    const style = getComputedStyle(element);
    const size = Number.parseFloat(style.fontSize);
    if (size < 10) {
      push('tiny-text', `${describe(element)} uses ${size}px text`);
    }
    const foreground = parseColor(style.color);
    const background = backgroundFor(element);
    if (foreground && foreground.a > 0 && background && background.a > 0 && contrast(foreground, background) < 3) {
      push('low-contrast', `${describe(element)} has ${contrast(foreground, background).toFixed(2)}:1 text contrast`);
    }
  }

  const focusable = elements.filter(element =>
    /^(A|BUTTON|INPUT|SELECT|TEXTAREA)$/.test(element.tagName) &&
    !element.disabled && element.tabIndex >= 0);
  for (const element of focusable) {
    element.focus();
    if (document.activeElement !== element) continue;
    const style = getComputedStyle(element);
    const noOutline = style.outlineStyle === 'none' || Number.parseFloat(style.outlineWidth) === 0;
    if (noOutline && style.boxShadow === 'none') {
      push('missing-focus', `${describe(element)} has no visible focus indicator`);
      break;
    }
  }
  // The focusability audit must not change the screenshot it is auditing.
  // Leave Chromium on the document body after probing each control so the
  // last focusable element does not acquire a browser-default outline in the
  // captured frame.
  document.activeElement?.blur();
  if (document.body?.innerText && /\bloading\b/i.test(document.body.innerText)) {
    push('stale-state', 'settled capture still contains a loading marker');
  }

  const scheme = getComputedStyle(root).colorScheme.trim();
  if ((expectedTheme === 'dark' && scheme === 'light') ||
      (expectedTheme === 'light' && scheme === 'dark')) {
    push('incorrect-theme', `requested ${expectedTheme} theme but document declares ${scheme}`);
  }
  if (document.getAnimations().some(animation => animation.playState === 'running' && animation.effect)) {
    push('animation-instability', 'one or more animations remained active during capture');
  }
  for (const selector of expectedControls) {
    try {
      if (!document.querySelector(selector)) push('missing-control', `required control ${selector} is absent`);
    } catch (error) {
      push('missing-control', `required control selector ${selector} is invalid`);
    }
  }
  return {schema_version:1, observations};
})()"#;

impl Drop for BrowserSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = &self.profile;
    }
}

/// Writes one JSON metadata value and creates its parent directory.
pub(crate) fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
