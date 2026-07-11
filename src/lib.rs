//! Shared AI visual-rubric runner for screenshot review.
//!
//! This crate owns the Codex ACP plumbing so browser screenshots, offscreen
//! renderer captures, and VM/VNC screenshots can use one rubric path.
//!
//! Direct Codex ACP evaluation sends the rubric prompt and screenshot in one
//! multimodal request. The crate also provides a two-stage pipeline: vision
//! model extraction via an OpenAI-compatible HTTP API, then rubric scoring via
//! ACP.
#![warn(missing_docs)]
#![allow(
    clippy::io_other_error,
    clippy::manual_checked_ops,
    clippy::single_match
)]

#[cfg(feature = "acp")]
mod acp;
#[cfg(feature = "batch")]
mod batch;
pub mod cli;
mod config;
mod configured_eval;
mod errors;
#[cfg(feature = "pool")]
mod pool;
pub mod presets;
mod report;
mod typed_strings;
mod verdict;
#[cfg(any(feature = "vision-api", feature = "http-rubric"))]
pub mod vision;

use std::path::Path;

#[cfg(feature = "acp")]
use acp::AcpClient;
use base64::Engine as _;
#[cfg(any(feature = "vision-api", feature = "http-rubric"))]
use vision::VisionApiConfig;

#[cfg(feature = "codex-acp")]
pub use acp::build_codex_acp_args;
#[cfg(feature = "batch")]
pub use batch::{
    AggregateStatus, AssetChange, AssetRubricReport, AssetRubricResult, AssetSnapshot,
    BatchRubricConfig, BatchRubricReport, BatchRubricRun, IssueClassificationInput,
    IssueClassifier, IssueRecommendation, RecommendationSeverity, SelectionMode, diff_snapshots,
    select_changed,
};
pub use cli::Cli;
pub use config::{
    ConfigMode, RubricOptions, RubricRunConfig, TomlConfig, TomlRubric, TomlVision,
    load_config_toml,
};
#[cfg(feature = "codex-acp")]
pub use config::{default_codex_acp_binary, default_options, direct_codex_gpt_config};
pub use configured_eval::{evaluate_configured, evaluate_configured_with_vision};
pub use errors::{PoolError, RateLimitEvent, RubricError};
#[cfg(feature = "pool")]
pub use pool::{LogCaptureConfig, LogPathMode, PoolConfig, PoolStats, RubricPool};
pub use report::{PageResult, RubricReport};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};
pub use verdict::{RubricVerdict, assert_verdict, parse_verdict};

/// Default system prompt used for screenshot rubric requests.
///
/// Shared with the `ui-regression` question preset.
pub const DEFAULT_SYSTEM_PROMPT: &str = presets::UI_REGRESSION_SYSTEM_PROMPT;

/// Default Codex ACP model.
#[cfg(feature = "codex-acp")]
pub const DEFAULT_CODEX_ACP_MODEL: &str = "gpt-5.5";
/// Default Codex ACP reasoning effort.
#[cfg(feature = "codex-acp")]
pub const DEFAULT_CODEX_ACP_REASONING_EFFORT: &str = "medium";

/// Default prompt for the vision extraction stage.
///
/// Asks the vision model to describe the screenshot as structured JSON
/// so a text-only rubric model (e.g. DeepSeek V4 via opencode) can score it.
#[cfg(feature = "vision-api")]
pub const DEFAULT_VISION_PROMPT: &str = "\
You are a UI description engine. Given a screenshot, produce a structured JSON \
description of all visible user interface elements, their text content, layout, \
and any visual issues (clipping, overlap, blank regions, contrast problems). \
Output ONLY valid JSON with no additional text.";

/// Reads and base64-encodes a PNG file.
///
/// # Errors
///
/// Returns [`PoolError::Rpc`] when the PNG cannot be read.
pub fn encode_png(png_path: &Path) -> Result<String, PoolError> {
    let bytes = std::fs::read(png_path)
        .map_err(|e| PoolError::Rpc(format!("read png {}: {e}", png_path.display())))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Evaluates a PNG and returns an error when the verdict is not pass.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, JSON parsing, or failed
/// assertion errors.
#[cfg(feature = "codex-acp")]
pub fn assert_image_rubric(png_path: &Path, name: &str, question: &str) -> Result<(), RubricError> {
    let verdict = evaluate_image_rubric(png_path, question)?;
    assert_verdict(name, verdict)
}

/// Evaluates a PNG with default options.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, or verdict parsing failures.
#[cfg(feature = "codex-acp")]
pub fn evaluate_image_rubric(
    png_path: &Path,
    question: &str,
) -> Result<RubricVerdict, RubricError> {
    evaluate_image_rubric_with_options(png_path, question, default_options())
}

/// Evaluates a PNG with caller-provided model options.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, or verdict parsing failures.
#[cfg(feature = "codex-acp")]
pub fn evaluate_image_rubric_with_options(
    png_path: &Path,
    question: &str,
    opts: RubricOptions,
) -> Result<RubricVerdict, RubricError> {
    evaluate_image_rubric_with_config(png_path, question, opts, RubricRunConfig::default())
}

/// Evaluates a PNG with caller-provided model and runtime configuration.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, or verdict parsing failures.
#[cfg(feature = "codex-acp")]
pub fn evaluate_image_rubric_with_config(
    png_path: &Path,
    question: &str,
    opts: RubricOptions,
    config: RubricRunConfig,
) -> Result<RubricVerdict, RubricError> {
    let bytes = std::fs::read(png_path).map_err(|source| RubricError::ReadPng {
        path: png_path.to_path_buf(),
        source,
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let text = run_codex_acp_rubric(
        &b64,
        question,
        opts.model
            .as_deref()
            .map_or(DEFAULT_CODEX_ACP_MODEL, |model| model),
        opts.effort
            .as_deref()
            .map_or(DEFAULT_CODEX_ACP_REASONING_EFFORT, |effort| effort),
        opts.system_prompt
            .as_deref()
            .map_or(DEFAULT_SYSTEM_PROMPT, |system_prompt| system_prompt),
        &config,
    )?;

    parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })
}

/// Runs the rubric step: either an HTTP call to a text model or ACP.
///
/// When `config.url` is set, posts `rubric_prompt` to an OpenAI-compatible
/// text model.  Otherwise spawns an ACP child process.
#[cfg(feature = "pipeline")]
fn run_rubric_prompt(rubric_prompt: &str, config: &RubricRunConfig) -> Result<String, RubricError> {
    #[cfg(feature = "http-rubric")]
    if let Some(ref url) = config.url {
        let api_config = VisionApiConfig {
            url: url.clone(),
            model: config.api_model.clone().unwrap_or_default(),
            api_key: None,
        };
        return vision::call_text_api(rubric_prompt, &api_config).map_err(RubricError::Pool);
    }
    #[cfg(feature = "acp")]
    {
        let mut acp = AcpClient::spawn(
            &config.codex_acp_binary,
            &config.acp_args,
            &config.extra_env,
            config.cwd.as_deref(),
        )
        .map_err(RubricError::Pool)?;
        acp.start_session(config.cwd.as_deref())
            .map_err(RubricError::Pool)?;
        acp.prompt_text(rubric_prompt).map_err(RubricError::Pool)
    }
    #[cfg(not(feature = "acp"))]
    Err(RubricError::Pool(PoolError::Spawn(
        "no ACP backend enabled; enable 'acp' feature".to_string(),
    )))
}

/// Two-stage pipeline evaluation: vision model → rubric model.
///
/// Stage 1: Sends the screenshot to an OpenAI-compatible vision API and
/// returns a structured JSON description.
///
/// Stage 2: When [`RubricRunConfig::url`] is set, sends the structured
/// description (plus the rubric question) to an OpenAI-compatible text
/// model.  Otherwise falls back to the configured ACP backend.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, vision API, ACP, or verdict parsing
/// failures.
#[cfg(feature = "pipeline")]
pub fn evaluate_image_rubric_pipeline(
    png_path: &Path,
    question: &str,
    vision_config: &VisionApiConfig,
    vision_prompt: &str,
    rubric_options: &RubricOptions,
    rubric_config: &RubricRunConfig,
) -> Result<RubricVerdict, RubricError> {
    let bytes = std::fs::read(png_path).map_err(|source| RubricError::ReadPng {
        path: png_path.to_path_buf(),
        source,
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let structured =
        vision::call_vision_api(&b64, vision_prompt, vision_config).map_err(RubricError::Pool)?;

    let system_prompt = rubric_options
        .system_prompt
        .as_deref()
        .map_or(DEFAULT_SYSTEM_PROMPT, |system_prompt| system_prompt);
    let rubric_prompt =
        format!("{system_prompt}\n\nUI description:\n{structured}\n\nQuestion: {question}");

    let text = run_rubric_prompt(&rubric_prompt, rubric_config)?;

    parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })
}

/// Like [`evaluate_image_rubric_pipeline`] but also returns the vision
/// model's text description alongside the rubric verdict.
///
/// The first element of the tuple is the rubric verdict, the second is
/// the structured UI description from the vision model.
///
/// # Errors
///
/// See [`evaluate_image_rubric_pipeline`].
#[cfg(feature = "pipeline")]
pub fn evaluate_image_rubric_pipeline_with_vision(
    png_path: &Path,
    question: &str,
    vision_config: &VisionApiConfig,
    vision_prompt: &str,
    rubric_options: &RubricOptions,
    rubric_config: &RubricRunConfig,
) -> Result<(RubricVerdict, String), RubricError> {
    let bytes = std::fs::read(png_path).map_err(|source| RubricError::ReadPng {
        path: png_path.to_path_buf(),
        source,
    })?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let structured =
        vision::call_vision_api(&b64, vision_prompt, vision_config).map_err(RubricError::Pool)?;

    let system_prompt = rubric_options
        .system_prompt
        .as_deref()
        .map_or(DEFAULT_SYSTEM_PROMPT, |system_prompt| system_prompt);
    let rubric_prompt =
        format!("{system_prompt}\n\nUI description:\n{structured}\n\nQuestion: {question}");

    let text = run_rubric_prompt(&rubric_prompt, rubric_config)?;

    let verdict =
        parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })?;
    Ok((verdict, structured))
}

/// Runs the CLI command.
///
/// # Errors
///
/// Returns command parsing, IO, ACP, or audit failures as [`anyhow::Error`].
pub fn run(cli: Cli) -> anyhow::Result<()> {
    cli::run(cli)
}

#[cfg(feature = "codex-acp")]
fn run_codex_acp_rubric(
    b64_png: &str,
    question: &str,
    model: &str,
    effort: &str,
    system_prompt: &str,
    config: &RubricRunConfig,
) -> Result<String, PoolError> {
    let args = effective_acp_args(config, model, effort);
    let mut acp = AcpClient::spawn(
        &config.codex_acp_binary,
        args.as_slice(),
        &config.extra_env,
        config.cwd.as_deref(),
    )?;
    acp.start_session(config.cwd.as_deref())?;

    let prompt = format!("{system_prompt}\n\nQuestion: {question}");
    acp.prompt_image(&prompt, b64_png)
}

#[cfg(feature = "codex-acp")]
fn effective_acp_args(config: &RubricRunConfig, model: &str, effort: &str) -> Vec<String> {
    if config.acp_args
        == build_codex_acp_args(DEFAULT_CODEX_ACP_MODEL, DEFAULT_CODEX_ACP_REASONING_EFFORT)
    {
        build_codex_acp_args(model, effort)
    } else {
        config.acp_args.clone()
    }
}

#[cfg(test)]
mod tests;
