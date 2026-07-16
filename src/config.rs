//! Runtime and TOML configuration for visual rubric evaluation.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::RubricEffort;
#[cfg(feature = "codex-acp")]
use crate::{
    DEFAULT_CODEX_ACP_MODEL, DEFAULT_CODEX_ACP_REASONING_EFFORT, DEFAULT_SYSTEM_PROMPT,
    build_codex_acp_args,
};

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

/// Runtime configuration for rubric evaluation.
///
/// Supports two backends:
/// - **HTTP** (via `url`): calls an OpenAI-compatible text model endpoint.
/// - **ACP** (via `codex_acp_binary`): spawns an ACP child process.
///
/// When `url` is `Some`, the pipeline uses the HTTP backend and ignores ACP
/// fields.  When `url` is `None`, the pipeline falls back to ACP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RubricRunConfig {
    /// Path to the ACP binary (e.g. `codex-acp` or `opencode`).
    pub codex_acp_binary: PathBuf,
    /// Extra CLI arguments for the ACP binary.
    /// Defaults to `["-c", "model=...", "-c", "model_reasoning_effort=..."]`
    /// for codex-acp. For opencode use `["acp"]`.
    pub acp_args: Vec<String>,
    /// HTTP rubric API base URL (e.g. `"http://127.0.0.1:8013"`).
    /// When set, the pipeline calls an OpenAI-compatible text model
    /// directly instead of spawning an ACP child process.
    pub url: Option<String>,
    /// Model name for the HTTP rubric backend (ignored when `url` is `None`).
    pub api_model: Option<String>,
    /// Extra environment variables for the child process.
    pub extra_env: Vec<(OsString, OsString)>,
    /// Working directory passed to ACP.
    pub cwd: Option<PathBuf>,
}

impl Default for RubricRunConfig {
    fn default() -> Self {
        Self {
            #[cfg(feature = "codex-acp")]
            codex_acp_binary: PathBuf::from("codex-acp"),
            #[cfg(not(feature = "codex-acp"))]
            codex_acp_binary: PathBuf::from("opencode"),
            #[cfg(feature = "codex-acp")]
            acp_args: build_codex_acp_args(
                DEFAULT_CODEX_ACP_MODEL,
                DEFAULT_CODEX_ACP_REASONING_EFFORT,
            ),
            #[cfg(not(feature = "codex-acp"))]
            acp_args: vec!["acp".to_string()],
            url: None,
            api_model: None,
            extra_env: Vec::new(),
            cwd: None,
        }
    }
}

impl RubricRunConfig {
    /// Build a `RubricRunConfig` from the Home-Manager-managed TOML file.
    ///
    /// Reads `~/.config/visual-rubric/config.toml` (or an explicit
    /// `path`) and extracts the `[rubric]` section: `backend` becomes
    /// `codex_acp_binary`, `args` becomes `acp_args`.
    ///
    /// In `pipeline` mode the vision config is ignored — this method only
    /// populates the rubric/ACP fields.  When the file is missing or
    /// unreadable, returns the library [`Default`].
    #[must_use]
    pub fn from_config_toml(path: Option<&Path>) -> Self {
        let toml = match load_config_toml(path) {
            Ok(c) => c,
            Err(_) => return RubricRunConfig::default(),
        };
        let mode = toml.mode.unwrap_or_default();
        let default_backend = match mode {
            ConfigMode::Direct => "codex-acp",
            ConfigMode::Pipeline => "opencode",
        };
        let acp_args = match toml.rubric.args {
            Some(args) => args,
            None if mode == ConfigMode::Direct => direct_codex_acp_args(&toml.rubric),
            None => vec!["acp".to_string()],
        };
        RubricRunConfig {
            codex_acp_binary: toml
                .rubric
                .backend
                .unwrap_or_else(|| default_backend.to_string())
                .into(),
            acp_args,
            url: toml.rubric.url,
            api_model: toml.rubric.model.clone(),
            ..Default::default()
        }
    }
}

fn direct_codex_acp_args(rubric: &TomlRubric) -> Vec<String> {
    #[cfg(feature = "codex-acp")]
    {
        build_codex_acp_args(
            rubric.model.as_deref().unwrap_or(DEFAULT_CODEX_ACP_MODEL),
            rubric
                .effort
                .as_deref()
                .unwrap_or(DEFAULT_CODEX_ACP_REASONING_EFFORT),
        )
    }
    #[cfg(not(feature = "codex-acp"))]
    {
        let _ = rubric;
        Vec::new()
    }
}

/// Returns the default rubric options.
#[cfg(feature = "codex-acp")]
#[must_use]
pub fn default_options() -> RubricOptions {
    RubricOptions {
        model: Some(DEFAULT_CODEX_ACP_MODEL.to_string()),
        effort: Some(DEFAULT_CODEX_ACP_REASONING_EFFORT.into()),
        system_prompt: Some(DEFAULT_SYSTEM_PROMPT.to_string()),
    }
}

/// Returns the default Codex ACP executable name.
#[cfg(feature = "codex-acp")]
#[must_use]
pub fn default_codex_acp_binary() -> PathBuf {
    PathBuf::from("codex-acp")
}

/// Backend mode read from `config.toml`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigMode {
    /// Direct screenshot evaluation through codex-acp.
    #[default]
    Direct,
    /// Vision extraction followed by rubric scoring.
    Pipeline,
}

/// Full shape of `~/.config/visual-rubric/config.toml`.
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct TomlConfig {
    /// Top-level mode: `"direct"` or `"pipeline"`.
    pub mode: Option<ConfigMode>,
    /// `[vision]` section (used by pipeline mode only).
    pub vision: TomlVision,
    /// `[rubric]` section — binary, args, model, effort, system prompt.
    pub rubric: TomlRubric,
}

/// `[vision]` section of the TOML config.
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct TomlVision {
    /// Vision API base URL.
    pub url: Option<String>,
    /// Vision model name.
    pub model: Option<String>,
    /// Vision API key (Bearer token).
    pub api_key: Option<String>,
    /// Custom prompt for the vision extraction stage.
    pub prompt: Option<String>,
}

/// `[rubric]` section of the TOML config.
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct TomlRubric {
    /// ACP binary name or path (e.g. `"opencode"` or `"codex-acp"`).
    pub backend: Option<String>,
    /// Extra CLI arguments for the ACP binary (e.g. `["acp"]` for opencode).
    pub args: Option<Vec<String>>,
    /// HTTP rubric API base URL (e.g. `"http://127.0.0.1:8013"`).
    /// When set, the pipeline uses a direct HTTP call to a text model
    /// instead of spawning an ACP child process.
    pub url: Option<String>,
    /// Rubric model name (passed to codex-acp or used as the HTTP model).
    pub model: Option<String>,
    /// Rubric reasoning effort.
    pub effort: Option<String>,
    /// Rubric system prompt override.
    pub system_prompt: Option<String>,
}

/// Build the canonical direct Codex GPT TOML configuration.
///
/// This is the optimized subscription-backed path: `codex-acp` receives one
/// multimodal prompt containing both the rubric text and the screenshot.
#[cfg(feature = "codex-acp")]
#[must_use]
pub fn direct_codex_gpt_config(model: Option<&str>, effort: Option<&str>) -> TomlConfig {
    TomlConfig {
        mode: Some(ConfigMode::Direct),
        vision: TomlVision::default(),
        rubric: TomlRubric {
            backend: Some("codex-acp".to_string()),
            args: None,
            url: None,
            model: Some(model.unwrap_or(DEFAULT_CODEX_ACP_MODEL).to_string()),
            effort: Some(
                effort
                    .unwrap_or(DEFAULT_CODEX_ACP_REASONING_EFFORT)
                    .to_string(),
            ),
            system_prompt: None,
        },
    }
}

/// Load the TOML config from `path`, falling back to the user config path and
/// then the Infernix-managed `/etc/visual-rubric/config.toml`.
///
/// Returns the default `TomlConfig` (empty optional fields) when the file
/// is missing.
pub fn load_config_toml(path: Option<&Path>) -> Result<TomlConfig, std::io::Error> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => config_candidates()
            .into_iter()
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| PathBuf::from("/etc/visual-rubric/config.toml")),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TomlConfig::default());
        }
        Err(e) => return Err(e),
    };
    toml::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn config_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(base) = config_dir() {
        candidates.push(base.join("visual-rubric/config.toml"));
    }
    candidates.push(PathBuf::from("/etc/visual-rubric/config.toml"));
    candidates
}

fn config_dir() -> Option<PathBuf> {
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
