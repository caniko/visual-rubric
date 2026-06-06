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
    assert!(matches!(err, RubricError::Assertion { .. }));
    let message = err.to_string();
    assert!(message.contains("vm-1"));
    assert!(message.contains("blank screen"));
}

#[test]
fn evaluate_with_config_preserves_parse_error_source() {
    let err = RubricError::ParseVerdict {
        text: "not json".to_string(),
        source: parse_verdict("not json").unwrap_err(),
    };
    assert!(std::error::Error::source(&err).is_some());
    assert!(err.to_string().contains("not json"));
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

#[test]
fn parse_verdict_accepts_structured_anomalies() {
    let verdict = parse_verdict(
        r#"{
            "verdict": "fail",
            "reason": "needs work",
            "anomalies": [
                {
                    "issue": "Command wraps awkwardly.",
                    "fix": "Keep commands horizontally scrollable."
                }
            ]
        }"#,
    )
    .unwrap();

    assert_eq!(
        verdict.anomalies,
        vec!["Command wraps awkwardly. Fix: Keep commands horizontally scrollable."]
    );
}

#[test]
fn parse_verdict_accepts_prefixed_json_object() {
    let verdict = parse_verdict(
        r#"I will inspect the screenshot first.{"verdict":"pass","reason":"ok","anomalies":[]}"#,
    )
    .unwrap();

    assert!(verdict.verdict.is_pass());
    assert_eq!(verdict.reason, "ok");
    assert!(verdict.anomalies.is_empty());
}
