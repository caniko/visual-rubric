use std::fmt;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateLimitEvent {
    pub worker_id: usize,
    pub attempt: u32,
    pub delay: Duration,
    pub retry_after: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoolError {
    Spawn(String),
    Rpc(String),
    RateLimited { retry_after: Option<Duration> },
    QuotaExceeded,
    WorkerCrashed { worker_id: usize, message: String },
    ParseVerdict(String),
    Timeout { worker_id: usize, timeout: Duration },
    NoLiveWorkers,
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

#[derive(Debug)]
#[non_exhaustive]
pub enum RubricError {
    ReadPng {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    Pool(PoolError),
    ParseVerdict {
        text: String,
        source: serde_json::Error,
    },
    Assertion {
        name: String,
        reason: String,
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
