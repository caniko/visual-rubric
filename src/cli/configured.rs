//! `configured` subcommand — applies the rubric using environment-variable
//! defaults set by the Home Manager module.
//!
//! All model/URL configuration is read from `VISUAL_RUBRIC_*` environment
//! variables, so the user only needs `--image` and `--question`. CLI flags
//! override individual env vars for one-off testing.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::vision::VisionApiConfig;

/// Arguments for the `configured` subcommand.
#[derive(Clone, Debug, clap::Parser)]
pub struct ConfiguredArgs {
    /// PNG screenshot path.
    #[arg(long)]
    pub image: PathBuf,

    /// Rubric question.
    #[arg(long)]
    pub question: String,

    /// Asset name for assertion messages.
    #[arg(long, default_value = "screenshot")]
    pub name: String,

    /// Output verdict as JSON.
    #[arg(long)]
    pub json: bool,

    // --- Configuration read from env vars ---
    /// Vision API base URL.
    /// Reads from VISUAL_RUBRIC_VISION_URL when not provided.
    #[arg(long, env = "VISUAL_RUBRIC_VISION_URL")]
    pub vision_url: Option<String>,

    /// Vision model name.
    /// Reads from VISUAL_RUBRIC_VISION_MODEL.
    #[arg(
        long,
        env = "VISUAL_RUBRIC_VISION_MODEL",
        default_value = "qwen3-vl-8b"
    )]
    pub vision_model: String,

    /// Vision API key (Bearer token).
    /// Reads from VISUAL_RUBRIC_VISION_API_KEY.
    #[arg(long, env = "VISUAL_RUBRIC_VISION_API_KEY")]
    pub vision_api_key: Option<String>,

    /// Custom prompt for the vision extraction stage.
    /// Reads from VISUAL_RUBRIC_VISION_PROMPT.
    #[arg(long, env = "VISUAL_RUBRIC_VISION_PROMPT")]
    pub vision_prompt: Option<String>,

    /// ACP binary path (opencode or codex-acp).
    /// Reads from VISUAL_RUBRIC_ACP_BINARY.
    #[arg(long, env = "VISUAL_RUBRIC_ACP_BINARY", default_value = "opencode")]
    pub acp_binary: PathBuf,

    /// Extra CLI arguments for the ACP binary. May be repeated.
    /// When not provided on the CLI, parsed from VISUAL_RUBRIC_ACP_ARGS
    /// (space-separated string, default "acp").
    #[arg(long = "acp-arg")]
    pub acp_args: Vec<String>,

    /// Rubric model name (passed to codex-acp; ignored for opencode).
    /// Reads from VISUAL_RUBRIC_MODEL.
    #[arg(long, env = "VISUAL_RUBRIC_MODEL")]
    pub model: Option<String>,

    /// Rubric reasoning effort.
    /// Reads from VISUAL_RUBRIC_EFFORT.
    #[arg(long, env = "VISUAL_RUBRIC_EFFORT")]
    pub effort: Option<String>,

    /// Rubric system prompt override.
    /// Reads from VISUAL_RUBRIC_SYSTEM_PROMPT.
    #[arg(long, env = "VISUAL_RUBRIC_SYSTEM_PROMPT")]
    pub system_prompt: Option<String>,
}

/// Runs the configured pipeline using env-var defaults.
///
/// # Errors
///
/// Returns errors from the vision API, ACP, or verdict parsing.
pub fn run_configured(args: ConfiguredArgs) -> Result<()> {
    let vision_url = args.vision_url.context(
        "VISUAL_RUBRIC_VISION_URL is not set. Either pass --vision-url or set the environment variable.",
    )?;

    let vision_config = VisionApiConfig {
        url: vision_url,
        model: args.vision_model,
        api_key: args.vision_api_key,
    };

    let vision_prompt = args
        .vision_prompt
        .unwrap_or_else(|| crate::DEFAULT_VISION_PROMPT.to_string());

    let rubric_options = crate::RubricOptions {
        model: args.model,
        effort: args.effort.map(Into::into),
        system_prompt: args.system_prompt,
    };

    let acp_args = if args.acp_args.is_empty() {
        parse_acp_args_from_env()
    } else {
        args.acp_args
    };

    let rubric_config = crate::RubricRunConfig {
        codex_acp_binary: args.acp_binary,
        acp_args,
        extra_env: Vec::new(),
        cwd: None,
    };

    let verdict = crate::evaluate_image_rubric_pipeline(
        &args.image,
        &args.question,
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

/// Parses `VISUAL_RUBRIC_ACP_ARGS` as a space-separated list of arguments.
fn parse_acp_args_from_env() -> Vec<String> {
    match std::env::var("VISUAL_RUBRIC_ACP_ARGS") {
        Ok(val) if !val.is_empty() => val.split_whitespace().map(str::to_string).collect(),
        _ => vec!["acp".to_string()],
    }
}
