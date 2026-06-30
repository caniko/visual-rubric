//! Rubric verdict types, parsing, and assertion helpers.

use serde::{Deserialize, Serialize};

use crate::{RubricError, RubricVerdictStatus};

/// Parsed rubric verdict returned by ACP.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RubricVerdict {
    /// Machine-readable pass/fail status.
    pub verdict: RubricVerdictStatus,
    /// Human-readable reason for the verdict.
    pub reason: String,
    /// Optional anomalies observed in the screenshot.
    #[serde(default, deserialize_with = "deserialize_anomalies")]
    pub anomalies: Vec<String>,
}

/// Parses strict rubric JSON into a typed verdict.
///
/// Tries the text as raw JSON first, then tries to extract a JSON object
/// from surrounding text (e.g. markdown code blocks), and finally applies
/// a lightweight repair pass for common model-output issues (missing
/// opening quotes on keys, trailing commas before `]`/`}`).
///
/// # Errors
///
/// Returns the underlying JSON error when the text is malformed or contains an
/// unsupported verdict status.
pub fn parse_verdict(text: &str) -> Result<RubricVerdict, serde_json::Error> {
    match serde_json::from_str(text) {
        Ok(verdict) => return Ok(verdict),
        Err(_) => {}
    }

    if let Some(json) = extract_json_object(text) {
        if let Ok(verdict) = serde_json::from_str(json) {
            return Ok(verdict);
        }
        let repaired = repair_json(json);
        if let Ok(verdict) = serde_json::from_str(&repaired) {
            return Ok(verdict);
        }
    }

    let repaired = repair_json(text);
    serde_json::from_str(&repaired)
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

/// Lightweight repair for common JSON formatting issues in model output.
///
/// Handles:
/// 1. Unquoted object keys (`key":` -> `"key":`)
/// 2. Trailing commas before `]` or `}`
fn repair_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] as char {
            '"' => {
                out.push('"');
                i += 1;
                while i < len {
                    let ch = bytes[i] as char;
                    out.push(ch);
                    i += 1;
                    if ch == '\\' && i < len {
                        out.push(bytes[i] as char);
                        i += 1;
                    } else if ch == '"' {
                        break;
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < len && ((bytes[i] as char).is_alphanumeric() || bytes[i] as char == '_') {
                    i += 1;
                }
                let word = &text[start..i];
                if i + 1 < len && bytes[i] as char == '"' && bytes[i + 1] as char == ':' {
                    out.push('"');
                    out.push_str(word);
                    out.push('"');
                    out.push(':');
                    i += 2;
                } else {
                    out.push_str(word);
                }
            }
            ',' => {
                let mut j = i + 1;
                while j < len && (bytes[j] as char).is_ascii_whitespace() {
                    j += 1;
                }
                if j < len && matches!(bytes[j] as char, ']' | '}') {
                    out.push(' ');
                    i = j;
                } else {
                    out.push(',');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + offset + character.len_utf8();
                    return Some(&text[start..end]);
                }
            }
            _ => {}
        }
    }

    None
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
