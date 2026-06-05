use std::fmt;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
/// One observed Codex ACP rate-limit event.
pub struct RateLimitEvent {
    /// Worker that observed the rate limit.
    pub worker_id: usize,
    /// Retry attempt number.
    pub attempt: u32,
    /// Delay applied before the next retry.
    pub delay: Duration,
    /// Server-provided retry delay, when present.
    pub retry_after: Option<Duration>,
}

/// Errors produced by the Codex ACP pool and worker runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoolError {
    /// Failed to spawn or initialize Codex ACP.
    Spawn(String),
    /// JSON-RPC request or response failure.
    Rpc(String),
    /// Codex ACP reported a rate limit.
    RateLimited {
        /// Server-provided retry delay, when present.
        retry_after: Option<Duration>,
    },
    /// Codex ACP reported exhausted usage quota.
    QuotaExceeded,
    /// A worker process or thread crashed.
    WorkerCrashed {
        /// Worker that crashed.
        worker_id: usize,
        /// Crash detail.
        message: String,
    },
    /// A model response could not be parsed into a verdict.
    ParseVerdict(String),
    /// A submitted job exceeded its timeout.
    Timeout {
        /// Worker that timed out.
        worker_id: usize,
        /// Configured timeout.
        timeout: Duration,
    },
    /// No worker is currently available to accept jobs.
    NoLiveWorkers,
    /// The pool has been closed.
    Closed,
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(message) => write!(f, "spawn codex-acp: {message}"),
            Self::Rpc(message) => write!(f, "codex-acp rpc error: {message}"),
            Self::RateLimited { retry_after } => {
                write!(f, "codex-acp rate limited")?;
                if let Some(retry_after) = retry_after {
                    write!(f, " retry_after={retry_after:?}")?;
                }
                Ok(())
            }
            Self::QuotaExceeded => write!(f, "codex-acp usage quota exceeded"),
            Self::WorkerCrashed { worker_id, message } => {
                write!(f, "rubric worker {worker_id} crashed: {message}")
            }
            Self::ParseVerdict(message) => write!(f, "parse rubric verdict: {message}"),
            Self::Timeout { worker_id, timeout } => {
                write!(f, "rubric worker {worker_id} timed out after {timeout:?}")
            }
            Self::NoLiveWorkers => write!(f, "no live rubric workers"),
            Self::Closed => write!(f, "rubric pool is closed"),
        }
    }
}

impl std::error::Error for PoolError {}

/// Errors returned by public one-shot rubric APIs.
#[derive(Debug)]
#[non_exhaustive]
pub enum RubricError {
    /// The PNG file could not be read.
    ReadPng {
        /// Path that failed to read.
        path: std::path::PathBuf,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// Codex ACP pool or worker failure.
    Pool(PoolError),
    /// Model output could not be parsed as a rubric verdict.
    ParseVerdict {
        /// Raw model output.
        text: String,
        /// Underlying JSON error.
        source: serde_json::Error,
    },
    /// A parsed verdict failed the assertion.
    Assertion {
        /// Screenshot or check name.
        name: String,
        /// Failure reason from the verdict.
        reason: String,
        /// Reported visual anomalies.
        anomalies: Vec<String>,
    },
}

impl fmt::Display for RubricError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadPng { path, source } => {
                write!(f, "read png {}: {source}", path.display())
            }
            Self::Pool(error) => error.fmt(f),
            Self::ParseVerdict { text, source } => {
                write!(f, "parse verdict from {text:?}: {source}")
            }
            Self::Assertion {
                name,
                reason,
                anomalies,
            } => {
                write!(f, "[{name}] {reason} (anomalies: {anomalies:?})")
            }
        }
    }
}

impl std::error::Error for RubricError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadPng { source, .. } => Some(source),
            Self::Pool(error) => Some(error),
            Self::ParseVerdict { source, .. } => Some(source),
            Self::Assertion { .. } => None,
        }
    }
}

impl From<PoolError> for RubricError {
    fn from(error: PoolError) -> Self {
        Self::Pool(error)
    }
}
