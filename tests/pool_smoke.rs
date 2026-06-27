#![cfg(all(feature = "codex-acp", feature = "pool"))]

mod common;

use std::time::{Duration, Instant};

use visual_rubric::{PoolConfig, RubricOptions, RubricPool};

#[test]
fn pool_smoke_parallel_real_codex_acp() {
    if std::env::var_os("VISUAL_RUBRIC_REAL_ACP").is_none() {
        eprintln!("skipping: set VISUAL_RUBRIC_REAL_ACP=1 to run real codex-acp smoke test");
        return;
    }
    if which::which("codex-acp").is_err() {
        eprintln!("skipping: codex-acp not on PATH");
        return;
    }

    let temp = tempfile::TempDir::new().expect("tempdir");
    let image = common::write_fixture_png(&temp);
    let pool = RubricPool::new(PoolConfig {
        workers: 2,
        ..PoolConfig::default()
    })
    .expect("spawn rubric pool");

    let started = Instant::now();
    let results = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..4 {
            let image = image.clone();
            let pool = &pool;
            handles.push(scope.spawn(move || {
                pool.submit(
                    &image,
                    "This is a valid visual-test fixture image. Return pass unless the image data is unreadable.",
                    RubricOptions::default(),
                )
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("rubric submit thread"))
            .collect::<Vec<_>>()
    });

    for result in results {
        let verdict = result.expect("rubric verdict");
        assert_eq!(verdict.verdict, "pass");
    }
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "parallel rubric run exceeded 60s: {:?}",
        started.elapsed()
    );

    let stats = pool.shutdown();
    assert_eq!(stats.completed, 4);
    assert_eq!(stats.failures, 0);
}
