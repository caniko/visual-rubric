#![cfg(feature = "codex-acp")]

mod common;

use std::ffi::OsString;
use std::process::Command;

use visual_rubric::{
    ConfigMode, PoolError, RubricError, RubricOptions, RubricRunConfig, SequenceFrame,
    SequenceOptions, direct_codex_gpt_config, evaluate_image_rubric_with_config,
    evaluate_image_sequence_rubric_with_options,
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
fn sequence_api_honors_custom_and_default_system_prompts() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let frames = ["before", "after"].map(|label| SequenceFrame {
        label: label.to_owned(),
        path: image.clone(),
    });
    for custom in [Some("Project-specific sequence rubric"), None] {
        let prompt_log = temp.path().join(if custom.is_some() {
            "custom.log"
        } else {
            "default.log"
        });
        let verdict = evaluate_image_sequence_rubric_with_options(
            &frames,
            "Did the menu open?",
            RubricOptions {
                system_prompt: custom.map(str::to_owned),
                ..RubricOptions::default()
            },
            RubricRunConfig {
                codex_acp_binary: fake.clone(),
                extra_env: vec![
                    (
                        OsString::from("FAKE_CODEX_ACP_MODE"),
                        OsString::from("pass"),
                    ),
                    (
                        OsString::from("FAKE_CODEX_ACP_PROMPT_LOG"),
                        prompt_log.as_os_str().to_owned(),
                    ),
                ],
                ..RubricRunConfig::default()
            },
            SequenceOptions::default(),
        )
        .expect("sequence verdict");
        assert_eq!(verdict.verdict, "pass");
        let prompt = std::fs::read_to_string(prompt_log).expect("prompt log");
        assert!(
            prompt.contains(custom.unwrap_or(visual_rubric::DEFAULT_SYSTEM_PROMPT)),
            "{prompt}"
        );
        assert!(prompt.contains("Question: Did the menu open?"), "{prompt}");
        assert!(prompt.contains("before/after transition"), "{prompt}");
        if custom.is_some() {
            assert!(
                !prompt.contains(visual_rubric::DEFAULT_SYSTEM_PROMPT),
                "{prompt}"
            );
        }
    }
}

#[test]
fn sequence_cli_forwards_preset_system_prompt() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let prompt_log = temp.path().join("prompts.log");
    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("sequence")
        .arg("--frame")
        .arg(format!("before={}", image.display()))
        .arg("--frame")
        .arg(format!("after={}", image.display()))
        .args(["--preset", "website-install", "--max-frames", "2"])
        .arg("--codex-acp")
        .arg(fake)
        .arg("--json")
        .env("XDG_CONFIG_HOME", temp.path())
        .env("FAKE_CODEX_ACP_MODE", "pass")
        .env("FAKE_CODEX_ACP_PROMPT_LOG", &prompt_log)
        .output()
        .expect("run sequence CLI");
    assert!(output.status.success(), "{output:?}");
    let prompt = std::fs::read_to_string(prompt_log).expect("prompt log");
    assert!(
        prompt.contains(visual_rubric::presets::WEBSITE_INSTALL_SYSTEM_PROMPT),
        "{prompt}"
    );
}

#[test]
fn sequence_rejects_excess_frames_before_reading_files() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let frames = ["before", "during", "after"].map(|label| SequenceFrame {
        label: label.to_owned(),
        path: temp.path().join(format!("missing-{label}.png")),
    });
    let result = evaluate_image_sequence_rubric_with_options(
        &frames,
        "Did the menu open?",
        RubricOptions::default(),
        RubricRunConfig::default(),
        SequenceOptions {
            max_frames: 2,
            require_transition: true,
        },
    );
    assert!(
        matches!(result, Err(RubricError::Pool(PoolError::Rpc(ref message)))
        if message == "sequence contains 3 frames, maximum is 2"),
        "{result:?}"
    );
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
            acp_args: Vec::new(),
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
