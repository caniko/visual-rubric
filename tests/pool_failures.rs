mod common;

use std::ffi::OsString;
use std::time::Duration;

use visual_rubric::{PoolConfig, PoolError, RubricOptions, RubricPool};

#[test]
fn rate_limit_records_retry_after_and_exhausts_retries() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        max_retries: 1,
        backoff_base: Duration::ZERO,
        backoff_cap: Duration::ZERO,
        codex_acp_binary: env!("CARGO_BIN_EXE_fake-codex-acp").into(),
        extra_env: vec![(
            OsString::from("FAKE_CODEX_ACP_MODE"),
            OsString::from("rate_limit"),
        )],
        ..PoolConfig::default()
    })
    .expect("pool");

    let err = pool
        .submit(&image, "rate limit", RubricOptions::default())
        .expect_err("rate limit");
    assert_eq!(
        err,
        PoolError::RateLimited {
            retry_after: Some(Duration::from_secs(3))
        }
    );
    let stats = pool.stats();
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.failures, 1);
    assert_eq!(stats.rate_limit_events.len(), 2);
    assert_eq!(
        stats.rate_limit_events[0].retry_after,
        Some(Duration::from_secs(3))
    );
    let _ = pool.shutdown();
}

#[test]
fn validates_worker_count_bounds() {
    let err = match RubricPool::new(PoolConfig {
        workers: 0,
        ..PoolConfig::default()
    }) {
        Ok(_) => panic!("zero workers should fail"),
        Err(error) => error,
    };
    assert!(matches!(err, PoolError::Spawn(message) if message.contains("greater than zero")));

    let err = match RubricPool::new(PoolConfig {
        workers: 65,
        ..PoolConfig::default()
    }) {
        Ok(_) => panic!("too many workers should fail"),
        Err(error) => error,
    };
    assert!(matches!(err, PoolError::Spawn(message) if message.contains("alive bitmask")));
}

#[test]
fn spawn_failure_is_reported() {
    let err = match RubricPool::new(PoolConfig {
        workers: 1,
        codex_acp_binary: "/definitely/missing/codex-acp".into(),
        ..PoolConfig::default()
    }) {
        Ok(_) => panic!("spawn should fail"),
        Err(error) => error,
    };
    assert!(matches!(err, PoolError::Spawn(message) if message.contains("definitely/missing")));
}

#[cfg(unix)]
#[test]
fn worker_crash_can_leave_no_live_workers() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let wrapper = temp.path().join("stateful-fake-acp");
    let counter = temp.path().join("spawn-count");
    write_stateful_crash_wrapper(
        &wrapper,
        env!("CARGO_BIN_EXE_fake-codex-acp"),
        counter.to_str().unwrap(),
    );
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        max_retries: 0,
        codex_acp_binary: wrapper,
        ..PoolConfig::default()
    })
    .expect("pool");

    let first = pool
        .submit(&image, "crash", RubricOptions::default())
        .expect_err("worker crash");
    assert!(matches!(first, PoolError::WorkerCrashed { .. }));

    let second = pool
        .submit(&image, "no workers", RubricOptions::default())
        .expect_err("no live workers");
    assert_eq!(second, PoolError::NoLiveWorkers);
    let _ = pool.shutdown();
}

#[cfg(unix)]
fn write_stateful_crash_wrapper(path: &std::path::Path, fake: &str, counter: &str) {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let mut file = std::fs::File::create(path).expect("wrapper");
    writeln!(
        file,
        r#"#!{}
set -eu
counter={counter:?}
fake={fake:?}
n=0
if [ -f "$counter" ]; then
  n="$(cat "$counter")"
fi
n=$((n + 1))
printf '%s' "$n" > "$counter"
if [ "$n" = 1 ]; then
  export FAKE_CODEX_ACP_MODE=prompt_crash
else
  export FAKE_CODEX_ACP_MODE=crash
fi
exec "$fake" "$@"
"#,
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    )
    .expect("write wrapper");
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn missing_png_path_is_reported() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let missing = temp.path().join("missing.png");
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        max_retries: 0,
        codex_acp_binary: env!("CARGO_BIN_EXE_fake-codex-acp").into(),
        extra_env: vec![(
            OsString::from("FAKE_CODEX_ACP_MODE"),
            OsString::from("pass"),
        )],
        ..PoolConfig::default()
    })
    .expect("pool");

    let err = pool
        .submit(&missing, "missing", RubricOptions::default())
        .expect_err("missing image");
    assert!(matches!(err, PoolError::Rpc(message) if message.contains("read png")));
    let _ = pool.shutdown();
}

#[test]
fn worker_codex_home_is_seeded_from_source_home() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let source_home = temp.path().join("source-codex");
    std::fs::create_dir(&source_home).expect("source home");
    std::fs::write(source_home.join("config.toml"), "seeded = true\n").expect("seed config");
    let env_log = temp.path().join("codex-home.log");
    let image = common::write_fixture_png(&temp);
    let pool = RubricPool::new(PoolConfig {
        workers: 1,
        codex_acp_binary: env!("CARGO_BIN_EXE_fake-codex-acp").into(),
        source_codex_home: Some(source_home),
        extra_env: vec![
            (
                OsString::from("FAKE_CODEX_ACP_MODE"),
                OsString::from("pass"),
            ),
            (
                OsString::from("FAKE_CODEX_ACP_ENV_LOG"),
                env_log.as_os_str().to_os_string(),
            ),
            (
                OsString::from("FAKE_CODEX_ACP_LOG_ENV_KEY"),
                OsString::from("CODEX_HOME"),
            ),
        ],
        ..PoolConfig::default()
    })
    .expect("pool");

    let verdict = pool
        .submit(&image, "pass", RubricOptions::default())
        .expect("verdict");
    assert_eq!(verdict.verdict, "pass");
    let worker_home = std::fs::read_to_string(env_log).expect("env log");
    let seeded_config =
        std::fs::read_to_string(std::path::Path::new(worker_home.trim()).join("config.toml"))
            .expect("seeded worker config");
    assert!(seeded_config.contains("seeded = true"));
    let _ = pool.shutdown();
}
