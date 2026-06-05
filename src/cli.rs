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
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

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
    /// Extra argument passed to the browser. May be repeated.
    #[arg(long = "browser-arg")]
    browser_args: Vec<String>,
    /// Delay before each browser capture, in milliseconds.
    #[arg(long, default_value_t = 0)]
    wait_ms: u64,
    /// Device scale factor passed to Chromium.
    #[arg(long)]
    device_scale_factor: Option<f32>,
    /// Number of times to retry a failed browser capture.
    #[arg(long, default_value_t = 0)]
    capture_retries: u32,
    /// Return a non-zero exit when any rubric fails or errors.
    #[arg(long)]
    fail_on_rubric: bool,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    Pass,
    Fail,
    Error,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditReport {
    pub schema_version: u32,
    pub aggregate_status: AuditStatus,
    url: String,
    elapsed_ms: u128,
    options: AuditOptionsReport,
    screenshots: Vec<ScreenshotReport>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct AuditOptionsReport {
    question: String,
    model: Option<String>,
    effort: Option<String>,
    system_prompt_provided: bool,
    skip_ai: bool,
    fake_pass: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ScreenshotReport {
    name: String,
    width: u32,
    height: u32,
    path: PathBuf,
    rubric: RubricReport,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    let started = Instant::now();
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
    ensure_hosted_path_ok(&url)?;
    let mut screenshots = Vec::new();

    for viewport in viewports {
        let path = args.screenshots.join(format!("{}.png", viewport.name));
        capture_screenshot(&args, &url, &viewport, &path)?;
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

    let aggregate_status = aggregate_status(&screenshots);
    let report = AuditReport {
        schema_version: 1,
        aggregate_status: aggregate_status.clone(),
        url,
        elapsed_ms: started.elapsed().as_millis(),
        options: AuditOptionsReport {
            question: args.question.clone(),
            model: args.model.clone(),
            effort: args.effort.clone(),
            system_prompt_provided: args.system_prompt.is_some(),
            skip_ai: args.skip_ai,
            fake_pass: args.fake_pass,
        },
        screenshots,
    };
    write_report(&args.report, &report)?;
    if args.fail_on_rubric && matches!(aggregate_status, AuditStatus::Fail | AuditStatus::Error) {
        bail!("visual rubric audit finished with aggregate status {aggregate_status:?}");
    }
    Ok(())
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

fn aggregate_status(screenshots: &[ScreenshotReport]) -> AuditStatus {
    if screenshots
        .iter()
        .any(|screenshot| matches!(screenshot.rubric, RubricReport::Error { .. }))
    {
        AuditStatus::Error
    } else if screenshots
        .iter()
        .any(|screenshot| matches!(screenshot.rubric, RubricReport::Fail { .. }))
    {
        AuditStatus::Fail
    } else if screenshots
        .iter()
        .all(|screenshot| matches!(screenshot.rubric, RubricReport::Skipped { .. }))
    {
        AuditStatus::Skipped
    } else {
        AuditStatus::Pass
    }
}

fn ensure_hosted_path_ok(url: &str) -> Result<()> {
    let status = http_status(url).with_context(|| format!("check hosted path {url}"))?;
    if status != 200 {
        bail!("hosted path {url} returned HTTP {status}");
    }
    Ok(())
}

fn http_status(url: &str) -> Result<u16> {
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .context("only local audit URLs are supported")?;
    let (port, path) = rest
        .split_once('/')
        .context("local audit URL missing path")?;
    let port = port.parse::<u16>().context("local audit URL port")?;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).context("connect local server")?;
    write!(
        stream,
        "GET /{path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("read local server response")?;
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("missing HTTP status")?
        .parse()
        .context("parse HTTP status")
}

fn capture_screenshot(
    args: &AuditArgs,
    url: &str,
    viewport: &ViewportArg,
    output: &Path,
) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..=args.capture_retries {
        if args.wait_ms > 0 {
            thread::sleep(Duration::from_millis(args.wait_ms));
        }
        match capture_screenshot_once(args, url, viewport, output) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt < args.capture_retries {
            thread::sleep(Duration::from_millis(100));
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("browser capture failed")))
}

fn capture_screenshot_once(
    args: &AuditArgs,
    url: &str,
    viewport: &ViewportArg,
    output: &Path,
) -> Result<()> {
    let mut command = ProcessCommand::new(&args.browser);
    command
        .arg("--headless")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-sandbox")
        .arg(format!(
            "--window-size={},{}",
            viewport.width, viewport.height
        ));
    if let Some(scale) = args.device_scale_factor {
        command.arg(format!("--force-device-scale-factor={scale}"));
    }
    command
        .args(&args.browser_args)
        .arg(format!("--screenshot={}", output.display()))
        .arg(url);
    let status = command
        .status()
        .with_context(|| format!("run browser {}", args.browser.display()))?;
    if !status.success() {
        bail!(
            "browser {} failed for {} with status {status}",
            args.browser.display(),
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
    let mut request_parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = request_parts.next().unwrap_or_default();
    let request_path = request_parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        return write_http_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"method not allowed",
            method == "HEAD",
        );
    }
    let file = resolve_static_path(root, request_path);
    if let Ok(bytes) = fs::read(&file) {
        write_http_response(
            &mut stream,
            "200 OK",
            content_type(&file),
            &bytes,
            method == "HEAD",
        )?;
    } else {
        write_http_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found",
            method == "HEAD",
        )?;
    }
    Ok(())
}

fn resolve_static_path(root: &Path, request_path: &str) -> PathBuf {
    let request_path_without_query = request_path.split('?').next().unwrap_or("/");
    let Some(clean) = percent_decode_path(request_path_without_query) else {
        return root.join("__invalid__");
    };
    let clean = clean.trim_start_matches('/');
    if clean.is_empty() {
        return root.join("index.html");
    }
    if clean
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return root.join("__invalid__");
    }
    let path = root.join(clean);
    if request_path_without_query.ends_with('/') {
        path.join("index.html")
    } else {
        path
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("gif") => "image/gif",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !head_only {
        stream.write_all(body)?;
    }
    Ok(())
}

fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            decoded.push(hex_value(hi)? * 16 + hex_value(lo)?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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

    use super::{AuditReport, AuditStatus, Cli, Commands, ImageArgs, PathBuf, run};

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
    fn legacy_image_args_require_image_and_question() {
        let cli = Cli::parse_from(["visual-rubric", "--question", "Is it readable?"]);
        let err = ImageArgs::try_from(cli.image).unwrap_err();
        assert!(err.to_string().contains("--image is required"));

        let cli = Cli::parse_from(["visual-rubric", "--image", "shot.png"]);
        let err = ImageArgs::try_from(cli.image).unwrap_err();
        assert!(err.to_string().contains("--question is required"));
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
        let report: AuditReport =
            serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.aggregate_status, AuditStatus::Pass);
        assert_eq!(report.screenshots[0].name, "tiny");
        assert!(matches!(
            report.screenshots[0].rubric,
            super::RubricReport::Pass { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn audit_skip_ai_uses_default_viewports_and_report_contract() {
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
            "--skip-ai",
        ]);

        run(cli).unwrap();
        let report: AuditReport =
            serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.aggregate_status, AuditStatus::Skipped);
        assert_eq!(report.screenshots.len(), 2);
        assert_eq!(report.screenshots[0].name, "desktop");
        assert_eq!(report.screenshots[0].width, 1440);
        assert_eq!(report.screenshots[1].name, "mobile");
        assert_eq!(report.screenshots[1].width, 390);
        assert!(matches!(
            report.screenshots[0].rubric,
            super::RubricReport::Skipped { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn audit_preserves_multiple_viewport_order_and_custom_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let public = temp.path().join("public");
        std::fs::create_dir_all(public.join("__audit")).unwrap();
        std::fs::write(public.join("__audit/install.html"), "<h1>Install</h1>").unwrap();
        let browser = temp.path().join("fake-browser");
        write_fake_browser(&browser);
        let report = temp.path().join("report.json");

        let cli = Cli::parse_from([
            "visual-rubric",
            "audit",
            "--root",
            public.to_str().unwrap(),
            "--path",
            "__audit/install.html",
            "--question",
            "Does it render?",
            "--browser",
            browser.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
            "--skip-ai",
            "--viewport",
            "wide=1200x800",
            "--viewport",
            "narrow=320x700",
        ]);

        run(cli).unwrap();
        let report: AuditReport =
            serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
        assert!(report.url.ends_with("/__audit/install.html"));
        assert_eq!(report.screenshots[0].name, "wide");
        assert_eq!(report.screenshots[1].name, "narrow");
    }

    #[cfg(unix)]
    #[test]
    fn audit_errors_when_browser_fails_or_writes_no_screenshot() {
        let temp = tempfile::TempDir::new().unwrap();
        let public = temp.path().join("public");
        std::fs::create_dir_all(&public).unwrap();
        std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
        let failing_browser = temp.path().join("failing-browser");
        write_fake_browser_script(&failing_browser, "#!/usr/bin/env bash\nexit 7\n");
        let report = temp.path().join("report.json");

        let cli = Cli::parse_from([
            "visual-rubric",
            "audit",
            "--root",
            public.to_str().unwrap(),
            "--question",
            "Does it render?",
            "--browser",
            failing_browser.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
            "--skip-ai",
            "--viewport",
            "tiny=320x240",
        ]);
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains("browser"));

        let silent_browser = temp.path().join("silent-browser");
        write_fake_browser_script(&silent_browser, "#!/usr/bin/env bash\nexit 0\n");
        let cli = Cli::parse_from([
            "visual-rubric",
            "audit",
            "--root",
            public.to_str().unwrap(),
            "--question",
            "Does it render?",
            "--browser",
            silent_browser.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
            "--skip-ai",
            "--viewport",
            "tiny=320x240",
        ]);
        let err = run(cli).unwrap_err();
        assert!(err.to_string().contains("did not write"));
    }

    #[test]
    fn static_path_resolution_and_content_types_are_strict() {
        let root = PathBuf::from("/tmp/site");
        assert_eq!(
            super::resolve_static_path(&root, "/"),
            root.join("index.html")
        );
        assert_eq!(
            super::resolve_static_path(&root, "/docs/?v=1"),
            root.join("docs").join("index.html")
        );
        assert_eq!(
            super::resolve_static_path(&root, "/assets%2Fapp.js"),
            root.join("assets").join("app.js")
        );
        assert_eq!(
            super::resolve_static_path(&root, "/%2e%2e/secret.txt"),
            root.join("__invalid__")
        );
        assert_eq!(
            super::resolve_static_path(&root, "/bad%zz"),
            root.join("__invalid__")
        );
        assert_eq!(
            super::content_type(&root.join("app.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            super::content_type(&root.join("data.json")),
            "application/json; charset=utf-8"
        );
    }

    #[cfg(unix)]
    fn write_fake_browser(path: &std::path::Path) {
        write_fake_browser_script(
            path,
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
        );
    }

    #[cfg(unix)]
    fn write_fake_browser_script(path: &std::path::Path, script: impl AsRef<[u8]>) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(script.as_ref()).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
