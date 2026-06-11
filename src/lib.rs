//! Shared AI visual-rubric runner for screenshot review.
//!
//! This crate owns the Codex ACP plumbing so browser screenshots, offscreen
//! renderer captures, and VM/VNC screenshots can use one rubric path.
//!
//! It also provides a two-stage pipeline: vision model extraction via an
//! OpenAI-compatible HTTP API, then rubric scoring via ACP.
#![warn(missing_docs)]

mod acp;
mod batch;
pub mod cli;
mod errors;
mod pool;
pub mod presets;
mod typed_strings;
pub mod vision;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use acp::AcpClient;
use vision::VisionApiConfig;

pub use acp::build_codex_acp_args;
pub use batch::{
    AggregateStatus, AssetChange, AssetRubricReport, AssetRubricResult, AssetSnapshot,
    BatchRubricConfig, BatchRubricReport, BatchRubricRun, IssueClassificationInput,
    IssueClassifier, IssueRecommendation, RecommendationSeverity, SelectionMode, diff_snapshots,
    select_changed,
};
pub use cli::Cli;
pub use errors::{PoolError, RateLimitEvent, RubricError};
pub use pool::{LogCaptureConfig, LogPathMode, PoolConfig, PoolStats, RubricPool};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};

#[derive(Debug, Deserialize, Serialize)]
/// Parsed rubric verdict returned by ACP.
pub struct RubricVerdict {
    /// Machine-readable pass/fail status.
    pub verdict: RubricVerdictStatus,
    /// Human-readable reason for the verdict.
    pub reason: String,
    /// Optional anomalies observed in the screenshot.
    #[serde(default, deserialize_with = "deserialize_anomalies")]
    pub anomalies: Vec<String>,
}

/// Optional model settings for one rubric request.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RubricOptions {
    /// ACP model override.
    pub model: Option<String>,
    /// Reasoning effort override.
    pub effort: Option<RubricEffort>,
    /// System prompt override.
    pub system_prompt: Option<String>,
}

/// Runtime configuration for direct ACP calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RubricRunConfig {
    /// Path to the ACP binary (e.g. `codex-acp` or `opencode`).
    pub codex_acp_binary: PathBuf,
    /// Extra CLI arguments for the ACP binary.
    /// Defaults to `["-c", "model=...", "-c", "model_reasoning_effort=..."]`
    /// for codex-acp. For opencode use `["acp"]`.
    pub acp_args: Vec<String>,
    /// Extra environment variables for the child process.
    pub extra_env: Vec<(OsString, OsString)>,
    /// Working directory passed to ACP.
    pub cwd: Option<PathBuf>,
}

impl Default for RubricRunConfig {
    fn default() -> Self {
        Self {
            codex_acp_binary: default_codex_acp_binary(),
            acp_args: build_codex_acp_args(
                DEFAULT_CODEX_ACP_MODEL,
                DEFAULT_CODEX_ACP_REASONING_EFFORT,
            ),
            extra_env: Vec::new(),
            cwd: None,
        }
    }
}

/// Default system prompt used for screenshot rubric requests.
///
/// Shared with the `ui-regression` question preset.
pub const DEFAULT_SYSTEM_PROMPT: &str = presets::UI_REGRESSION_SYSTEM_PROMPT;

/// Default Codex ACP model.
pub const DEFAULT_CODEX_ACP_MODEL: &str = "gpt-5.4-mini";
/// Default Codex ACP reasoning effort.
pub const DEFAULT_CODEX_ACP_REASONING_EFFORT: &str = "medium";

/// Default prompt for the vision extraction stage.
///
/// Asks the vision model to describe the screenshot as structured JSON
/// so a text-only rubric model (e.g. DeepSeek V4 via opencode) can score it.
pub const DEFAULT_VISION_PROMPT: &str = "\
You are a UI description engine. Given a screenshot, produce a structured JSON \
description of all visible user interface elements, their text content, layout, \
and any visual issues (clipping, overlap, blank regions, contrast problems). \
Output ONLY valid JSON with no additional text.";

/// Returns the default rubric options.
#[must_use]
pub fn default_options() -> RubricOptions {
    RubricOptions {
        model: Some(DEFAULT_CODEX_ACP_MODEL.to_string()),
        effort: Some(DEFAULT_CODEX_ACP_REASONING_EFFORT.into()),
        system_prompt: Some(DEFAULT_SYSTEM_PROMPT.to_string()),
    }
}

/// Returns the default Codex ACP executable name.
#[must_use]
pub fn default_codex_acp_binary() -> PathBuf {
    PathBuf::from("codex-acp")
}

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
pub fn assert_image_rubric(png_path: &Path, name: &str, question: &str) -> Result<(), RubricError> {
    let verdict = evaluate_image_rubric(png_path, question)?;
    assert_verdict(name, verdict)
}

/// Evaluates a PNG with default options.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, or verdict parsing failures.
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

/// Two-stage pipeline evaluation: vision model → rubric model.
///
/// Stage 1: Sends the screenshot to an OpenAI-compatible vision API and
/// returns a structured JSON description.
///
/// Stage 2: Sends the structured description (plus the rubric question) to
/// the configured ACP backend for the final rubric verdict.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, vision API, ACP, or verdict parsing
/// failures.
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

    let mut acp = AcpClient::spawn(
        &rubric_config.codex_acp_binary,
        &rubric_config.acp_args,
        &rubric_config.extra_env,
        rubric_config.cwd.as_deref(),
    )
    .map_err(RubricError::Pool)?;
    acp.start_session(rubric_config.cwd.as_deref())
        .map_err(RubricError::Pool)?;

    let text = acp.prompt_text(&rubric_prompt).map_err(RubricError::Pool)?;

    parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })
}

/// Parses strict rubric JSON into a typed verdict.
///
/// # Errors
///
/// Returns the underlying JSON error when the text is malformed or contains an
/// unsupported verdict status.
pub fn parse_verdict(text: &str) -> Result<RubricVerdict, serde_json::Error> {
    match serde_json::from_str(text) {
        Ok(verdict) => Ok(verdict),
        Err(source) => match extract_json_object(text) {
            Some(json) => serde_json::from_str(json),
            None => Err(source),
        },
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + character.len_utf8();
                    return Some(&text[start..end]);
                }
            }
            _ => {}
        }
    }

    None
}

fn deserialize_anomalies<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values.into_iter().map(anomaly_to_string).collect())
}

fn anomaly_to_string(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text,
        serde_json::Value::Object(mut object) => {
            let issue = object
                .remove("issue")
                .and_then(|value| value.as_str().map(str::to_owned));
            let fix = object
                .remove("fix")
                .and_then(|value| value.as_str().map(str::to_owned));
            match (issue, fix) {
                (Some(issue), Some(fix)) => format!("{issue} Fix: {fix}"),
                (Some(issue), None) => issue,
                (None, Some(fix)) => fix,
                (None, None) => serde_json::Value::Object(object).to_string(),
            }
        }
        other => other.to_string(),
    }
}

/// Converts a verdict into an assertion-style result.
///
/// # Errors
///
/// Returns [`RubricError::Assertion`] when the verdict is not pass.
pub fn assert_verdict(name: &str, verdict: RubricVerdict) -> Result<(), RubricError> {
    if verdict.verdict.is_pass() {
        Ok(())
    } else {
        Err(RubricError::Assertion {
            name: name.to_string(),
            reason: verdict.reason,
            anomalies: verdict.anomalies,
        })
    }
}

/// Runs the CLI command.
///
/// # Errors
///
/// Returns command parsing, IO, ACP, or audit failures as [`anyhow::Error`].
pub fn run(cli: Cli) -> anyhow::Result<()> {
    cli::run(cli)
}

fn run_codex_acp_rubric(
    b64_png: &str,
    question: &str,
    model: &str,
    effort: &str,
    system_prompt: &str,
    config: &RubricRunConfig,
) -> Result<String, PoolError> {
    let args = acp::build_codex_acp_args(model, effort);
    let mut acp = AcpClient::spawn(
        &config.codex_acp_binary,
        &args,
        &config.extra_env,
        config.cwd.as_deref(),
    )?;
    acp.start_session(config.cwd.as_deref())?;

    let prompt = format!("{system_prompt}\n\nQuestion: {question}");
    acp.prompt_image(&prompt, b64_png)
}

#[cfg(test)]
mod tests;
