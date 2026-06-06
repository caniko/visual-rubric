//! Shared AI visual-rubric runner for screenshot review.
//!
//! This crate owns the Codex ACP plumbing so browser screenshots, offscreen
//! renderer captures, and VM/VNC screenshots can use one rubric path.
#![warn(missing_docs)]

pub mod cli;
mod errors;
mod pool;
mod typed_strings;

use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

pub use cli::Cli;
pub use errors::{PoolError, RateLimitEvent, RubricError};
pub use pool::{PoolConfig, PoolStats, RubricPool};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};

#[derive(Debug, Deserialize, Serialize)]
/// Parsed rubric verdict returned by Codex ACP.
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
    /// Codex model override.
    pub model: Option<String>,
    /// Reasoning effort override.
    pub effort: Option<RubricEffort>,
    /// System prompt override.
    pub system_prompt: Option<String>,
}

/// Runtime configuration for direct Codex ACP calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RubricRunConfig {
    /// Path to the `codex-acp` executable.
    pub codex_acp_binary: PathBuf,
    /// Extra environment variables for the child process.
    pub extra_env: Vec<(OsString, OsString)>,
    /// Working directory passed to Codex ACP.
    pub cwd: Option<PathBuf>,
}

impl Default for RubricRunConfig {
    fn default() -> Self {
        Self {
            codex_acp_binary: default_codex_acp_binary(),
            extra_env: Vec::new(),
            cwd: None,
        }
    }
}

/// Default system prompt used for screenshot rubric requests.
pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are a UI regression auditor. \
You will be shown one screenshot and asked a specific question. Reply with strict \
JSON matching this schema and nothing else:
{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }
Fail criteria: text clipped or overflowing its container, overlapping interactive \
elements, missing/blank regions where content should appear, illegible contrast, \
visibly broken layout. Cosmetic differences from previous runs are NOT failures \
unless they make the UI worse by the criteria above.";

/// Default Codex ACP model.
pub const DEFAULT_CODEX_ACP_MODEL: &str = "gpt-5.4-mini";
/// Default Codex ACP reasoning effort.
pub const DEFAULT_CODEX_ACP_REASONING_EFFORT: &str = "medium";

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
/// Returns [`RubricError`] for PNG IO, Codex ACP, JSON parsing, or failed
/// assertion errors.
pub fn assert_image_rubric(png_path: &Path, name: &str, question: &str) -> Result<(), RubricError> {
    let verdict = evaluate_image_rubric(png_path, question)?;
    assert_verdict(name, verdict)
}

/// Evaluates a PNG with default options.
///
/// # Errors
///
/// Returns [`RubricError`] for PNG IO, Codex ACP, or verdict parsing failures.
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
/// Returns [`RubricError`] for PNG IO, Codex ACP, or verdict parsing failures.
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
/// Returns [`RubricError`] for PNG IO, Codex ACP, or verdict parsing failures.
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
        opts.model.as_deref().unwrap_or(DEFAULT_CODEX_ACP_MODEL),
        opts.effort
            .as_deref()
            .unwrap_or(DEFAULT_CODEX_ACP_REASONING_EFFORT),
        opts.system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_SYSTEM_PROMPT),
        &config,
    )?;

    parse_verdict(&text).map_err(|source| RubricError::ParseVerdict { text, source })
}

/// Parses strict rubric JSON into a typed verdict.
///
/// # Errors
///
/// Returns the underlying JSON error when the text is malformed or contains an
/// unsupported verdict status.
pub fn parse_verdict(text: &str) -> Result<RubricVerdict, serde_json::Error> {
    serde_json::from_str(text)
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
/// Returns command parsing, IO, Codex ACP, or audit failures as [`anyhow::Error`].
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
    let mut acp = AcpClient::spawn(
        &config.codex_acp_binary,
        model,
        effort,
        &config.extra_env,
        config.cwd.as_deref(),
    )?;
    acp.start_session(config.cwd.as_deref())?;

    let prompt = format!("{system_prompt}\n\nQuestion: {question}");
    acp.prompt_image(&prompt, b64_png)
}

struct AcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    session_id: Option<String>,
}

impl AcpClient {
    fn spawn(
        binary: &Path,
        model: &str,
        effort: &str,
        extra_env: &[(OsString, OsString)],
        cwd: Option<&Path>,
    ) -> Result<Self, PoolError> {
        let mut command = Command::new(binary);
        command
            .arg("-c")
            .arg(format!("model=\"{model}\""))
            .arg("-c")
            .arg(format!("model_reasoning_effort=\"{effort}\""))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        for (key, value) in extra_env {
            command.env::<&OsStr, &OsStr>(key.as_os_str(), value.as_os_str());
        }
        let mut child = command
            .spawn()
            .map_err(|e| PoolError::Spawn(format!("spawn {}: {e}", binary.display())))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PoolError::Spawn("codex-acp stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PoolError::Spawn("codex-acp stdout unavailable".to_string()))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            session_id: None,
        })
    }

    fn start_session(&mut self, cwd: Option<&Path>) -> Result<(), PoolError> {
        let init_id = self.claim_id();
        self.request(
            init_id,
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "cb-rubric",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;

        let cwd = match cwd {
            Some(cwd) => cwd.to_path_buf(),
            None => {
                std::env::current_dir().map_err(|e| PoolError::Rpc(format!("current dir: {e}")))?
            }
        }
        .to_string_lossy()
        .into_owned();
        let session_request_id = self.claim_id();
        let session_id = self.request(
            session_request_id,
            "session/new",
            serde_json::json!({
                "cwd": cwd,
                "mcpServers": []
            }),
        )?["sessionId"]
            .as_str()
            .ok_or_else(|| PoolError::Rpc("unexpected session/new response shape".to_string()))?
            .to_string();
        self.session_id = Some(session_id);
        Ok(())
    }

    fn prompt_image(&mut self, prompt: &str, b64_png: &str) -> Result<String, PoolError> {
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| PoolError::Rpc("session not initialized".to_string()))?;
        let prompt_id = self.claim_id();
        self.prompt(
            prompt_id,
            &session_id,
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [
                    { "type": "text", "text": prompt },
                    { "type": "image", "data": b64_png, "mimeType": "image/png" }
                ]
            }),
        )
    }

    fn claim_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn request(
        &mut self,
        id: i64,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, PoolError> {
        self.send(id, method, params)?;

        loop {
            let msg = self.read_message()?;
            if msg["id"].as_i64() == Some(id) {
                return rpc_result(msg);
            }
        }
    }

    fn prompt(
        &mut self,
        id: i64,
        session_id: &str,
        params: serde_json::Value,
    ) -> Result<String, PoolError> {
        self.send(id, "session/prompt", params)?;

        let mut text = String::new();
        loop {
            let msg = self.read_message()?;
            if msg["id"].as_i64() == Some(id) {
                rpc_result(msg)?;
                return Ok(text);
            }

            if msg["method"] == "session/update" && msg["params"]["sessionId"] == session_id {
                let update = &msg["params"]["update"];
                if update["sessionUpdate"] == "agent_message_chunk"
                    && let Some(chunk) = update["content"]["text"].as_str()
                {
                    text.push_str(chunk);
                }
            }
        }
    }

    fn send(&mut self, id: i64, method: &str, params: serde_json::Value) -> Result<(), PoolError> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        serde_json::to_writer(&mut self.stdin, &msg)
            .map_err(|e| PoolError::Rpc(format!("write codex-acp request: {e}")))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|e| PoolError::Rpc(format!("write codex-acp newline: {e}")))?;
        self.stdin
            .flush()
            .map_err(|e| PoolError::Rpc(format!("flush codex-acp request: {e}")))
    }

    fn read_message(&mut self) -> Result<serde_json::Value, PoolError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| PoolError::Rpc(format!("read codex-acp response: {e}")))?;
        if n == 0 {
            let stderr = self
                .child
                .stderr
                .take()
                .map(|mut stderr| {
                    let mut buf = String::new();
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut buf);
                    buf
                })
                .unwrap_or_default();
            return Err(PoolError::WorkerCrashed {
                worker_id: usize::MAX,
                message: format!("codex-acp exited before response: {stderr}"),
            });
        }

        serde_json::from_str(&line)
            .map_err(|e| PoolError::Rpc(format!("parse codex-acp message {line:?}: {e}")))
    }
}

impl Drop for AcpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rpc_result(msg: serde_json::Value) -> Result<serde_json::Value, PoolError> {
    if let Some(error) = msg.get("error") {
        let message = error.to_string();
        let lowered = message.to_ascii_lowercase();
        if lowered.contains("usage limit") || lowered.contains("quota") {
            Err(PoolError::QuotaExceeded)
        } else if lowered.contains("rate limit") {
            Err(PoolError::RateLimited {
                retry_after: parse_retry_after(error),
            })
        } else {
            Err(PoolError::Rpc(format!("codex-acp rpc error: {error}")))
        }
    } else {
        Ok(msg["result"].clone())
    }
}

fn parse_retry_after(error: &serde_json::Value) -> Option<std::time::Duration> {
    let candidates = [
        &error["retry_after"],
        &error["retryAfter"],
        &error["data"]["retry_after"],
        &error["data"]["retryAfter"],
    ];
    for candidate in candidates {
        if let Some(seconds) = candidate.as_u64() {
            return Some(std::time::Duration::from_secs(seconds));
        }
        if let Some(seconds) = candidate.as_f64()
            && seconds.is_finite()
            && seconds >= 0.0
        {
            return Some(std::time::Duration::from_secs_f64(seconds));
        }
        if let Some(value) = candidate.as_str()
            && let Ok(seconds) = value.parse::<u64>()
        {
            return Some(std::time::Duration::from_secs(seconds));
        }
    }
    None
}

#[cfg(test)]
mod tests;
