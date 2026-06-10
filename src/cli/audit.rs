use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use super::static_server::StaticServer;
use super::{AuditArgs, ImageArgs, ViewportArg, evaluate_image};

/// Aggregate status for an audit run.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    /// Every evaluated rubric passed.
    Pass,
    /// At least one evaluated rubric failed.
    Fail,
    /// At least one screenshot produced an execution or model error.
    Error,
    /// All screenshots skipped AI evaluation.
    Skipped,
}

/// JSON report produced by the `audit` command.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditReport {
    /// Report schema version.
    pub schema_version: u32,
    /// Aggregate status across all screenshots.
    pub aggregate_status: AuditStatus,
    /// Hosted URL captured during the audit.
    pub url: String,
    /// Total audit elapsed time in milliseconds.
    pub elapsed_ms: u128,
    /// Options recorded for reproducibility.
    pub options: AuditOptionsReport,
    /// Per-screenshot audit results.
    pub screenshots: Vec<ScreenshotReport>,
}

/// Options recorded in an [`AuditReport`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditOptionsReport {
    /// Rubric question used for each screenshot.
    pub question: String,
    /// Model override used for the run.
    pub model: Option<String>,
    /// Reasoning effort override used for the run.
    pub effort: Option<String>,
    /// Whether a system prompt override was provided.
    pub system_prompt_provided: bool,
    /// Whether AI evaluation was skipped.
    pub skip_ai: bool,
    /// Whether deterministic pass verdicts were generated.
    pub fake_pass: bool,
}

/// One screenshot entry in an [`AuditReport`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ScreenshotReport {
    /// Viewport name.
    pub name: String,
    /// Viewport width in CSS pixels.
    pub width: u32,
    /// Viewport height in CSS pixels.
    pub height: u32,
    /// Screenshot path.
    pub path: PathBuf,
    /// Rubric result for this screenshot.
    pub rubric: RubricReport,
}

/// Per-screenshot rubric outcome.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RubricReport {
    /// The rubric passed.
    Pass {
        /// Pass reason.
        reason: String,
        /// Reported anomalies.
        anomalies: Vec<String>,
    },
    /// The rubric failed.
    Fail {
        /// Failure reason.
        reason: String,
        /// Reported anomalies.
        anomalies: Vec<String>,
    },
    /// The rubric could not be evaluated.
    Error {
        /// Error message.
        message: String,
    },
    /// AI evaluation was skipped.
    Skipped {
        /// Skip reason.
        reason: String,
    },
}

pub(super) fn run_audit(args: AuditArgs) -> Result<()> {
    let started = Instant::now();
    let question = args.questions.resolve().map_err(|e| anyhow!(e))?;
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
        aggregate_status,
        url,
        elapsed_ms: started.elapsed().as_millis(),
        options: AuditOptionsReport {
            question: question.clone(),
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

fn evaluate_audit_image(args: &AuditArgs, image: &Path) -> RubricReport {
    let image_args = ImageArgs {
        image: image.to_path_buf(),
        questions: args.questions.clone(),
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

fn create_clean_dir(path: &Path) -> Result<()> {
    if path.exists() {
        match fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("clean {}", path.display())),
        }
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
    let mut status_line = String::new();
    BufReader::new(stream)
        .read_line(&mut status_line)
        .context("read local server status")?;
    status_line
        .split_whitespace()
        .nth(1)
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
