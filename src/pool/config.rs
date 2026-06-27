use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::{
    DEFAULT_SYSTEM_PROMPT,
    RubricOptions,
};
#[cfg(feature = "codex-acp")]
use crate::{
    DEFAULT_CODEX_ACP_MODEL, DEFAULT_CODEX_ACP_REASONING_EFFORT, RubricEffort,
    default_codex_acp_binary,
};

pub(super) const DEFAULT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Configuration for a [`crate::RubricPool`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    /// Number of worker processes to keep alive.
    pub workers: usize,
    /// Number of prompts after which a worker is recycled.
    pub max_prompts_per_worker: u32,
    /// Number of retries for recoverable worker or rate-limit failures.
    pub max_retries: u32,
    /// Initial retry backoff.
    pub backoff_base: Duration,
    /// Maximum retry backoff.
    pub backoff_cap: Duration,
    /// Options applied when a submitted job omits an override.
    pub default_options: RubricOptions,
    /// Path to the ACP executable.
    pub codex_acp_binary: PathBuf,
    /// Extra CLI arguments for the ACP binary.
    /// When empty, defaults are used (codex-acp args with the feature on,
    /// `["acp"]` with the feature off).
    pub acp_args: Vec<String>,
    /// Extra environment variables for worker processes.
    pub extra_env: Vec<(OsString, OsString)>,
    /// Maximum time to wait for one submitted job.
    pub submit_timeout: Duration,
    /// Optional Codex home directory to seed into worker-local homes.
    #[cfg(feature = "codex-acp")]
    pub source_codex_home: Option<PathBuf>,
    /// Optional ACP log capture directories.
    pub log_capture: Option<LogCaptureConfig>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            max_prompts_per_worker: 50,
            max_retries: 4,
            backoff_base: Duration::from_secs(30),
            backoff_cap: Duration::from_secs(300),
            default_options: default_pool_options(),
            #[cfg(feature = "codex-acp")]
            codex_acp_binary: default_codex_acp_binary(),
            #[cfg(not(feature = "codex-acp"))]
            codex_acp_binary: PathBuf::from("opencode"),
            acp_args: Vec::new(),
            extra_env: Vec::new(),
            submit_timeout: DEFAULT_SUBMIT_TIMEOUT,
            #[cfg(feature = "codex-acp")]
            source_codex_home: None,
            log_capture: None,
        }
    }
}

fn default_pool_options() -> RubricOptions {
    #[cfg(feature = "codex-acp")]
    {
        RubricOptions {
            model: Some(DEFAULT_CODEX_ACP_MODEL.to_string()),
            effort: Some(RubricEffort::from(DEFAULT_CODEX_ACP_REASONING_EFFORT)),
            system_prompt: Some(DEFAULT_SYSTEM_PROMPT.to_string()),
        }
    }
    #[cfg(not(feature = "codex-acp"))]
    {
        RubricOptions {
            model: None,
            effort: None,
            system_prompt: Some(DEFAULT_SYSTEM_PROMPT.to_string()),
        }
    }
}

/// Directories used to capture logs produced by Codex ACP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCaptureConfig {
    /// Temporary root exposed to worker processes through `TMPDIR`.
    pub temp_dir: PathBuf,
    /// Destination directory where selected log files are copied.
    pub output_dir: PathBuf,
    /// How copied log paths are represented in reports.
    pub path_mode: LogPathMode,
}

/// Path representation for copied ACP logs in batch reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LogPathMode {
    /// Report paths relative to this root when possible.
    RelativeTo(PathBuf),
    /// Report absolute paths.
    #[default]
    Absolute,
}

/// Snapshot of pool execution counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Successfully completed jobs.
    pub completed: u64,
    /// Failed jobs.
    pub failures: u64,
    /// Rate-limit events observed by workers.
    pub rate_limit_events: Vec<crate::RateLimitEvent>,
    /// Number of worker runtime recycles.
    pub worker_recycles: u64,
}
