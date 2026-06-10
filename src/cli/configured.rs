//! `configured` subcommand — applies the rubric using a TOML config file
//! written by the Home Manager module.
//!
//! The default config path is `~/.config/visual-rubric/config.toml`.
//! CLI flags override individual TOML fields for one-off testing.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

use crate::vision::VisionApiConfig;

use super::QuestionSource;

const DEFAULT_CONFIG_PATH: &str = "visual-rubric/config.toml";

/// Arguments for the `configured` subcommand.
#[derive(Clone, Debug, clap::Parser)]
pub struct ConfiguredArgs {
    /// PNG screenshot path.
    #[arg(long)]
    pub image: PathBuf,

    /// Rubric question (required if --preset is not set).
    #[command(flatten)]
    pub questions: QuestionSource,

    /// Asset name for assertion messages.
    #[arg(long, default_value = "screenshot")]
    pub name: String,

    /// Output verdict as JSON.
    #[arg(long)]
    pub json: bool,

    /// TOML config path (default: ~/.config/visual-rubric/config.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,

    // --- CLI overrides (fall back to TOML, then built-in defaults) ---
    /// Vision API base URL.
    #[arg(long)]
    pub vision_url: Option<String>,

    /// Vision model name.
    #[arg(long)]
    pub vision_model: Option<String>,

    /// Vision API key (Bearer token).
    #[arg(long)]
    pub vision_api_key: Option<String>,

    /// Custom prompt for the vision extraction stage.
    #[arg(long)]
    pub vision_prompt: Option<String>,

    /// ACP binary path (opencode or codex-acp).
    #[arg(long)]
    pub acp_binary: Option<String>,

    /// Extra CLI arguments for the ACP binary. May be repeated.
    #[arg(long = "acp-arg")]
    pub acp_args: Vec<String>,

    /// Rubric model name (passed to codex-acp; ignored for opencode).
    #[arg(long)]
    pub model: Option<String>,

    /// Rubric reasoning effort.
    #[arg(long)]
    pub effort: Option<String>,

    /// Rubric system prompt override.
    #[arg(long)]
    pub system_prompt: Option<String>,
}

/// TOML config shape written by the HM module.
#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlConfig {
    vision: TomlVision,
    rubric: TomlRubric,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlVision {
    url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    prompt: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TomlRubric {
    backend: Option<String>,
    args: Option<Vec<String>>,
    model: Option<String>,
    effort: Option<String>,
    system_prompt: Option<String>,
}

/// Runs the configured pipeline using TOML config + CLI overrides.
///
/// # Errors
///
/// Returns errors from the vision API, ACP, or verdict parsing.
pub fn run_configured(args: ConfiguredArgs) -> Result<()> {
    let question = args.questions.resolve().map_err(|e| anyhow!(e))?;
    let toml = load_config(args.config.as_deref())?;

    let vision_url = args
        .vision_url
        .or(toml.vision.url)
        .context("vision URL is not set. Set it in config.toml or pass --vision-url")?;

    let vision_model = args
        .vision_model
        .or(toml.vision.model)
        .unwrap_or_else(|| "qwen3-vl-8b".to_string());

    let vision_config = VisionApiConfig {
        url: vision_url,
        model: vision_model,
        api_key: args.vision_api_key.or(toml.vision.api_key),
    };

    let vision_prompt = args
        .vision_prompt
        .or(toml.vision.prompt)
        .unwrap_or_else(|| crate::DEFAULT_VISION_PROMPT.to_string());

    let system_prompt = match args.system_prompt.or(toml.rubric.system_prompt) {
        Some(prompt) => Some(prompt),
        None => args
            .questions
            .resolve_system_prompt()
            .map_err(|e| anyhow!(e))?,
    };
    let rubric_options = crate::RubricOptions {
        model: args.model.or(toml.rubric.model),
        effort: args.effort.or(toml.rubric.effort).map(Into::into),
        system_prompt,
    };

    let acp_args = if !args.acp_args.is_empty() {
        args.acp_args
    } else if let Some(toml_args) = &toml.rubric.args {
        toml_args.clone()
    } else {
        vec!["acp".to_string()]
    };

    let acp_binary = args
        .acp_binary
        .or(toml.rubric.backend)
        .unwrap_or_else(|| "opencode".to_string());

    let rubric_config = crate::RubricRunConfig {
        codex_acp_binary: acp_binary.into(),
        acp_args,
        extra_env: Vec::new(),
        cwd: None,
    };

    let verdict = crate::evaluate_image_rubric_pipeline(
        &args.image,
        &question,
        &vision_config,
        &vision_prompt,
        &rubric_options,
        &rubric_config,
    )
    .with_context(|| format!("pipeline for {} failed", args.image.display()))?;

    if args.json {
        println!("{}", serde_json::to_string(&verdict)?);
        return Ok(());
    }

    crate::assert_verdict(&args.name, verdict)
        .map(|()| println!("visual rubric passed"))
        .map_err(|error| anyhow::anyhow!(error))
}

fn load_config(cli_path: Option<&std::path::Path>) -> Result<TomlConfig> {
    let path = match cli_path {
        Some(p) => p.to_path_buf(),
        None => match dirs_config_dir() {
            Some(base) => base.join(DEFAULT_CONFIG_PATH),
            None => return Ok(TomlConfig::default()),
        },
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TomlConfig::default()),
        Err(e) => return Err(e).context(format!("read config {}", path.display())),
    };

    toml::from_str(&content).context(format!("parse config {}", path.display()))
}

fn dirs_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".config"))
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        dirs::config_dir()
    }
}
