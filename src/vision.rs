//! OpenAI-compatible vision API client for the vision extraction stage.
//!
//! Sends base64-encoded images to a vision model (e.g. Qwen VL via
//! llama-swap) at `POST /v1/chat/completions` and returns the model's
//! text output as a structured JSON description for downstream rubric
//! scoring.

use crate::PoolError;

/// Configuration for calling an OpenAI-compatible vision API.
#[derive(Clone, Debug)]
pub struct VisionApiConfig {
    /// Base URL of the API, e.g. `"http://localhost:8013"`.
    pub url: String,
    /// Model name, e.g. `"qwen3-vl-8b"`.
    pub model: String,
    /// Optional Bearer token for API authentication.
    pub api_key: Option<String>,
}

/// Calls an OpenAI-compatible vision API with a base64-encoded PNG and a
/// text question, returning the model's response text.
///
/// The request is a `POST {url}/v1/chat/completions` with the image
/// embedded as a `data:image/png;base64,...` URL in a multimodal message.
///
/// # Errors
///
/// Returns [`PoolError::VisionApi`] for HTTP, JSON, or unexpected response
/// shape failures.
pub fn call_vision_api(
    b64_png: &str,
    question: &str,
    config: &VisionApiConfig,
) -> Result<String, PoolError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| PoolError::VisionApi(format!("build http client: {e}")))?;

    let url = format!("{}/v1/chat/completions", config.url.trim_end_matches('/'));

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

    let mut request = client.post(&url).json(&body);
    if let Some(ref api_key) = config.api_key {
        request = request.header("Authorization", format!("Bearer {api_key}"));
    }

    let response = request
        .send()
        .map_err(|e| PoolError::VisionApi(format!("vision request: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        return Err(PoolError::VisionApi(format!("vision API {status}: {text}")));
    }

    let result: serde_json::Value = response
        .json()
        .map_err(|e| PoolError::VisionApi(format!("vision response json: {e}")))?;

    let content = result["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            PoolError::VisionApi("vision response missing choices[0].message.content".to_string())
        })?
        .to_string();

    Ok(content)
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
