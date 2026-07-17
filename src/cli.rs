//! Command-line interface and audit report types.

use std::path::PathBuf;

#[cfg(feature = "codex-acp")]
use anyhow::Context as _;
use anyhow::{Result, anyhow};
use clap::{Args, Parser, Subcommand};

use crate::presets::PresetError;

#[cfg(feature = "audit")]
mod audit;
pub mod configured;
#[cfg(feature = "pipeline")]
pub mod pipeline;
mod static_server;
#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "audit"))]
use audit::RubricReport;
#[cfg(feature = "audit")]
use audit::run_audit;
#[cfg(feature = "audit")]
pub use audit::{AuditReport, AuditStatus};
use static_server::StaticServer;
#[cfg(test)]
use static_server::{content_type, resolve_static_path};

/// Parsed command-line interface for `visual-rubric`.
#[derive(Debug, Parser)]
#[command(name = "visual-rubric")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    image: LegacyImageArgs,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Evaluate one screenshot.
    #[cfg(feature = "codex-acp")]
    Image(ImageArgs),
    /// Evaluate an ordered before/after screenshot journey.
    #[cfg(feature = "codex-acp")]
    Sequence(SequenceArgs),
    /// Host a local static site, capture screenshots, and evaluate them.
    #[cfg(feature = "audit")]
    Audit(AuditArgs),
    /// Serve a local static directory for manual browser testing.
    Serve(ServeArgs),
    /// Two-stage pipeline: vision model → rubric model.
    ///
    /// Stage 1 sends the screenshot to an OpenAI-compatible vision API
    /// (e.g. Qwen VL via llama-swap) producing structured JSON.
    /// Stage 2 sends that description to an ACP backend (opencode with
    /// DeepSeek V4, or codex-acp) for the final rubric verdict.
    #[cfg(feature = "pipeline")]
    Pipeline(pipeline::PipelineArgs),
    /// Pipeline configured via a TOML file written by the HM module.
    ///
    /// Reads configuration from `~/.config/visual-rubric/config.toml` so the
    /// user only needs `--image` and `--question`. CLI flags override
    /// individual TOML fields for one-off testing.
    Configured(configured::ConfiguredArgs),
}

#[derive(Clone, Debug, Parser)]
struct LegacyImageArgs {
    #[arg(long)]
    image: Option<PathBuf>,
    #[arg(long)]
    question: Option<String>,
    #[arg(long)]
    preset: Option<String>,
    #[arg(long)]
    system_prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[cfg(feature = "codex-acp")]
    #[arg(long)]
    codex_acp: Option<PathBuf>,
    #[arg(long, default_value = "vnc-screenshot")]
    name: String,
    #[arg(long)]
    json: bool,
}

#[cfg(feature = "codex-acp")]
#[derive(Clone, Debug, Parser)]
struct ImageArgs {
    #[arg(long)]
    image: PathBuf,
    #[command(flatten)]
    questions: QuestionSource,
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

#[cfg(feature = "codex-acp")]
#[derive(Clone, Debug, Parser)]
struct SequenceArgs {
    /// Ordered checkpoint as LABEL=PATH. May be repeated.
    #[arg(long = "frame", value_parser = parse_sequence_frame, required = true, num_args = 1..)]
    frames: Vec<SequenceFrameArg>,
    #[command(flatten)]
    questions: QuestionSource,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    #[arg(long)]
    codex_acp: Option<PathBuf>,
    #[arg(long, default_value = "sequence")]
    name: String,
    #[arg(long)]
    json: bool,
    /// Override the configured maximum number of ordered checkpoints.
    #[arg(long)]
    max_frames: Option<usize>,
    /// Disable semantic before/after transition assessment for this run.
    #[arg(long)]
    no_require_transition: bool,
}

#[cfg(feature = "codex-acp")]
#[derive(Clone, Debug)]
struct SequenceFrameArg {
    label: String,
    path: PathBuf,
}

#[cfg(feature = "audit")]
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
    #[command(flatten)]
    questions: QuestionSource,
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

#[cfg(feature = "audit")]
#[derive(Clone, Debug)]
struct ViewportArg {
    name: String,
    width: u32,
    height: u32,
}

/// Source of the rubric question: either an explicit `--question` string
/// or a named `--preset`.  At least one of the two must be provided.
#[derive(Clone, Debug, Args)]
pub struct QuestionSource {
    /// Explicit rubric question (required if --preset is not set).
    #[arg(long, required_unless_present = "preset")]
    pub question: Option<String>,

    /// Named question preset (required if --question is not set).
    #[arg(long, required_unless_present = "question")]
    pub preset: Option<String>,
}

impl QuestionSource {
    /// Build a source from an already-known question string (no preset).
    pub fn from_question(question: String) -> Self {
        Self {
            question: Some(question),
            preset: None,
        }
    }

    /// Resolves the effective question text.
    ///
    /// When `--preset` was provided, delegates to [`crate::presets::resolve`].
    /// When `--question` was provided, returns it verbatim.
    pub fn resolve(&self) -> Result<String, PresetError> {
        match (&self.preset, &self.question) {
            (Some(name), _) => {
                let questions = crate::presets::resolve(name)?;
                Ok(questions.join("\n"))
            }
            (None, Some(q)) => Ok(q.clone()),
            (None, None) => Err(PresetError::MissingQuestionSource),
        }
    }

    /// Resolves the preset's standard system prompt.
    ///
    /// Returns `None` when no preset was selected or the preset does not
    /// define a system prompt.  Callers should prefer an explicit
    /// `--system-prompt` over this value.
    pub fn resolve_system_prompt(&self) -> Result<Option<String>, PresetError> {
        match &self.preset {
            Some(name) => Ok(crate::presets::find(name)?
                .system_prompt()
                .map(str::to_owned)),
            None => Ok(None),
        }
    }
}

/// Runs a parsed CLI command.
///
/// # Errors
///
/// Returns command, IO, browser, ACP, or rubric audit failures.
pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        #[cfg(feature = "codex-acp")]
        Some(Commands::Image(args)) => run_image(args),
        #[cfg(feature = "codex-acp")]
        Some(Commands::Sequence(args)) => run_sequence(args),
        #[cfg(feature = "audit")]
        Some(Commands::Audit(args)) => run_audit(args),
        Some(Commands::Serve(args)) => run_serve(args),
        #[cfg(feature = "pipeline")]
        Some(Commands::Pipeline(args)) => pipeline::run_pipeline(args),
        Some(Commands::Configured(args)) => configured::run_configured(args),
        #[cfg(feature = "codex-acp")]
        None => run_image(cli.image.try_into()?),
        #[cfg(not(feature = "codex-acp"))]
        None => Err(anyhow!(
            "no subcommand given; use --help for available commands"
        )),
    }
}

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
fn run_sequence(args: SequenceArgs) -> Result<()> {
    let question = args.questions.resolve().map_err(|error| anyhow!(error))?;
    let frames = args
        .frames
        .into_iter()
        .map(|frame| crate::SequenceFrame {
            label: frame.label,
            path: frame.path,
        })
        .collect::<Vec<_>>();
    let options = crate::RubricOptions {
        model: args.model,
        effort: args.effort.map(Into::into),
        system_prompt: args
            .questions
            .resolve_system_prompt()
            .map_err(|e| anyhow!(e))?,
    };
    let configured_sequence = crate::SequenceOptions::from_config_toml(None);
    let sequence_options = crate::SequenceOptions {
        max_frames: args.max_frames.unwrap_or(configured_sequence.max_frames),
        require_transition: configured_sequence.require_transition && !args.no_require_transition,
    };
    let mut runtime_config = crate::RubricRunConfig::from_config_toml(None);
    if let Some(binary) = args.codex_acp {
        runtime_config.codex_acp_binary = binary;
    }
    let verdict = crate::evaluate_image_sequence_rubric_with_options(
        &frames,
        &question,
        options,
        runtime_config,
        sequence_options,
    )?;
    if args.json {
        println!("{}", serde_json::to_string(&verdict)?);
        return Ok(());
    }
    crate::assert_verdict(&args.name, verdict)
        .map(|()| println!("visual sequence rubric passed"))
        .map_err(|error| anyhow!(error))
}

fn run_serve(args: ServeArgs) -> Result<()> {
    let server = StaticServer::start(args.root, args.port)?;
    println!("{}", server.base_url());
    server.wait_forever()
}

#[cfg(feature = "codex-acp")]
fn evaluate_image(args: &ImageArgs) -> Result<crate::RubricVerdict> {
    let question = args.questions.resolve().map_err(|e| anyhow!(e))?;
    let system_prompt = match args.system_prompt.clone() {
        Some(prompt) => Some(prompt),
        None => args
            .questions
            .resolve_system_prompt()
            .map_err(|e| anyhow!(e))?,
    };
    let options = crate::RubricOptions {
        model: args.model.clone(),
        effort: args.effort.clone().map(Into::into),
        system_prompt,
    };
    if let Some(codex_acp) = &args.codex_acp {
        let pool = crate::RubricPool::new(crate::PoolConfig {
            workers: 1,
            codex_acp_binary: codex_acp.clone(),
            default_options: merge_with_defaults(options),
            ..crate::PoolConfig::default()
        })?;
        let verdict = pool.submit(&args.image, &question, crate::RubricOptions::default())?;
        let _ = pool.shutdown();
        Ok(verdict)
    } else {
        crate::evaluate_image_rubric_with_options(&args.image, &question, options)
            .map_err(|error| anyhow!(error))
    }
}

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
impl TryFrom<LegacyImageArgs> for ImageArgs {
    type Error = anyhow::Error;

    fn try_from(value: LegacyImageArgs) -> Result<Self> {
        let questions = match (value.question, value.preset) {
            (Some(q), _) => QuestionSource::from_question(q),
            (None, Some(preset)) => QuestionSource {
                question: None,
                preset: Some(preset),
            },
            (None, None) => {
                anyhow::bail!("either --question or --preset is required");
            }
        };
        Ok(Self {
            image: value.image.context("--image is required")?,
            questions,
            system_prompt: value.system_prompt,
            model: value.model,
            effort: value.effort,
            codex_acp: value.codex_acp,
            name: value.name,
            json: value.json,
        })
    }
}

#[cfg(feature = "codex-acp")]
fn parse_sequence_frame(value: &str) -> Result<SequenceFrameArg, String> {
    let (label, path) = value
        .split_once('=')
        .ok_or_else(|| "frame must be LABEL=PATH".to_owned())?;
    if label.trim().is_empty() {
        return Err("frame label must not be empty".to_owned());
    }
    if path.trim().is_empty() {
        return Err("frame path must not be empty".to_owned());
    }
    Ok(SequenceFrameArg {
        label: label.to_owned(),
        path: PathBuf::from(path),
    })
}

#[cfg(feature = "audit")]
impl std::str::FromStr for ViewportArg {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let (name, size) = value
            .split_once('=')
            .context("viewport must be name=WIDTHxHEIGHT")?;
        let (width, height) = size
            .split_once('x')
            .context("viewport size must be WIDTHxHEIGHT")?;
        if name.is_empty() {
            anyhow::bail!("viewport name must not be empty");
        }
        if name.contains('/') || name.contains('\\') {
            anyhow::bail!("viewport name {name:?} must not contain path separators");
        }
        let width = width.parse().context("viewport width must be an integer")?;
        let height = height
            .parse()
            .context("viewport height must be an integer")?;
        if width == 0 || height == 0 {
            anyhow::bail!("viewport dimensions must be greater than zero");
        }
        Ok(Self {
            name: name.to_string(),
            width,
            height,
        })
    }
}
