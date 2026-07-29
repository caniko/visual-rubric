#![cfg(feature = "pool")]

mod common;

use std::ffi::OsString;

use visual_rubric::{PoolConfig, RubricOptions, RubricPool};

#[test]
fn pool_recycles_after_prompt_limit() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let args_log = temp.path().join("args.log");
    let config_log = temp.path().join("config.log");
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let pool = RubricPool::new(PoolConfig {
        workers: 2,
        max_prompts_per_worker: 2,
        codex_acp_binary: fake,
        extra_env: vec![
            (
                OsString::from("FAKE_CODEX_ACP_MODE"),
                OsString::from("pass"),
            ),
            (
                OsString::from("FAKE_CODEX_ACP_ARG_LOG"),
                args_log.as_os_str().to_os_string(),
            ),
            (
                OsString::from("FAKE_CODEX_ACP_CONFIG_LOG"),
                config_log.as_os_str().to_os_string(),
            ),
        ],
        ..PoolConfig::default()
    })
    .expect("spawn fake rubric pool");

    for _ in 0..5 {
        let verdict = pool
            .submit(&image, "fake pass", RubricOptions::default())
            .expect("rubric verdict");
        assert_eq!(verdict.verdict, "pass");
    }

    let stats = pool.shutdown();
    assert_eq!(stats.completed, 5);
    assert_eq!(stats.failures, 0);
    assert!(
        stats.worker_recycles >= 1,
        "expected at least one recycle, got {}",
        stats.worker_recycles
    );
    let args = std::fs::read_to_string(args_log).expect("ACP argument log");
    assert!(
        !args.lines().any(|line| line == "-c"),
        "legacy adapter arguments leaked: {args}"
    );
    assert!(
        !args.lines().any(|line| line.starts_with("model=")),
        "legacy model argument leaked: {args}"
    );
    let config = std::fs::read_to_string(config_log).expect("ACP config log");
    assert!(config.contains("\"configId\":\"model\""), "{config}");
    assert!(
        config.contains("\"configId\":\"reasoning_effort\""),
        "{config}"
    );
}

#[test]
fn pool_uses_custom_system_prompt() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let prompt_log = temp.path().join("prompts.log");
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        codex_acp_binary: fake,
        extra_env: vec![
            (
                OsString::from("FAKE_CODEX_ACP_MODE"),
                OsString::from("pass"),
            ),
            (
                OsString::from("FAKE_CODEX_ACP_PROMPT_LOG"),
                prompt_log.as_os_str().to_os_string(),
            ),
        ],
        ..PoolConfig::default()
    })
    .expect("spawn fake rubric pool");

    let verdict = pool
        .submit(
            &image,
            "fake pass",
            RubricOptions {
                system_prompt: Some("Project-specific UX rubric".to_string()),
                ..RubricOptions::default()
            },
        )
        .expect("rubric verdict");
    assert_eq!(verdict.verdict, "pass");

    let stats = pool.shutdown();
    assert_eq!(stats.completed, 1);
    let prompts = std::fs::read_to_string(prompt_log).expect("prompt log");
    assert!(prompts.contains("Project-specific UX rubric"));
    assert!(prompts.contains("Question: fake pass"));
}
