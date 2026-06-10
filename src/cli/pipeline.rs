//! Two-stage pipeline: vision extraction → rubric scoring.
//!
//! Stage 1 calls an OpenAI-compatible vision API (e.g. Qwen VL via
//! llama-swap) to produce a structured JSON description of the screenshot.
//! Stage 2 sends that description to an ACP backend (opencode or codex-acp)
//! for the final rubric verdict.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::vision::VisionApiConfig;

/// Arguments for the pipeline subcommand.
#[derive(Clone, Debug, clap::Parser)]
pub struct PipelineArgs {
    /// PNG screenshot path.
    #[arg(long)]
    pub image: PathBuf,

    /// Rubric question.
    #[arg(long)]
    pub question: String,

    /// Vision API base URL (e.g. http://localhost:8013).
    #[arg(long)]
    pub vision_url: String,

    /// Vision model name (e.g. qwen3-vl-8b).
    #[arg(long, default_value = "qwen3-vl-8b")]
    pub vision_model: String,

    /// Vision API key (Bearer token).
    #[arg(long)]
    pub vision_api_key: Option<String>,

    /// Custom prompt for the vision extraction stage.
    #[arg(long)]
    pub vision_prompt: Option<String>,

    /// ACP binary path (opencode or codex-acp).
    #[arg(long, default_value = "opencode")]
    pub acp_binary: PathBuf,

    /// Extra CLI arguments for the ACP binary. May be repeated.
    /// Default for codex-acp: -c model=... -c model_reasoning_effort=...
    /// For opencode (default): acp
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

    /// Output verdict as JSON.
    #[arg(long)]
    pub json: bool,

    /// Asset name for assertion messages.
    #[arg(long, default_value = "screenshot")]
    pub name: String,
}

/// Runs the two-stage pipeline.
///
/// # Errors
///
/// Returns errors from the vision API, ACP, or verdict parsing.
pub fn run_pipeline(args: PipelineArgs) -> Result<()> {
    let vision_config = VisionApiConfig {
        url: args.vision_url,
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
        vec!["acp".to_string()]
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
    .context("pipeline evaluation failed")?;

    if args.json {
        println!("{}", serde_json::to_string(&verdict)?);
        return Ok(());
    }

    crate::assert_verdict(&args.name, verdict)
        .map(|()| println!("visual rubric passed"))
        .map_err(|error| anyhow::anyhow!(error))
}
