//! Shared AI visual-rubric runner for screenshot review.
//!
//! This crate owns the Codex ACP plumbing so browser screenshots, offscreen
//! renderer captures, and VM/VNC screenshots can use one rubric path.

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
pub use errors::{PoolError, RateLimitEvent};
pub use pool::{PoolConfig, PoolStats, RubricPool};
pub use typed_strings::{RubricEffort, RubricVerdictStatus};

#[derive(Debug, Deserialize, Serialize)]
pub struct RubricVerdict {
    pub verdict: RubricVerdictStatus,
    pub reason: String,
    #[serde(default)]
    pub anomalies: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct RubricOptions {
    pub model: Option<String>,
    pub effort: Option<RubricEffort>,
    pub system_prompt: Option<String>,
}

pub const DEFAULT_SYSTEM_PROMPT: &str = "\
You are a UI regression auditor. \
You will be shown one screenshot and asked a specific question. Reply with strict \
JSON matching this schema and nothing else:
{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }
Fail criteria: text clipped or overflowing its container, overlapping interactive \
elements, missing/blank regions where content should appear, illegible contrast, \
visibly broken layout. Cosmetic differences from previous runs are NOT failures \
unless they make the UI worse by the criteria above.";

pub const DEFAULT_CODEX_ACP_MODEL: &str = "gpt-5.4-mini";
pub const DEFAULT_CODEX_ACP_REASONING_EFFORT: &str = "medium";

pub fn default_options() -> RubricOptions {
    RubricOptions {
        model: Some(DEFAULT_CODEX_ACP_MODEL.to_string()),
        effort: Some(DEFAULT_CODEX_ACP_REASONING_EFFORT.into()),
        system_prompt: Some(DEFAULT_SYSTEM_PROMPT.to_string()),
    }
}

pub fn default_codex_acp_binary() -> PathBuf {
    PathBuf::from("codex-acp")
}

pub fn encode_png(png_path: &Path) -> Result<String, PoolError> {
    let bytes = std::fs::read(png_path)
        .map_err(|e| PoolError::Rpc(format!("read png {}: {e}", png_path.display())))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub fn assert_image_rubric(png_path: &Path, name: &str, question: &str) -> Result<(), String> {
    let verdict = evaluate_image_rubric(png_path, question)?;
    assert_verdict(name, verdict)
}

pub fn evaluate_image_rubric(png_path: &Path, question: &str) -> Result<RubricVerdict, String> {
    evaluate_image_rubric_with_options(png_path, question, default_options())
}

pub fn evaluate_image_rubric_with_options(
    png_path: &Path,
    question: &str,
    opts: RubricOptions,
) -> Result<RubricVerdict, String> {
    let bytes = std::fs::read(png_path).map_err(|e| format!("read png: {e}"))?;
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
    )?;

    parse_verdict(&text).map_err(|e| format!("parse verdict from {text:?}: {e}"))
}

pub fn parse_verdict(text: &str) -> Result<RubricVerdict, serde_json::Error> {
    serde_json::from_str(text)
}

pub fn assert_verdict(name: &str, verdict: RubricVerdict) -> Result<(), String> {
    if verdict.verdict.is_pass() {
        Ok(())
    } else {
        Err(format!(
            "[{name}] {} (anomalies: {:?})",
            verdict.reason, verdict.anomalies
        ))
    }
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    cli::run(cli)
}

fn run_codex_acp_rubric(
    b64_png: &str,
    question: &str,
    model: &str,
    effort: &str,
    system_prompt: &str,
) -> Result<String, String> {
    let mut acp = AcpClient::spawn(&default_codex_acp_binary(), model, effort, &[])
        .map_err(|e| e.to_string())?;
    acp.start_session().map_err(|e| e.to_string())?;

    let prompt = format!("{system_prompt}\n\nQuestion: {question}");
    acp.prompt_image(&prompt, b64_png)
        .map_err(|e| e.to_string())
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

    fn start_session(&mut self) -> Result<(), PoolError> {
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

        let cwd = std::env::current_dir()
            .map_err(|e| PoolError::Rpc(format!("current dir: {e}")))?
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
            Err(PoolError::RateLimited { retry_after: None })
        } else {
            Err(PoolError::Rpc(format!("codex-acp rpc error: {error}")))
        }
    } else {
        Ok(msg["result"].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_verdict_succeeds() {
        let verdict = RubricVerdict {
            verdict: "pass".into(),
            reason: "ok".into(),
            anomalies: vec![],
        };
        assert_verdict("smoke", verdict).unwrap();
    }

    #[test]
    fn fail_verdict_includes_name_and_reason() {
        let verdict = RubricVerdict {
            verdict: "fail".into(),
            reason: "blank screen".into(),
            anomalies: vec!["black frame".into()],
        };
        let err = assert_verdict("vm-1", verdict).unwrap_err();
        assert!(err.contains("vm-1"));
        assert!(err.contains("blank screen"));
    }

    #[test]
    fn parse_verdict_rejects_unknown_status() {
        let err =
            parse_verdict(r#"{"verdict":"maybe","reason":"unclear","anomalies":[]}"#).unwrap_err();
        assert!(err.to_string().contains("unknown rubric verdict status"));
    }

    #[test]
    fn parse_verdict_rejects_malformed_json() {
        let err = parse_verdict(r#"{"verdict":"pass""#).unwrap_err();
        assert!(err.is_syntax() || err.is_eof());
    }
}
