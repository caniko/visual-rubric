use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::PoolError;

pub(crate) struct AcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    session_id: Option<String>,
}

impl AcpClient {
    pub(crate) fn spawn(
        binary: &Path,
        args: &[String],
        extra_env: &[(OsString, OsString)],
        cwd: Option<&Path>,
    ) -> Result<Self, PoolError> {
        let mut command = Command::new(binary);
        command
            .args(args)
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
            .ok_or_else(|| PoolError::Spawn("acp stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PoolError::Spawn("acp stdout unavailable".to_string()))?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            session_id: None,
        })
    }

    pub(crate) fn start_session(&mut self, cwd: Option<&Path>) -> Result<(), PoolError> {
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

    pub(crate) fn prompt_image(
        &mut self,
        prompt: &str,
        b64_png: &str,
    ) -> Result<String, PoolError> {
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

    pub(crate) fn prompt_text(&mut self, prompt: &str) -> Result<String, PoolError> {
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
                    { "type": "text", "text": prompt }
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
                if update["sessionUpdate"] == "agent_message_chunk" {
                    if let Some(chunk) = update["content"]["text"].as_str() {
                        text.push_str(chunk);
                    }
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
            .map_err(|e| PoolError::Rpc(format!("write acp request: {e}")))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|e| PoolError::Rpc(format!("write acp newline: {e}")))?;
        self.stdin
            .flush()
            .map_err(|e| PoolError::Rpc(format!("flush acp request: {e}")))
    }

    fn read_message(&mut self) -> Result<serde_json::Value, PoolError> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| PoolError::Rpc(format!("read acp response: {e}")))?;
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
                message: format!("acp exited before response: {stderr}"),
            });
        }

        serde_json::from_str(&line)
            .map_err(|e| PoolError::Rpc(format!("parse acp message {line:?}: {e}")))
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
            Err(PoolError::Rpc(format!("acp rpc error: {error}")))
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
        if let Some(seconds) = candidate.as_f64() {
            if seconds.is_finite() && seconds >= 0.0 {
                return Some(std::time::Duration::from_secs_f64(seconds));
            }
        }
        if let Some(value) = candidate.as_str() {
            if let Ok(seconds) = value.parse::<u64>() {
                return Some(std::time::Duration::from_secs(seconds));
            }
        }
    }
    None
}

/// Builds the default CLI arguments for the codex-acp binary from a model
/// name and reasoning effort.
#[must_use]
pub fn build_codex_acp_args(model: &str, effort: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("model=\"{model}\""),
        "-c".to_string(),
        format!("model_reasoning_effort=\"{effort}\""),
    ]
}
