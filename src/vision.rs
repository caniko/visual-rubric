//! OpenAI-compatible API client for the vision and rubric stages.
//!
//! Sends images and text prompts to an OpenAI-compatible endpoint (e.g.
//! llama-swap, vLLM, Ollama) at `POST /v1/chat/completions` and returns
//! the model's text output.

use crate::PoolError;

/// Configuration for calling an OpenAI-compatible API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisionApiConfig {
    /// Base URL of the API, e.g. `"http://localhost:8013"`.
    pub url: String,
    /// Model name, e.g. `"qwen3-vl-8b"`.
    pub model: String,
    /// Optional Bearer token for API authentication.
    pub api_key: Option<String>,
}

/// Sends a chat-completions body and extracts the response text.
///
/// Shares the retry/HTTP plumbing between [`call_vision_api`] and
/// [`call_text_api`].
fn post_chat_completions(
    body: serde_json::Value,
    config: &VisionApiConfig,
) -> Result<String, PoolError> {
    // `reqwest::blocking::Client::build()` creates a tokio runtime
    // internally.  Running it on a dedicated thread avoids a nested-runtime
    // panic when the caller is already inside a `#[tokio::test]` context.
    let cfg = config.clone();
    std::thread::spawn(move || -> Result<String, PoolError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| PoolError::VisionApi(format!("build http client: {e}")))?;

        let url = format!("{}/v1/chat/completions", cfg.url.trim_end_matches('/'));

        let mut request = client.post(&url).json(&body);
        if let Some(ref api_key) = cfg.api_key {
            request = request.header("Authorization", format!("Bearer {api_key}"));
        }

        // Retry up to 3 times on 503 (model warmup / backend busy).
        let mut response = None;
        for attempt in 0..3 {
            match request
                .try_clone()
                .unwrap_or_else(|| client.post(&url).json(&body))
                .send()
            {
                Ok(resp) => {
                    if resp.status() == 503 {
                        if attempt < 2 {
                            let delay_secs = 5 + attempt * 10;
                            std::thread::sleep(std::time::Duration::from_secs(delay_secs));
                            continue;
                        }
                        let text = resp.text().unwrap_or_default();
                        return Err(PoolError::VisionApi(format!(
                            "API 503 after retries: {text}"
                        )));
                    }
                    response = Some(resp);
                    break;
                }
                Err(e) => {
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_secs(5));
                        continue;
                    }
                    return Err(PoolError::VisionApi(format!("API request: {e}")));
                }
            }
        }

        let response = response
            .ok_or_else(|| PoolError::VisionApi("API request failed after retries".to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().unwrap_or_default();
            return Err(PoolError::VisionApi(format!("API {status}: {text}")));
        }

        let result: serde_json::Value = response
            .json()
            .map_err(|e| PoolError::VisionApi(format!("API response json: {e}")))?;

        let content = result["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                PoolError::VisionApi("response missing choices[0].message.content".to_string())
            })?
            .to_string();

        Ok(content)
    })
    .join()
    .map_err(|_| PoolError::VisionApi("thread panicked".to_string()))?
}

/// Calls a vision API with a base64-encoded PNG and a text question.
///
/// The request embeds the image as a `data:image/png;base64,...` URL in a
/// multimodal message for VLM models.
///
/// # Errors
///
/// Returns [`PoolError::VisionApi`] for HTTP, JSON, or unexpected response
/// shape failures.
#[cfg(feature = "vision-api")]
pub fn call_vision_api(
    b64_png: &str,
    question: &str,
    config: &VisionApiConfig,
) -> Result<String, PoolError> {
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{b64_png}")
                    }
                },
                {
                    "type": "text",
                    "text": question
                }
            ]
        }],
        "max_tokens": 4096,
    });
    post_chat_completions(body, config)
}

/// Calls a vision API with an ordered sequence of labelled PNG frames.
///
/// The labels are included between image parts so the model can reason about
/// before/after transitions instead of treating the input as an unordered
/// collage.
#[cfg(feature = "vision-api")]
pub fn call_vision_api_sequence(
    frames: &[(String, String)],
    question: &str,
    config: &VisionApiConfig,
) -> Result<String, PoolError> {
    if frames.is_empty() {
        return Err(PoolError::VisionApi(
            "sequence vision evaluation requires at least one frame".to_owned(),
        ));
    }
    let mut content = Vec::with_capacity(frames.len() * 2 + 1);
    for (label, b64_png) in frames {
        content.push(serde_json::json!({
            "type": "text",
            "text": format!("Checkpoint: {label}"),
        }));
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{b64_png}")
            }
        }));
    }
    content.push(serde_json::json!({"type": "text", "text": question}));
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": content}],
        "max_tokens": 4096,
    });
    post_chat_completions(body, config)
}

/// Calls a text-only model for rubric evaluation.
///
/// Sends a plain-text prompt (no image) to an OpenAI-compatible endpoint.
/// Used when the rubric evaluation is performed by a separate text model
/// rather than through ACP.
///
/// # Errors
///
/// Returns [`PoolError::VisionApi`] for HTTP, JSON, or unexpected response
/// shape failures.
#[cfg(feature = "http-rubric")]
pub fn call_text_api(prompt: &str, config: &VisionApiConfig) -> Result<String, PoolError> {
    let body = serde_json::json!({
        "model": config.model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 4096,
    });
    post_chat_completions(body, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_api_rejects_bad_url() {
        let config = VisionApiConfig {
            url: "http://127.0.0.1:1".to_string(),
            model: "test-model".to_string(),
            api_key: None,
        };
        let result = call_vision_api("fake-base64", "test question", &config);
        assert!(result.is_err(), "expected error for unreachable port");
        match result {
            Err(PoolError::VisionApi(msg)) => {
                assert!(!msg.is_empty(), "error message should not be empty");
            }
            other => panic!("expected VisionApi error, got {other:?}"),
        }
    }
}
