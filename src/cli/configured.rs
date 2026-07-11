//! `configured` subcommand — applies the rubric using a TOML config file
//! written by the Home Manager module.
//!
//! The default config path is `~/.config/visual-rubric/config.toml`.
//! CLI flags override individual TOML fields for one-off testing.

use std::path::PathBuf;

#[cfg(any(feature = "codex-acp", feature = "pipeline"))]
use anyhow::Context as _;
use anyhow::{Result, anyhow};

#[cfg(feature = "pipeline")]
use crate::vision::VisionApiConfig;
use crate::{ConfigMode, load_config_toml};
#[cfg(feature = "codex-acp")]
use crate::{DEFAULT_CODEX_ACP_MODEL, DEFAULT_CODEX_ACP_REASONING_EFFORT};

use super::QuestionSource;

#[cfg(feature = "codex-acp")]
const DEFAULT_DIRECT_MODEL: &str = DEFAULT_CODEX_ACP_MODEL;
#[cfg(feature = "codex-acp")]
const DEFAULT_DIRECT_EFFORT: &str = DEFAULT_CODEX_ACP_REASONING_EFFORT;
#[cfg(feature = "pipeline")]
const DEFAULT_PIPELINE_VISION_MODEL: &str = "qwen3-vl-8b";

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
    /// Backend mode: direct codex-acp image evaluation or two-stage pipeline.
    #[arg(long, value_enum)]
    pub mode: Option<ConfigMode>,

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

/// Runs the configured pipeline using TOML config + CLI overrides.
///
/// # Errors
///
/// Returns errors from the vision API, ACP, or verdict parsing.
#[cfg_attr(
    not(any(feature = "codex-acp", feature = "pipeline")),
    allow(unreachable_code, dead_code, unused_variables)
)]
pub fn run_configured(args: ConfiguredArgs) -> Result<()> {
    let question = args.questions.resolve().map_err(|e| anyhow!(e))?;
    let toml = load_config_toml(args.config.as_deref())?;
    let mode = args.mode.or(toml.mode).unwrap_or_default();

    let system_prompt = match args.system_prompt.or(toml.rubric.system_prompt) {
        Some(prompt) => Some(prompt),
        None => args
            .questions
            .resolve_system_prompt()
            .map_err(|e| anyhow!(e))?,
    };
    let rubric_options = rubric_options_for_mode(
        mode,
        args.model,
        toml.rubric.model,
        args.effort,
        toml.rubric.effort,
        system_prompt,
    );

    let verdict = match mode {
        ConfigMode::Direct => {
            #[cfg(feature = "codex-acp")]
            {
                let rubric_config = direct_rubric_config(args.acp_binary, toml.rubric.backend);
                crate::evaluate_image_rubric_with_config(
                    &args.image,
                    &question,
                    rubric_options,
                    rubric_config,
                )
                .with_context(|| format!("direct rubric for {} failed", args.image.display()))?
            }
            #[cfg(not(feature = "codex-acp"))]
            {
                anyhow::bail!("direct mode requires the 'codex-acp' feature")
            }
        }
        ConfigMode::Pipeline => {
            #[cfg(feature = "pipeline")]
            {
                let vision_url = args
                    .vision_url
                    .or(toml.vision.url)
                    .context("vision URL is not set. Set it in config.toml or pass --vision-url")?;

                let vision_model = args
                    .vision_model
                    .or(toml.vision.model)
                    .unwrap_or_else(|| DEFAULT_PIPELINE_VISION_MODEL.to_string());

                let vision_config = VisionApiConfig {
                    url: vision_url,
                    model: vision_model,
                    api_key: args.vision_api_key.or(toml.vision.api_key),
                };

                let vision_prompt = args
                    .vision_prompt
                    .or(toml.vision.prompt)
                    .unwrap_or_else(|| crate::DEFAULT_VISION_PROMPT.to_string());

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
                    url: None,
                    api_model: None,
                    extra_env: Vec::new(),
                    cwd: None,
                };

                crate::evaluate_image_rubric_pipeline(
                    &args.image,
                    &question,
                    &vision_config,
                    &vision_prompt,
                    &rubric_options,
                    &rubric_config,
                )
                .with_context(|| format!("pipeline for {} failed", args.image.display()))?
            }
            #[cfg(not(feature = "pipeline"))]
            {
                anyhow::bail!("pipeline mode requires the 'pipeline' feature")
            }
        }
    };

    if args.json {
        println!("{}", serde_json::to_string(&verdict)?);
        return Ok(());
    }

    crate::assert_verdict(&args.name, verdict)
        .map(|()| println!("visual rubric passed"))
        .map_err(|error| anyhow::anyhow!(error))
}

fn rubric_options_for_mode(
    mode: ConfigMode,
    cli_model: Option<String>,
    toml_model: Option<String>,
    cli_effort: Option<String>,
    toml_effort: Option<String>,
    system_prompt: Option<String>,
) -> crate::RubricOptions {
    let (default_model, default_effort) = match mode {
        #[cfg(feature = "codex-acp")]
        ConfigMode::Direct => (Some(DEFAULT_DIRECT_MODEL), Some(DEFAULT_DIRECT_EFFORT)),
        ConfigMode::Pipeline => (None, None),
        #[cfg(not(feature = "codex-acp"))]
        ConfigMode::Direct => (None, None),
    };
    crate::RubricOptions {
        model: cli_model
            .or(toml_model)
            .or_else(|| default_model.map(str::to_string)),
        effort: cli_effort
            .or(toml_effort)
            .or_else(|| default_effort.map(str::to_string))
            .map(Into::into),
        system_prompt,
    }
}

#[cfg(feature = "codex-acp")]
fn direct_rubric_config(
    cli_binary: Option<String>,
    toml_binary: Option<String>,
) -> crate::RubricRunConfig {
    crate::RubricRunConfig {
        codex_acp_binary: cli_binary
            .or(toml_binary)
            .unwrap_or_else(|| "codex-acp".to_string())
            .into(),
        acp_args: Vec::new(),
        url: None,
        api_model: None,
        extra_env: Vec::new(),
        cwd: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TomlConfig;

    #[cfg(feature = "codex-acp")]
    #[test]
    fn parses_direct_mode_without_vision_config() {
        let config: TomlConfig = toml::from_str(
            r#"
mode = "direct"

[rubric]
backend = "codex-acp"
"#,
        )
        .unwrap();

        assert_eq!(config.mode, Some(ConfigMode::Direct));
        assert!(config.vision.url.is_none());
    }

    #[cfg(feature = "codex-acp")]
    #[test]
    fn direct_mode_defaults_to_gpt55_medium_codex_acp() {
        let options = rubric_options_for_mode(ConfigMode::Direct, None, None, None, None, None);
        let config = direct_rubric_config(None, None);

        assert_eq!(options.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(options.effort.as_deref(), Some("medium"));
        assert_eq!(config.codex_acp_binary, PathBuf::from("codex-acp"));
        assert!(config.acp_args.is_empty());
    }

    #[test]
    fn pipeline_mode_does_not_add_direct_model_defaults() {
        let options = rubric_options_for_mode(ConfigMode::Pipeline, None, None, None, None, None);

        assert!(options.model.is_none());
        assert!(options.effort.is_none());
    }

    #[cfg(feature = "codex-acp")]
    #[test]
    fn rubric_options_prefer_cli_then_toml_then_direct_defaults() {
        let options = rubric_options_for_mode(
            ConfigMode::Direct,
            Some("cli-model".to_string()),
            Some("toml-model".to_string()),
            Some("high".to_string()),
            Some("low".to_string()),
            Some("Use the exact rubric.".to_string()),
        );

        assert_eq!(options.model.as_deref(), Some("cli-model"));
        assert_eq!(options.effort.as_deref(), Some("high"));
        assert_eq!(
            options.system_prompt.as_deref(),
            Some("Use the exact rubric.")
        );

        let options = rubric_options_for_mode(
            ConfigMode::Direct,
            None,
            Some("toml-model".to_string()),
            None,
            Some("low".to_string()),
            None,
        );

        assert_eq!(options.model.as_deref(), Some("toml-model"));
        assert_eq!(options.effort.as_deref(), Some("low"));
    }

    #[test]
    fn pipeline_options_use_cli_or_toml_without_direct_defaults() {
        let options = rubric_options_for_mode(
            ConfigMode::Pipeline,
            None,
            Some("rubric-model".to_string()),
            None,
            Some("medium".to_string()),
            None,
        );

        assert_eq!(options.model.as_deref(), Some("rubric-model"));
        assert_eq!(options.effort.as_deref(), Some("medium"));

        let options = rubric_options_for_mode(
            ConfigMode::Pipeline,
            Some("cli-model".to_string()),
            Some("toml-model".to_string()),
            Some("high".to_string()),
            Some("low".to_string()),
            None,
        );

        assert_eq!(options.model.as_deref(), Some("cli-model"));
        assert_eq!(options.effort.as_deref(), Some("high"));
    }

    #[cfg(feature = "codex-acp")]
    #[test]
    fn direct_rubric_config_prefers_cli_then_toml_then_default() {
        let config = direct_rubric_config(
            Some("cli-codex-acp".to_string()),
            Some("toml-codex-acp".to_string()),
        );
        assert_eq!(config.codex_acp_binary, PathBuf::from("cli-codex-acp"));
        assert!(config.acp_args.is_empty());

        let config = direct_rubric_config(None, Some("toml-codex-acp".to_string()));
        assert_eq!(config.codex_acp_binary, PathBuf::from("toml-codex-acp"));
        assert!(config.acp_args.is_empty());

        let config = direct_rubric_config(None, None);
        assert_eq!(config.codex_acp_binary, PathBuf::from("codex-acp"));
        assert!(config.acp_args.is_empty());
    }

    #[test]
    fn parses_full_configured_toml_schema() {
        let config: TomlConfig = toml::from_str(
            r#"
mode = "pipeline"

[vision]
url = "http://localhost:8013"
model = "qwen3-vl-8b"
api_key = "secret"
prompt = "Describe this UI."

[rubric]
backend = "opencode"
args = ["acp", "--debug"]
model = "deepseek-v4"
effort = "high"
system_prompt = "Return strict rubric JSON."
"#,
        )
        .unwrap();

        assert_eq!(config.mode, Some(ConfigMode::Pipeline));
        assert_eq!(config.vision.url.as_deref(), Some("http://localhost:8013"));
        assert_eq!(config.vision.model.as_deref(), Some("qwen3-vl-8b"));
        assert_eq!(config.vision.api_key.as_deref(), Some("secret"));
        assert_eq!(config.vision.prompt.as_deref(), Some("Describe this UI."));
        assert_eq!(config.rubric.backend.as_deref(), Some("opencode"));
        assert_eq!(
            config.rubric.args.as_deref(),
            Some(["acp".to_string(), "--debug".to_string()].as_slice())
        );
        assert_eq!(config.rubric.model.as_deref(), Some("deepseek-v4"));
        assert_eq!(config.rubric.effort.as_deref(), Some("high"));
        assert_eq!(
            config.rubric.system_prompt.as_deref(),
            Some("Return strict rubric JSON.")
        );
    }

    #[cfg(feature = "pipeline")]
    #[test]
    fn pipeline_mode_requires_vision_url() {
        let temp = tempfile::TempDir::new().unwrap();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "mode = \"pipeline\"\n").unwrap();

        let err = run_configured(ConfiguredArgs {
            image: temp.path().join("missing.png"),
            questions: QuestionSource::from_question("Does it render?".to_string()),
            name: "screenshot".to_string(),
            json: false,
            config: Some(config_path),
            mode: None,
            vision_url: None,
            vision_model: None,
            vision_api_key: None,
            vision_prompt: None,
            acp_binary: None,
            acp_args: Vec::new(),
            model: None,
            effort: None,
            system_prompt: None,
        })
        .unwrap_err();

        assert!(err.to_string().contains("vision URL is not set"));
    }

    #[test]
    fn invalid_mode_is_rejected_by_toml_parser() {
        let err = toml::from_str::<TomlConfig>("mode = \"gpt55\"").unwrap_err();

        assert!(err.to_string().contains("mode"));
    }
}
