//! Evaluation entry points backed by `visual-rubric` TOML configuration.

use std::path::Path;

#[cfg(any(not(feature = "pipeline"), not(feature = "codex-acp")))]
use crate::PoolError;
#[cfg(any(feature = "codex-acp", feature = "pipeline"))]
use crate::RubricRunConfig;
#[cfg(feature = "codex-acp")]
use crate::evaluate_image_rubric_with_config;
#[cfg(any(feature = "vision-api", feature = "http-rubric"))]
use crate::vision::VisionApiConfig;
use crate::{ConfigMode, RubricError, RubricOptions, RubricVerdict, load_config_toml};
#[cfg(feature = "pipeline")]
use crate::{
    DEFAULT_VISION_PROMPT, evaluate_image_rubric_pipeline,
    evaluate_image_rubric_pipeline_with_vision,
};

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
