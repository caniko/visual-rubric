mod common;

use std::ffi::OsString;

use visual_rubric::{PoolConfig, PoolError, RubricOptions, RubricPool};

#[test]
fn quota_exceeded_is_fatal_for_later_submits() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let spawn_log = temp.path().join("spawns.log");
    let fake = env!("CARGO_BIN_EXE_fake-codex-acp");
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        max_retries: 0,
        codex_acp_binary: fake.into(),
        extra_env: vec![(
            OsString::from("FAKE_CODEX_ACP_SPAWN_LOG"),
            spawn_log.as_os_str().to_os_string(),
        )],
        ..PoolConfig::default()
    })
    .expect("spawn fake rubric pool");

    let first = pool
        .submit(&image, "trigger quota", RubricOptions::default())
        .expect_err("first submit should hit quota");
    assert_eq!(first, PoolError::QuotaExceeded);

    let second = pool
        .submit(
            &image,
            "should not spawn new work",
            RubricOptions::default(),
        )
        .expect_err("second submit should reuse fatal quota state");
    assert_eq!(second, PoolError::QuotaExceeded);

    let stats = pool.shutdown();
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.failures, 1);

    let spawn_count = std::fs::read_to_string(spawn_log)
        .expect("spawn log")
        .lines()
        .count();
    assert_eq!(spawn_count, 1);
}
