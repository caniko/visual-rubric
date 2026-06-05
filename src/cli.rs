use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "visual-rubric")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    image: LegacyImageArgs,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Evaluate one screenshot.
    Image(ImageArgs),
    /// Host a local static site, capture screenshots, and evaluate them.
    Audit(AuditArgs),
    /// Serve a local static directory for manual browser testing.
    Serve(ServeArgs),
}

#[derive(Clone, Debug, Parser)]
struct LegacyImageArgs {
    #[arg(long)]
    image: Option<PathBuf>,
    #[arg(long)]
    question: Option<String>,
    #[arg(long)]
    system_prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long)]
    codex_acp: Option<PathBuf>,
    #[arg(long, default_value = "vnc-screenshot")]
    name: String,
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Debug, Parser)]
struct ImageArgs {
    #[arg(long)]
    image: PathBuf,
    #[arg(long)]
    question: String,
    #[arg(long)]
    system_prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long)]
    codex_acp: Option<PathBuf>,
    #[arg(long, default_value = "screenshot")]
    name: String,
    #[arg(long)]
    json: bool,
}

#[derive(Clone, Debug, Parser)]
struct AuditArgs {
    /// Static site root to serve.
    #[arg(long)]
    root: PathBuf,
    /// Path under the hosted root to capture.
    #[arg(long, default_value = "/")]
    path: String,
    /// Output directory for screenshots.
    #[arg(long, default_value = "target/visual-rubric")]
    screenshots: PathBuf,
    /// JSON report path.
    #[arg(long, default_value = "target/visual-rubric/report.json")]
    report: PathBuf,
    /// Browser binary for headless screenshots.
    #[arg(long, env = "VISUAL_RUBRIC_BROWSER", default_value = "chromium")]
    browser: PathBuf,
    /// Viewports as name=WIDTHxHEIGHT. May be repeated.
    #[arg(long = "viewport")]
    viewports: Vec<ViewportArg>,
    #[arg(long)]
    question: String,
    #[arg(long)]
    system_prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long)]
    codex_acp: Option<PathBuf>,
    /// Generate pass verdicts without starting codex-acp.
    #[arg(long)]
    fake_pass: bool,
    /// Capture screenshots and report deterministic data without model calls.
    #[arg(long)]
    skip_ai: bool,
}

#[derive(Clone, Debug, Parser)]
struct ServeArgs {
    #[arg(long)]
    root: PathBuf,
    #[arg(long, default_value_t = 1111)]
    port: u16,
}

#[derive(Clone, Debug)]
struct ViewportArg {
    name: String,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct AuditReport {
    url: String,
    screenshots: Vec<ScreenshotReport>,
}

#[derive(Serialize)]
struct ScreenshotReport {
    name: String,
    width: u32,
    height: u32,
    path: PathBuf,
    rubric: RubricReport,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RubricReport {
    Pass {
        reason: String,
        anomalies: Vec<String>,
    },
    Fail {
        reason: String,
        anomalies: Vec<String>,
    },
    Error {
        message: String,
    },
    Skipped {
        reason: String,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Commands::Image(args)) => run_image(args),
        Some(Commands::Audit(args)) => run_audit(args),
        Some(Commands::Serve(args)) => run_serve(args),
        None => run_image(cli.image.try_into()?),
    }
}

fn run_image(args: ImageArgs) -> Result<()> {
    let verdict = evaluate_image(&args)?;
    if args.json {
        println!("{}", serde_json::to_string(&verdict)?);
        return Ok(());
    }
    crate::assert_verdict(&args.name, verdict)
        .map(|()| println!("visual rubric passed"))
        .map_err(|error| anyhow!(error))
}

fn run_audit(args: AuditArgs) -> Result<()> {
    create_clean_dir(&args.screenshots)?;
    let viewports = if args.viewports.is_empty() {
        vec![
            ViewportArg {
                name: "desktop".into(),
                width: 1440,
                height: 1100,
            },
            ViewportArg {
                name: "mobile".into(),
                width: 390,
                height: 1200,
            },
        ]
    } else {
        args.viewports.clone()
    };
    let server = StaticServer::start(args.root.clone(), 0)?;
    let url = format!("{}{}", server.base_url(), args.path.trim_start_matches('/'));
    let mut screenshots = Vec::new();

    for viewport in viewports {
        let path = args.screenshots.join(format!("{}.png", viewport.name));
        capture_screenshot(&args.browser, &url, &viewport, &path)?;
        let rubric = if args.fake_pass {
            RubricReport::Pass {
                reason: "fake pass requested".into(),
                anomalies: Vec::new(),
            }
        } else if args.skip_ai {
            RubricReport::Skipped {
                reason: "AI rubric skipped by flag".into(),
            }
        } else {
            evaluate_audit_image(&args, &path)
        };
        screenshots.push(ScreenshotReport {
            name: viewport.name,
            width: viewport.width,
            height: viewport.height,
            path,
            rubric,
        });
    }

    write_report(&args.report, &AuditReport { url, screenshots })
}

fn run_serve(args: ServeArgs) -> Result<()> {
    let server = StaticServer::start(args.root, args.port)?;
    println!("{}", server.base_url());
    server.wait_forever()
}

fn evaluate_image(args: &ImageArgs) -> Result<crate::RubricVerdict> {
    let options = crate::RubricOptions {
        model: args.model.clone(),
        effort: args.effort.clone().map(Into::into),
        system_prompt: args.system_prompt.clone(),
    };
    if let Some(codex_acp) = &args.codex_acp {
        let pool = crate::RubricPool::new(crate::PoolConfig {
            workers: 1,
            codex_acp_binary: codex_acp.clone(),
            default_options: merge_with_defaults(options),
            ..crate::PoolConfig::default()
        })?;
        let verdict = pool.submit(&args.image, &args.question, crate::RubricOptions::default())?;
        let _ = pool.shutdown();
        Ok(verdict)
    } else {
        crate::evaluate_image_rubric_with_options(&args.image, &args.question, options)
            .map_err(|error| anyhow!(error))
    }
}

fn evaluate_audit_image(args: &AuditArgs, image: &Path) -> RubricReport {
    let image_args = ImageArgs {
        image: image.to_path_buf(),
        question: args.question.clone(),
        system_prompt: args.system_prompt.clone(),
        model: args.model.clone(),
        effort: args.effort.clone(),
        codex_acp: args.codex_acp.clone(),
        name: image.display().to_string(),
        json: false,
    };
    match evaluate_image(&image_args) {
        Ok(verdict) if verdict.verdict.is_pass() => RubricReport::Pass {
            reason: verdict.reason,
            anomalies: verdict.anomalies,
        },
        Ok(verdict) => RubricReport::Fail {
            reason: verdict.reason,
            anomalies: verdict.anomalies,
        },
        Err(error) => RubricReport::Error {
            message: error.to_string(),
        },
    }
}

fn merge_with_defaults(mut options: crate::RubricOptions) -> crate::RubricOptions {
    let defaults = crate::default_options();
    if options.model.is_none() {
        options.model = defaults.model;
    }
    if options.effort.is_none() {
        options.effort = defaults.effort;
    }
    if options.system_prompt.is_none() {
        options.system_prompt = defaults.system_prompt;
    }
    options
}

fn create_clean_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("clean {}", path.display()))?;
    }
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))
}

fn write_report(path: &Path, report: &AuditReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(report)?;
    fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

fn capture_screenshot(
    browser: &Path,
    url: &str,
    viewport: &ViewportArg,
    output: &Path,
) -> Result<()> {
    let status = ProcessCommand::new(browser)
        .arg("--headless")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-sandbox")
        .arg(format!(
            "--window-size={},{}",
            viewport.width, viewport.height
        ))
        .arg(format!("--screenshot={}", output.display()))
        .arg(url)
        .status()
        .with_context(|| format!("run browser {}", browser.display()))?;
    if !status.success() {
        bail!(
            "browser {} failed for {} with status {status}",
            browser.display(),
            viewport.name
        );
    }
    if !output.exists() {
        bail!("browser did not write {}", output.display());
    }
    Ok(())
}

struct StaticServer {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    port: u16,
}

impl StaticServer {
    fn start(root: PathBuf, port: u16) -> Result<Self> {
        let listener =
            TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("bind port {port}"))?;
        listener
            .set_nonblocking(true)
            .context("set static server nonblocking")?;
        let port = listener
            .local_addr()
            .context("read static server addr")?
            .port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = serve_static_request(stream, &root);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            stop,
            handle: Some(handle),
            port,
        })
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    fn wait_forever(mut self) -> Result<()> {
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| anyhow!("static server thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve_static_request(mut stream: TcpStream, root: &Path) -> Result<()> {
    let mut buf = [0; 2048];
    let n = stream.read(&mut buf).context("read request")?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let file = resolve_static_path(root, request_path);
    if let Ok(bytes) = fs::read(&file) {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            content_type(&file),
            bytes.len()
        );
        stream.write_all(header.as_bytes())?;
        stream.write_all(&bytes)?;
    } else {
        let body = b"not found";
        let header = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes())?;
        stream.write_all(body)?;
    }
    Ok(())
}

fn resolve_static_path(root: &Path, request_path: &str) -> PathBuf {
    let clean = request_path
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    if clean.is_empty() {
        return root.join("index.html");
    }
    if clean.contains("..") {
        return root.join("__invalid__");
    }
    let path = root.join(clean);
    if request_path.ends_with('/') {
        path.join("index.html")
    } else {
        path
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

impl TryFrom<LegacyImageArgs> for ImageArgs {
    type Error = anyhow::Error;

    fn try_from(value: LegacyImageArgs) -> Result<Self> {
        Ok(Self {
            image: value.image.context("--image is required")?,
            question: value.question.context("--question is required")?,
            system_prompt: value.system_prompt,
            model: value.model,
            effort: value.effort,
            codex_acp: value.codex_acp,
            name: value.name,
            json: value.json,
        })
    }
}

impl std::str::FromStr for ViewportArg {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (name, size) = value
            .split_once('=')
            .context("viewport must be name=WIDTHxHEIGHT")?;
        let (width, height) = size
            .split_once('x')
            .context("viewport size must be WIDTHxHEIGHT")?;
        Ok(Self {
            name: name.to_string(),
            width: width.parse().context("viewport width must be an integer")?,
            height: height
                .parse()
                .context("viewport height must be an integer")?,
        })
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::io::Write as _;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use clap::Parser as _;

    use super::{Cli, Commands, ImageArgs, PathBuf, run};

    #[test]
    fn parses_custom_system_prompt() {
        let cli = Cli::parse_from([
            "visual-rubric",
            "image",
            "--image",
            "shot.png",
            "--question",
            "Is it readable?",
            "--system-prompt",
            "Use this rubric.",
            "--json",
        ]);
        let Some(Commands::Image(image)) = cli.command else {
            panic!("expected image command");
        };
        assert_eq!(image.system_prompt.as_deref(), Some("Use this rubric."));
        assert!(image.json);
    }

    #[test]
    fn parses_legacy_image_args() {
        let cli = Cli::parse_from([
            "visual-rubric",
            "--image",
            "shot.png",
            "--question",
            "Is it readable?",
        ]);
        assert!(cli.command.is_none());
        let image: ImageArgs = cli.image.try_into().unwrap();
        assert_eq!(image.image, PathBuf::from("shot.png"));
    }

    #[test]
    fn parses_audit_viewports() {
        let cli = Cli::parse_from([
            "visual-rubric",
            "audit",
            "--root",
            "public",
            "--question",
            "Is it usable?",
            "--viewport",
            "wide=1440x900",
        ]);
        let Some(Commands::Audit(audit)) = cli.command else {
            panic!("expected audit command");
        };
        assert_eq!(audit.viewports[0].name, "wide");
        assert_eq!(audit.viewports[0].width, 1440);
    }

    #[cfg(unix)]
    #[test]
    fn audit_hosts_static_site_and_writes_report_with_fake_browser() {
        let temp = tempfile::TempDir::new().unwrap();
        let public = temp.path().join("public");
        std::fs::create_dir_all(&public).unwrap();
        std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
        let browser = temp.path().join("fake-browser");
        write_fake_browser(&browser);
        let report = temp.path().join("report.json");
        let screenshots = temp.path().join("shots");

        let cli = Cli::parse_from([
            "visual-rubric",
            "audit",
            "--root",
            public.to_str().unwrap(),
            "--question",
            "Does it render?",
            "--browser",
            browser.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
            "--screenshots",
            screenshots.to_str().unwrap(),
            "--fake-pass",
            "--viewport",
            "tiny=320x240",
        ]);

        run(cli).unwrap();
        assert!(screenshots.join("tiny.png").exists());
        let report = std::fs::read_to_string(report).unwrap();
        assert!(report.contains("\"status\": \"pass\""));
        assert!(report.contains("tiny"));
    }

    #[cfg(unix)]
    fn write_fake_browser(path: &std::path::Path) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(
            br#"#!/usr/bin/env bash
set -euo pipefail
out=
for arg in "$@"; do
  case "$arg" in
    --screenshot=*) out="${arg#--screenshot=}" ;;
  esac
done
test -n "$out"
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=' | base64 -d > "$out"
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
