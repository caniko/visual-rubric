//! Shared AI visual-rubric runner for screenshot review.
//!
//! This crate owns the Codex ACP plumbing so browser screenshots, offscreen
//! renderer captures, and VM/VNC screenshots can use one rubric path.
//!
//! It also provides a two-stage pipeline: vision model extraction via an
//! OpenAI-compatible HTTP API, then rubric scoring via ACP.
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
mod errors;
#[cfg(feature = "pool")]
mod pool;
pub mod presets;
mod report;
mod typed_strings;
#[cfg(any(feature = "vision-api", feature = "http-rubric"))]
pub mod vision;

use std::path::Path;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[cfg(feature = "acp")]
use acp::AcpClient;
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
pub use config::{default_codex_acp_binary, default_options};
pub use errors::{PoolError, RateLimitEvent, RubricError};
#[cfg(feature = "pool")]
pub use pool::{LogCaptureConfig, LogPathMode, PoolConfig, PoolStats, RubricPool};
pub use report::{PageResult, RubricReport};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};

/// Parsed rubric verdict returned by ACP.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RubricVerdict {
    /// Machine-readable pass/fail status.
    pub verdict: RubricVerdictStatus,
    /// Human-readable reason for the verdict.
    pub reason: String,
    /// Optional anomalies observed in the screenshot.
    #[serde(default, deserialize_with = "deserialize_anomalies")]
    pub anomalies: Vec<String>,
}

/// Default system prompt used for screenshot rubric requests.
///
/// Shared with the `ui-regression` question preset.
pub const DEFAULT_SYSTEM_PROMPT: &str = presets::UI_REGRESSION_SYSTEM_PROMPT;

/// Default Codex ACP model.
#[cfg(feature = "codex-acp")]
pub const DEFAULT_CODEX_ACP_MODEL: &str = "gpt-5.4-mini";
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

/// Parses strict rubric JSON into a typed verdict.
///
/// Tries the text as raw JSON first, then tries to extract a JSON object
/// from surrounding text (e.g. markdown code blocks), and finally applies
/// a lightweight repair pass for common model-output issues (missing
/// opening quotes on keys, trailing commas before `]`/`}`).
///
/// # Errors
///
/// Returns the underlying JSON error when the text is malformed or contains an
/// unsupported verdict status.
pub fn parse_verdict(text: &str) -> Result<RubricVerdict, serde_json::Error> {
    match serde_json::from_str(text) {
        Ok(verdict) => return Ok(verdict),
        Err(_) => {}
    }

    // Try extracting a JSON object from surrounding noise (code blocks, etc.)
    if let Some(json) = extract_json_object(text) {
        if let Ok(verdict) = serde_json::from_str(json) {
            return Ok(verdict);
        }
        // The object was found but the content is slightly malformed — try
        // a lightweight repair pass for common model output quirks.
        let repaired = repair_json(json);
        if let Ok(verdict) = serde_json::from_str(&repaired) {
            return Ok(verdict);
        }
    }

    // Last resort: repair the full text directly.
    let repaired = repair_json(text);
    serde_json::from_str(&repaired)
}

/// Lightweight repair for common JSON formatting issues in model output.
///
/// Handles:
/// 1. Unquoted object keys (`key":` → `"key":`)
/// 2. Trailing commas before `]` or `}`
fn repair_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] as char {
            '"' => {
                out.push('"');
                i += 1;
                while i < len {
                    let ch = bytes[i] as char;
                    out.push(ch);
                    i += 1;
                    if ch == '\\' && i < len {
                        out.push(bytes[i] as char);
                        i += 1;
                    } else if ch == '"' {
                        break;
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < len && ((bytes[i] as char).is_alphanumeric() || bytes[i] as char == '_') {
                    i += 1;
                }
                let word = &text[start..i];
                if i + 1 < len && bytes[i] as char == '"' && bytes[i + 1] as char == ':' {
                    out.push('"');
                    out.push_str(word);
                    out.push('"');
                    out.push(':');
                    i += 2;
                } else {
                    out.push_str(word);
                }
            }
            ',' => {
                let mut j = i + 1;
                while j < len && (bytes[j] as char).is_ascii_whitespace() {
                    j += 1;
                }
                if j < len && matches!(bytes[j] as char, ']' | '}') {
                    out.push(' ');
                    i = j;
                } else {
                    out.push(',');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
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

/// Load configuration from `~/.config/visual-rubric/config.toml` and
/// evaluate `png_path` against `question`.
///
/// In `pipeline` mode (the HM default) this routes through
/// `evaluate_image_rubric_pipeline` so that a vision model extracts the
/// UI description before the rubric model scores it.  In `direct` mode it
/// sends the image straight to ACP via
/// `evaluate_image_rubric_with_config`.
///
/// When the TOML file is missing or unreadable this falls back to
/// `evaluate_image_rubric` (direct `codex-acp` with defaults).
pub fn evaluate_configured(
    png_path: &Path,
    question: &str,
    options: &RubricOptions,
) -> Result<RubricVerdict, RubricError> {
    let toml = match load_config_toml(None) {
        Ok(c) => c,
        Err(_) => {
            return evaluate_fallback(png_path, question, options);
        }
    };
    let mode = toml.mode.unwrap_or_default();

    match mode {
        ConfigMode::Pipeline => {
            #[cfg(feature = "pipeline")]
            {
                let vision_url = toml
                    .vision
                    .url
                    .as_deref()
                    .unwrap_or("http://localhost:8013");
                let vision_model = toml.vision.model.as_deref().unwrap_or("qwen3-vl-8b");
                let vision_config = VisionApiConfig {
                    url: vision_url.to_string(),
                    model: vision_model.to_string(),
                    api_key: toml.vision.api_key.clone(),
                };
                let vision_prompt = toml
                    .vision
                    .prompt
                    .as_deref()
                    .unwrap_or(DEFAULT_VISION_PROMPT);
                let rubric_config = RubricRunConfig::from_config_toml(None);
                evaluate_image_rubric_pipeline(
                    png_path,
                    question,
                    &vision_config,
                    vision_prompt,
                    options,
                    &rubric_config,
                )
            }
            #[cfg(not(feature = "pipeline"))]
            Err(RubricError::Pool(PoolError::Spawn(
                "pipeline mode requires the 'pipeline' feature".to_string(),
            )))
        }
        ConfigMode::Direct => {
            #[cfg(feature = "codex-acp")]
            {
                let rubric_config = RubricRunConfig {
                    codex_acp_binary: toml
                        .rubric
                        .backend
                        .unwrap_or_else(|| "codex-acp".to_string())
                        .into(),
                    ..Default::default()
                };
                evaluate_image_rubric_with_config(
                    png_path,
                    question,
                    options.clone(),
                    rubric_config,
                )
            }
            #[cfg(not(feature = "codex-acp"))]
            Err(RubricError::Pool(PoolError::Spawn(
                "direct mode requires the 'codex-acp' feature".to_string(),
            )))
        }
    }
}

/// Like [`evaluate_configured`] but also returns the vision model's
/// text description alongside the rubric verdict.
///
/// In `pipeline` mode this calls `evaluate_image_rubric_pipeline_with_vision`
/// so the vision description is captured.  In `direct` mode the vision
/// description is empty (the ACP model evaluates the image directly).
pub fn evaluate_configured_with_vision(
    png_path: &Path,
    question: &str,
    options: &RubricOptions,
) -> Result<(RubricVerdict, String), RubricError> {
    let toml = match load_config_toml(None) {
        Ok(c) => c,
        Err(_) => {
            let verdict = evaluate_fallback(png_path, question, options)?;
            return Ok((verdict, String::new()));
        }
    };
    let mode = toml.mode.unwrap_or_default();

    match mode {
        ConfigMode::Pipeline => {
            #[cfg(feature = "pipeline")]
            {
                let vision_url = toml
                    .vision
                    .url
                    .as_deref()
                    .unwrap_or("http://localhost:8013");
                let vision_model = toml.vision.model.as_deref().unwrap_or("qwen3-vl-8b");
                let vision_config = VisionApiConfig {
                    url: vision_url.to_string(),
                    model: vision_model.to_string(),
                    api_key: toml.vision.api_key.clone(),
                };
                let vision_prompt = toml
                    .vision
                    .prompt
                    .as_deref()
                    .unwrap_or(DEFAULT_VISION_PROMPT);
                let rubric_config = RubricRunConfig::from_config_toml(None);
                evaluate_image_rubric_pipeline_with_vision(
                    png_path,
                    question,
                    &vision_config,
                    vision_prompt,
                    options,
                    &rubric_config,
                )
            }
            #[cfg(not(feature = "pipeline"))]
            Err(RubricError::Pool(PoolError::Spawn(
                "pipeline mode requires the 'pipeline' feature".to_string(),
            )))
        }
        ConfigMode::Direct => {
            #[cfg(feature = "codex-acp")]
            {
                let rubric_config = RubricRunConfig {
                    codex_acp_binary: toml
                        .rubric
                        .backend
                        .unwrap_or_else(|| "codex-acp".to_string())
                        .into(),
                    ..Default::default()
                };
                let verdict = evaluate_image_rubric_with_config(
                    png_path,
                    question,
                    options.clone(),
                    rubric_config,
                )?;
                Ok((verdict, String::new()))
            }
            #[cfg(not(feature = "codex-acp"))]
            Err(RubricError::Pool(PoolError::Spawn(
                "direct mode requires the 'codex-acp' feature".to_string(),
            )))
        }
    }
}

fn evaluate_fallback(
    #[allow(unused_variables)] png_path: &Path,
    #[allow(unused_variables)] question: &str,
    #[allow(unused_variables)] options: &RubricOptions,
) -> Result<RubricVerdict, RubricError> {
    #[cfg(feature = "codex-acp")]
    {
        evaluate_image_rubric_with_config(
            png_path,
            question,
            options.clone(),
            RubricRunConfig::default(),
        )
    }
    #[cfg(not(feature = "codex-acp"))]
    {
        Err(RubricError::Pool(PoolError::Spawn(
            "no TOML config found and codex-acp feature is disabled; \
             configure a TOML file or use a direct API call"
                .to_string(),
        )))
    }
}

#[cfg(test)]
mod tests;
