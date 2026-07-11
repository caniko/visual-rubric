use std::fs::OpenOptions;
use std::io::{BufRead as _, BufReader, Write as _};

fn main() {
    let mode = std::env::var("FAKE_CODEX_ACP_MODE").unwrap_or_else(|_| "quota".to_string());
    if mode == "crash" {
        eprintln!("fake codex-acp crash requested");
        std::process::exit(2);
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ACP_SPAWN_LOG") {
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "spawn"));
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ACP_ARG_LOG") {
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| {
                writeln!(file, "{}", std::env::args().collect::<Vec<_>>().join("\n"))
            });
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ACP_CWD_LOG") {
        if let Ok(cwd) = std::env::current_dir() {
            let _ = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "{}", cwd.display()));
        }
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ACP_ENV_LOG") {
        let value = std::env::var("FAKE_CODEX_ACP_LOG_ENV_KEY")
            .ok()
            .and_then(|key| std::env::var(key).ok())
            .or_else(|| std::env::var("FAKE_CODEX_ACP_CUSTOM_ENV").ok());
        if let Some(value) = value {
            let _ = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| writeln!(file, "{value}"));
        }
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in BufReader::new(stdin.lock()).lines() {
        let Ok(line) = line else {
            return;
        };
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            return;
        };
        let id = msg["id"].clone();
        let method = msg["method"].as_str().unwrap_or_default();
        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {}
            }),
            "session/new" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "sessionId": "fake-session" }
            }),
            "session/prompt" if mode == "prompt_crash" => {
                eprintln!("fake codex-acp prompt crash requested");
                std::process::exit(3);
            }
            "session/prompt"
                if mode == "pass" || mode == "fail" || mode == "chunks" || mode == "malformed" =>
            {
                let session_id = msg["params"]["sessionId"]
                    .as_str()
                    .unwrap_or("fake-session");
                if let Ok(path) = std::env::var("FAKE_CODEX_ACP_PROMPT_LOG") {
                    if let Some(prompt) = msg["params"]["prompt"][0]["text"].as_str() {
                        let _ = OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .and_then(|mut file| writeln!(file, "{prompt}"));
                    }
                }
                if let Ok(path) = std::env::var("FAKE_CODEX_ACP_PROMPT_KIND_LOG") {
                    let has_image = msg["params"]["prompt"]
                        .as_array()
                        .map(|items| items.iter().any(|item| item["type"] == "image"))
                        .unwrap_or(false);
                    let _ = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .and_then(|mut file| writeln!(file, "has_image={has_image}"));
                }
                let chunks: Vec<&str> = match mode.as_str() {
                    "fail" => vec![
                        "{\"verdict\":\"fail\",\"reason\":\"fake fail\",\"anomalies\":[\"bad\"]}",
                    ],
                    "chunks" => vec![
                        "{\"verdict\":\"pass\",",
                        "\"reason\":\"chunked\",",
                        "\"anomalies\":[]}",
                    ],
                    "malformed" => vec!["not json"],
                    _ => vec!["{\"verdict\":\"pass\",\"reason\":\"fake pass\",\"anomalies\":[]}"],
                };
                for chunk in chunks {
                    let update = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": {
                                    "text": chunk
                                }
                            }
                        }
                    });
                    let _ = serde_json::to_writer(&mut stdout, &update);
                    let _ = stdout.write_all(b"\n");
                    let _ = stdout.flush();
                }
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {}
                })
            }
            "session/prompt" if mode == "rate_limit" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": "rate limit reached",
                        "retry_after": 3
                    }
                }
            ),
            "session/prompt" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": "usage limit reached"
                    }
                }
            ),
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unknown method {method}")
                }
            }),
        };
        let _ = serde_json::to_writer(&mut stdout, &response);
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}
