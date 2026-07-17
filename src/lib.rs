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
pub mod coverage;
mod errors;
pub mod manifest;
#[cfg(feature = "pool")]
mod pool;
pub mod presets;
pub mod qa;
mod report;
mod typed_strings;
mod verdict;
#[cfg(any(feature = "vision-api", feature = "http-rubric"))]
pub mod vision;

use std::path::{Path, PathBuf};

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
    ConfigMode, RubricOptions, RubricRunConfig, TomlConfig, TomlRubric, TomlSequence, TomlVision,
    load_config_toml,
};
#[cfg(feature = "codex-acp")]
pub use config::{default_codex_acp_binary, default_options, direct_codex_gpt_config};
pub use configured_eval::{evaluate_configured, evaluate_configured_with_vision};
pub use coverage::{
    COVERAGE_CONTRACT_SCHEMA_VERSION, COVERAGE_REPORT_SCHEMA_VERSION, CoverageContractV1,
    CoverageExclusionV1, CoverageReportV1, CoverageSurfaceV1, CoverageTransitionV1,
};
pub use errors::{PoolError, RateLimitEvent, RubricError};
pub use manifest::{
    ArtifactDigest, CAPTURE_MANIFEST_SCHEMA_VERSION, CaptureCell, CaptureEnvironment,
    CaptureManifest, ManifestError, Viewport,
};
#[cfg(feature = "pool")]
pub use pool::{LogCaptureConfig, LogPathMode, PoolConfig, PoolStats, RubricPool};
pub use qa::{
    CALIBRATION_SENTINEL_SCHEMA_VERSION, CalibrationExpectation, CalibrationSentinelV1,
    FINDING_SCHEMA_VERSION, FindingV1, OBSERVATION_SCHEMA_VERSION, ObservationV1,
    VISUAL_RUN_REPORT_SCHEMA_VERSION, VisualRunReportV1, VisualRunStatus, finding_fingerprint,
    validate_calibration_sentinels, validate_visual_run, validate_visual_run_against_manifest,
};
pub use report::{PageResult, RubricReport};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};
pub use verdict::{RubricVerdict, assert_verdict, parse_verdict};

/// Default system prompt used for screenshot rubric requests.
///
/// Shared with the `ui-regression` question preset.
pub const DEFAULT_SYSTEM_PROMPT: &str = presets::UI_REGRESSION_SYSTEM_PROMPT;

/// One ordered screenshot checkpoint in an interaction journey.
#[cfg(any(feature = "codex-acp", feature = "pipeline"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceFrame {
    /// Stable checkpoint label used in the rubric prompt and evidence.
    pub label: String,
    /// PNG artifact containing the checkpoint frame.
    pub path: PathBuf,
}

/// Policy applied to one ordered screenshot sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceOptions {
    /// Maximum number of checkpoints accepted in one request.
    pub max_frames: usize,
    /// Whether the rubric must assess the semantic transition between
    /// adjacent checkpoints. A one-frame sequence never has a transition.
    pub require_transition: bool,
}

impl Default for SequenceOptions {
    fn default() -> Self {
        Self {
            max_frames: 8,
            require_transition: true,
        }
    }
}

impl SequenceOptions {
    /// Returns an error when the policy cannot provide a bounded request.
    fn validate(self) -> Result<(), String> {
        if self.max_frames == 0 {
            return Err("sequence max_frames must be greater than zero".to_owned());
        }
        if self.max_frames > 32 {
            return Err("sequence max_frames must not exceed 32".to_owned());
        }
        Ok(())
    }
}

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

/// Evaluates an ordered screenshot sequence as one interaction journey.
///
/// The evaluator receives every checkpoint in order, including labels, so it
/// can verify both the visible state at each point and the before/after
/// transition implied by the sequence. A single-frame rubric cannot prove
/// that an interaction actually changed the UI.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, ACP, or verdict parsing failures.
#[cfg(feature = "codex-acp")]
pub fn evaluate_image_sequence_rubric_with_config(
    frames: &[SequenceFrame],
    question: &str,
    opts: RubricOptions,
    config: RubricRunConfig,
) -> Result<RubricVerdict, RubricError> {
    evaluate_image_sequence_rubric_with_options(
        frames,
        question,
        opts,
        config,
        SequenceOptions::default(),
    )
}

/// Evaluates an ordered screenshot sequence with an explicit checkpoint
/// policy and ACP runtime configuration.
#[cfg(feature = "codex-acp")]
pub fn evaluate_image_sequence_rubric_with_options(
    frames: &[SequenceFrame],
    question: &str,
    opts: RubricOptions,
    config: RubricRunConfig,
    sequence_options: SequenceOptions,
) -> Result<RubricVerdict, RubricError> {
    sequence_options
        .validate()
        .map_err(PoolError::Rpc)
        .map_err(RubricError::Pool)?;
    if frames.is_empty() {
        return Err(RubricError::Pool(PoolError::Rpc(
            "sequence rubric requires at least one frame".to_owned(),
        )));
    }
    if sequence_options.require_transition && frames.len() < 2 {
        return Err(RubricError::Pool(PoolError::Rpc(
            "sequence transition assessment requires at least two frames".to_owned(),
        )));
    }
    let mut encoded = Vec::with_capacity(frames.len());
    for frame in frames {
        let bytes = std::fs::read(&frame.path).map_err(|source| RubricError::ReadPng {
            path: frame.path.clone(),
            source,
        })?;
        encoded.push((
            frame.label.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ));
    }
    if frames.len() > sequence_options.max_frames {
        return Err(RubricError::Pool(PoolError::Rpc(format!(
            "sequence contains {} frames, maximum is {}",
            frames.len(),
            sequence_options.max_frames
        ))));
    }
    let transition_instruction = if sequence_options.require_transition {
        "Check every checkpoint and whether each before/after transition is visible and semantically correct."
    } else {
        "Check every checkpoint for visual and semantic correctness. Transition assessment is disabled."
    };
    let prompt = format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\nEvaluate this ordered interaction sequence. {transition_instruction}\nQuestion: {question}"
    );
    let text = run_codex_acp_sequence(
        &encoded,
        &prompt,
        opts.model
            .as_deref()
            .map_or(DEFAULT_CODEX_ACP_MODEL, |model| model),
        opts.effort
            .as_deref()
            .map_or(DEFAULT_CODEX_ACP_REASONING_EFFORT, |effort| effort),
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

/// Two-stage evaluation for an ordered interaction sequence.
///
/// The vision stage receives all labelled frames and the rubric stage receives
/// the resulting ordered description, preserving transition context in both
/// supported backends.
#[cfg(feature = "pipeline")]
pub fn evaluate_image_sequence_rubric_pipeline(
    frames: &[SequenceFrame],
    question: &str,
    vision_config: &VisionApiConfig,
    vision_prompt: &str,
    rubric_options: &RubricOptions,
    rubric_config: &RubricRunConfig,
) -> Result<RubricVerdict, RubricError> {
    evaluate_image_sequence_rubric_pipeline_with_options(
        frames,
        question,
        vision_config,
        vision_prompt,
        rubric_options,
        rubric_config,
        SequenceOptions::default(),
    )
}

/// Two-stage sequence evaluation with an explicit checkpoint policy.
#[cfg(feature = "pipeline")]
pub fn evaluate_image_sequence_rubric_pipeline_with_options(
    frames: &[SequenceFrame],
    question: &str,
    vision_config: &VisionApiConfig,
    vision_prompt: &str,
    rubric_options: &RubricOptions,
    rubric_config: &RubricRunConfig,
    sequence_options: SequenceOptions,
) -> Result<RubricVerdict, RubricError> {
    sequence_options
        .validate()
        .map_err(PoolError::VisionApi)
        .map_err(RubricError::Pool)?;
    if frames.is_empty() {
        return Err(RubricError::Pool(PoolError::VisionApi(
            "sequence rubric requires at least one frame".to_owned(),
        )));
    }
    if sequence_options.require_transition && frames.len() < 2 {
        return Err(RubricError::Pool(PoolError::VisionApi(
            "sequence transition assessment requires at least two frames".to_owned(),
        )));
    }
    if frames.len() > sequence_options.max_frames {
        return Err(RubricError::Pool(PoolError::VisionApi(format!(
            "sequence contains {} frames, maximum is {}",
            frames.len(),
            sequence_options.max_frames
        ))));
    }
    let mut encoded = Vec::with_capacity(frames.len());
    for frame in frames {
        let bytes = std::fs::read(&frame.path).map_err(|source| RubricError::ReadPng {
            path: frame.path.clone(),
            source,
        })?;
        encoded.push((
            frame.label.clone(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ));
    }
    let structured = vision::call_vision_api_sequence(&encoded, vision_prompt, vision_config)
        .map_err(RubricError::Pool)?;
    let system_prompt = rubric_options
        .system_prompt
        .as_deref()
        .map_or(DEFAULT_SYSTEM_PROMPT, |prompt| prompt);
    let transition_instruction = if sequence_options.require_transition {
        "Assess the semantic before/after transition between adjacent checkpoints."
    } else {
        "Assess each checkpoint independently; transition assessment is disabled."
    };
    let rubric_prompt = format!(
        "{system_prompt}\n\nOrdered UI journey description:\n{structured}\n\n{transition_instruction}\nQuestion: {question}"
    );
    let text = run_rubric_prompt(&rubric_prompt, rubric_config)?;
    parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })
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
fn run_codex_acp_sequence(
    frames: &[(String, String)],
    prompt: &str,
    model: &str,
    effort: &str,
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
    acp.prompt_images(prompt, frames)
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
