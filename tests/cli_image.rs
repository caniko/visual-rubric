#![cfg(feature = "codex-acp")]

mod common;

use std::ffi::OsString;
use std::process::Command;

use visual_rubric::{
    ConfigMode, RubricOptions, RubricRunConfig, build_codex_acp_args, direct_codex_gpt_config,
    evaluate_image_rubric_with_config,
};

#[test]
fn image_json_outputs_verdict() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("image")
        .arg("--image")
        .arg(&image)
        .arg("--question")
        .arg("Does it pass?")
        .arg("--codex-acp")
        .arg(fake)
        .arg("--json")
        .env("FAKE_CODEX_ACP_MODE", "pass")
        .output()
        .expect("run visual-rubric");

    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json stdout");
    assert_eq!(json["verdict"], "pass");
    assert_eq!(json["reason"], "fake pass");
}

#[test]
fn image_fail_verdict_returns_error_without_json() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("image")
        .arg("--image")
        .arg(&image)
        .arg("--question")
        .arg("Does it fail?")
        .arg("--name")
        .arg("fixture")
        .arg("--codex-acp")
        .arg(fake)
        .env("FAKE_CODEX_ACP_MODE", "fail")
        .output()
        .expect("run visual-rubric");

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fixture"), "{stderr}");
    assert!(stderr.contains("fake fail"), "{stderr}");
}

#[test]
fn image_forwards_model_effort_and_system_prompt_to_custom_acp() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let config_log = temp.path().join("config.log");
    let prompt_log = temp.path().join("prompts.log");
    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("image")
        .arg("--image")
        .arg(&image)
        .arg("--question")
        .arg("Does it pass?")
        .arg("--system-prompt")
        .arg("Project rubric")
        .arg("--model")
        .arg("custom-model")
        .arg("--effort")
        .arg("high")
        .arg("--codex-acp")
        .arg(fake)
        .arg("--json")
        .env("FAKE_CODEX_ACP_MODE", "pass")
        .env("FAKE_CODEX_ACP_CONFIG_LOG", &config_log)
        .env("FAKE_CODEX_ACP_PROMPT_LOG", &prompt_log)
        .output()
        .expect("run visual-rubric");

    assert!(output.status.success(), "{output:?}");
    let config = std::fs::read_to_string(config_log).expect("config log");
    assert!(config.contains("\"configId\":\"model\""), "{config}");
    assert!(config.contains("\"value\":\"custom-model\""), "{config}");
    assert!(
        config.contains("\"configId\":\"reasoning_effort\""),
        "{config}"
    );
    assert!(config.contains("\"value\":\"high\""), "{config}");
    assert!(config.contains("\"configId\":\"mode\""), "{config}");
    let prompts = std::fs::read_to_string(prompt_log).expect("prompt log");
    assert!(prompts.contains("Project rubric"), "{prompts}");
    assert!(prompts.contains("Question: Does it pass?"), "{prompts}");
}

#[test]
fn configured_direct_mode_sends_one_multimodal_codex_prompt() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let config = temp.path().join("config.toml");
    let kind_log = temp.path().join("prompt-kind.log");
    std::fs::write(
        &config,
        format!(
            r#"
mode = "direct"

[rubric]
backend = "{}"
model = "gpt-5.5"
effort = "medium"
"#,
            fake.display()
        ),
    )
    .expect("write config");

    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("configured")
        .arg("--config")
        .arg(&config)
        .arg("--image")
        .arg(&image)
        .arg("--question")
        .arg("Does it pass?")
        .arg("--json")
        .env("FAKE_CODEX_ACP_MODE", "pass")
        .env("FAKE_CODEX_ACP_PROMPT_KIND_LOG", &kind_log)
        .output()
        .expect("run visual-rubric configured");

    assert!(output.status.success(), "{output:?}");
    let prompt_kind = std::fs::read_to_string(kind_log).expect("prompt kind log");
    assert!(prompt_kind.contains("has_image=true"), "{prompt_kind}");
}

#[test]
fn direct_codex_gpt_config_uses_subscription_defaults() {
    let config = direct_codex_gpt_config(None, None);

    assert_eq!(config.mode, Some(ConfigMode::Direct));
    assert_eq!(config.rubric.backend.as_deref(), Some("codex-acp"));
    assert_eq!(config.rubric.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(config.rubric.effort.as_deref(), Some("medium"));
    assert!(config.vision.url.is_none());
}

#[test]
fn public_api_accepts_custom_binary_env_and_cwd() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let cwd = temp.path().join("cwd");
    std::fs::create_dir(&cwd).expect("cwd");
    let cwd_log = temp.path().join("cwd.log");
    let env_log = temp.path().join("env.log");

    let verdict = evaluate_image_rubric_with_config(
        &image,
        "Does it pass?",
        RubricOptions::default(),
        RubricRunConfig {
            codex_acp_binary: fake,
            acp_args: build_codex_acp_args("gpt-5.4-mini", "medium"),
            url: None,
            api_model: None,
            extra_env: vec![
                (
                    OsString::from("FAKE_CODEX_ACP_MODE"),
                    OsString::from("pass"),
                ),
                (
                    OsString::from("FAKE_CODEX_ACP_CWD_LOG"),
                    cwd_log.as_os_str().to_os_string(),
                ),
                (
                    OsString::from("FAKE_CODEX_ACP_ENV_LOG"),
                    env_log.as_os_str().to_os_string(),
                ),
                (
                    OsString::from("FAKE_CODEX_ACP_CUSTOM_ENV"),
                    OsString::from("custom-value"),
                ),
            ],
            cwd: Some(cwd.clone()),
        },
    )
    .expect("verdict");

    assert_eq!(verdict.verdict, "pass");
    assert_eq!(
        std::fs::read_to_string(cwd_log).unwrap().trim(),
        cwd.to_string_lossy()
    );
    assert_eq!(
        std::fs::read_to_string(env_log).unwrap().trim(),
        "custom-value"
    );
}

#[test]
fn public_api_forwards_custom_acp_args() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let args_log = temp.path().join("args.log");

    let verdict = evaluate_image_rubric_with_config(
        &image,
        "Does it pass?",
        RubricOptions::default(),
        RubricRunConfig {
            codex_acp_binary: fake,
            acp_args: vec!["acp".to_string()],
            url: None,
            api_model: None,
            extra_env: vec![
                (
                    OsString::from("FAKE_CODEX_ACP_MODE"),
                    OsString::from("pass"),
                ),
                (
                    OsString::from("FAKE_CODEX_ACP_ARG_LOG"),
                    args_log.as_os_str().to_os_string(),
                ),
            ],
            cwd: None,
        },
    )
    .expect("verdict");

    assert_eq!(verdict.verdict, "pass");
    let args = std::fs::read_to_string(args_log).expect("args log");
    assert!(args.lines().any(|line| line == "acp"), "{args}");
    assert!(!args.contains("model=\""), "{args}");
}
