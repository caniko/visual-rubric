use super::*;
use std::path::PathBuf;

#[cfg(feature = "codex-acp")]
#[test]
fn default_options_use_documented_defaults() {
    let options = default_options();

    assert_eq!(options.model.as_deref(), Some(DEFAULT_CODEX_ACP_MODEL));
    assert_eq!(
        options.effort.as_deref(),
        Some(DEFAULT_CODEX_ACP_REASONING_EFFORT)
    );
    assert_eq!(
        options.system_prompt.as_deref(),
        Some(DEFAULT_SYSTEM_PROMPT)
    );
}

#[cfg(feature = "codex-acp")]
#[test]
fn direct_toml_run_config_uses_codex_model_args() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
mode = "direct"

[rubric]
backend = "codex-acp"
model = "gpt-5.5"
effort = "medium"
"#,
    )
    .expect("write config");

    let config = RubricRunConfig::from_config_toml(Some(&config_path));

    assert_eq!(config.codex_acp_binary, PathBuf::from("codex-acp"));
    assert_eq!(config.acp_args, build_codex_acp_args("gpt-5.5", "medium"));
}

#[test]
fn sequence_toml_policy_is_loaded_and_bounded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        "[sequence]\nmax_frames = 12\nrequire_transition = true\n",
    )
    .expect("write config");

    let options = SequenceOptions::from_config_toml(Some(&config_path));
    assert_eq!(options.max_frames, 12);
    assert!(options.require_transition);
    assert!(options.validate().is_ok());
}

#[test]
fn sequence_policy_rejects_unbounded_values() {
    assert!(
        SequenceOptions {
            max_frames: 0,
            require_transition: true,
        }
        .validate()
        .is_err()
    );
    assert!(
        SequenceOptions {
            max_frames: 33,
            require_transition: true,
        }
        .validate()
        .is_err()
    );
}

#[cfg(feature = "codex-acp")]
#[test]
fn transition_policy_rejects_single_checkpoint() {
    let error = evaluate_image_sequence_rubric_with_options(
        &[SequenceFrame {
            label: "only".to_owned(),
            path: PathBuf::from("does-not-exist.png"),
        }],
        "question",
        RubricOptions::default(),
        RubricRunConfig::default(),
        SequenceOptions::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("at least two frames"));
}

#[test]
fn encode_png_base64_encodes_file_contents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let png = temp.path().join("sample.png");
    std::fs::write(&png, [1_u8, 2, 3]).expect("write png");

    assert_eq!(encode_png(&png).expect("encoded png"), "AQID");
}

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
fn report_markdown_groups_anomalies_and_keeps_page_details() {
    let report = RubricReport::from_pages(
        "Regression context",
        vec![PageResult {
            label: "home".to_string(),
            route: Some("/".to_string()),
            viewport: Some((1280, 720)),
            screenshot_path: PathBuf::from("/tmp/home.png"),
            vision_description: "Header\nFooter".to_string(),
            verdict: RubricVerdict {
                verdict: "fail".into(),
                reason: "layout shifted".into(),
                anomalies: vec!["layout.header overlaps".to_string()],
            },
        }],
    );

    let markdown = report.to_markdown();

    assert!(markdown.contains("- **Total pages**: 1"));
    assert!(markdown.contains("- **Failed**: 1"));
    assert!(markdown.contains("| layout | 1 | `home` |"));
    assert!(markdown.contains("**Viewport:** 1280×720"));
    assert!(markdown.contains("> Header\n> Footer"));
    assert!(markdown.contains("- layout.header overlaps"));
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

#[test]
fn parse_verdict_extracts_json_when_strings_contain_braces() {
    let verdict = parse_verdict(
        r#"analysis {"verdict":"fail","reason":"selector {main} overlaps","anomalies":[]}"#,
    )
    .unwrap();

    assert_eq!(verdict.verdict, "fail");
    assert_eq!(verdict.reason, "selector {main} overlaps");
}

#[test]
fn parse_verdict_repairs_unquoted_key() {
    let verdict = parse_verdict(
        r#"{
            "verdict": "fail",
            "reason": "test",
            "anomalies": [
                {
                    "type": "visual_issue",
                description": "missing opening quote",
                    "position": "bottom-right"
                }
            ]
        }"#,
    )
    .unwrap();
    assert_eq!(verdict.verdict, "fail");
    assert_eq!(verdict.anomalies.len(), 1);
    assert!(verdict.anomalies[0].contains("missing opening quote"));
}

#[test]
fn parse_verdict_repairs_trailing_comma() {
    let verdict = parse_verdict(
        r#"{
            "verdict": "pass",
            "reason": "looks good",
            "anomalies": ["small issue",]
        }"#,
    )
    .unwrap();
    assert!(verdict.verdict.is_pass());
    assert_eq!(verdict.anomalies, vec!["small issue"]);
}

#[test]
fn parse_verdict_repairs_full_text() {
    let input = r#"{
  "verdict": "fail",
  "reason": "test",
  "anomalies": [
    {
      "type": "visual_issue",
    description": "missing quote",
      "position": "bottom-right"
    }
  ]
}"#;
    let verdict = parse_verdict(input).unwrap();
    assert_eq!(verdict.verdict, "fail");
    assert_eq!(verdict.anomalies.len(), 1);
    assert!(verdict.anomalies[0].contains("missing quote"));
}
