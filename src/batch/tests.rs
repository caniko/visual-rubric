use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::{LogCaptureConfig, LogPathMode, RubricVerdictStatus};

use super::*;

fn snapshot(entries: &[(&str, &str)]) -> AssetSnapshot {
    AssetSnapshot::from_hashes(entries.iter().map(|(path, hash)| (*path, *hash)))
}

#[test]
fn diff_snapshots_classifies_added_changed_deleted_and_unchanged() {
    let before = snapshot(&[
        ("deleted.png", "a"),
        ("changed.png", "a"),
        ("same.png", "a"),
    ]);
    let after = snapshot(&[("added.png", "a"), ("changed.png", "b"), ("same.png", "a")]);

    let changes = diff_snapshots(&before, &after);

    assert_eq!(
        changes,
        vec![
            AssetChange::Added(PathBuf::from("added.png")),
            AssetChange::Changed(PathBuf::from("changed.png")),
            AssetChange::Deleted(PathBuf::from("deleted.png")),
            AssetChange::Unchanged(PathBuf::from("same.png")),
        ]
    );
}

#[test]
fn select_changed_respects_selection_mode() {
    let changes = vec![
        AssetChange::Added(PathBuf::from("added.png")),
        AssetChange::Changed(PathBuf::from("changed.png")),
        AssetChange::Deleted(PathBuf::from("deleted.png")),
        AssetChange::Unchanged(PathBuf::from("same.png")),
    ];

    assert_eq!(
        select_changed(&changes, SelectionMode::ChangedOnly),
        vec![PathBuf::from("added.png"), PathBuf::from("changed.png")]
    );
    assert_eq!(
        select_changed(&changes, SelectionMode::IncludeUnchanged),
        vec![
            PathBuf::from("added.png"),
            PathBuf::from("changed.png"),
            PathBuf::from("same.png"),
        ]
    );
}

#[test]
fn aggregate_status_prefers_error_then_fail_then_pass_then_skipped() {
    let skipped = vec![AssetRubricReport::skipped(
        Path::new("a.png"),
        "unchanged",
        "same",
    )];
    assert_eq!(aggregate_status(&skipped), AggregateStatus::Skipped);

    let pass = vec![AssetRubricReport::selected(
        Path::new("a.png"),
        "changed",
        AssetRubricResult::Pass {
            reason: "ok".to_owned(),
            anomalies: Vec::new(),
        },
    )];
    assert_eq!(aggregate_status(&pass), AggregateStatus::Pass);

    let fail = vec![AssetRubricReport::selected(
        Path::new("a.png"),
        "changed",
        AssetRubricResult::Fail {
            reason: "bad".to_owned(),
            anomalies: Vec::new(),
        },
    )];
    assert_eq!(aggregate_status(&fail), AggregateStatus::Fail);

    let error = vec![AssetRubricReport::selected(
        Path::new("a.png"),
        "changed",
        AssetRubricResult::Error {
            message: "worker failed".to_owned(),
        },
    )];
    assert_eq!(aggregate_status(&error), AggregateStatus::Error);
}

struct FakeEvaluator {
    results: Mutex<Vec<Result<RubricVerdict, PoolError>>>,
}

impl BatchEvaluator for FakeEvaluator {
    fn submit_asset(&self, _png_path: &Path, _question: &str) -> Result<RubricVerdict, PoolError> {
        self.results.lock().expect("test evaluator mutex").remove(0)
    }
}

#[test]
fn batch_partial_error_preserves_completed_and_marks_remaining() {
    let changes = vec![
        AssetChange::Changed(PathBuf::from("a.png")),
        AssetChange::Changed(PathBuf::from("b.png")),
        AssetChange::Changed(PathBuf::from("c.png")),
    ];
    let evaluator = FakeEvaluator {
        results: Mutex::new(vec![
            Ok(RubricVerdict {
                verdict: RubricVerdictStatus::from("pass"),
                reason: "ok".to_owned(),
                anomalies: Vec::new(),
            }),
            Err(PoolError::Timeout {
                worker_id: 0,
                timeout: Duration::from_secs(180),
            }),
        ]),
    };
    let run = BatchRubricRun::new(BatchRubricConfig {
        pool: PoolConfig {
            workers: 1,
            ..PoolConfig::default()
        },
        question: "question".to_owned(),
        selection_mode: SelectionMode::ChangedOnly,
        cache_dir: None,
        classifier: None,
    });

    let report = run.run_with_evaluator(&changes, Some(&evaluator));

    assert_eq!(report.aggregate_status, AggregateStatus::Error);
    assert!(matches!(
        report.assets[0].result,
        AssetRubricResult::Pass { .. }
    ));
    assert!(matches!(
        report.assets[1].result,
        AssetRubricResult::Error { .. }
    ));
    assert!(matches!(
        report.assets[2].result,
        AssetRubricResult::NotEvaluatedAfterError { .. }
    ));
}

struct ConcurrentEvaluator {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl BatchEvaluator for ConcurrentEvaluator {
    fn submit_asset(&self, _png_path: &Path, _question: &str) -> Result<RubricVerdict, PoolError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(RubricVerdict {
            verdict: RubricVerdictStatus::from("pass"),
            reason: "ok".to_owned(),
            anomalies: Vec::new(),
        })
    }
}

#[test]
fn batch_keeps_multiple_evaluations_in_flight() {
    let changes = vec![
        AssetChange::Changed(PathBuf::from("a.png")),
        AssetChange::Changed(PathBuf::from("b.png")),
        AssetChange::Changed(PathBuf::from("c.png")),
    ];
    let evaluator = Arc::new(ConcurrentEvaluator {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    });
    let run = BatchRubricRun::new(BatchRubricConfig {
        pool: PoolConfig {
            workers: 2,
            ..PoolConfig::default()
        },
        question: "question".to_owned(),
        selection_mode: SelectionMode::ChangedOnly,
        cache_dir: None,
        classifier: None,
    });

    let report = run.run_with_evaluator(&changes, Some(evaluator.as_ref()));

    assert_eq!(report.aggregate_status, AggregateStatus::Pass);
    assert_eq!(report.assets.len(), 3);
    assert!(evaluator.max_active.load(Ordering::SeqCst) >= 2);
}

struct CountingEvaluator {
    calls: AtomicUsize,
    verdict: RubricVerdict,
}

impl BatchEvaluator for CountingEvaluator {
    fn submit_asset(&self, _png_path: &Path, _question: &str) -> Result<RubricVerdict, PoolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.verdict.clone())
    }
}

#[test]
fn batch_reuses_content_addressed_cache_without_resubmitting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let image = temp.path().join("screen.png");
    fs::write(&image, b"stable screenshot bytes").expect("image");
    let changes = vec![AssetChange::Changed(image)];
    let evaluator = CountingEvaluator {
        calls: AtomicUsize::new(0),
        verdict: RubricVerdict {
            verdict: RubricVerdictStatus::from("pass"),
            reason: "cached pass".to_owned(),
            anomalies: Vec::new(),
        },
    };
    let run = BatchRubricRun::new(BatchRubricConfig {
        pool: PoolConfig {
            workers: 1,
            ..PoolConfig::default()
        },
        question: "question".to_owned(),
        selection_mode: SelectionMode::ChangedOnly,
        cache_dir: Some(temp.path().join("cache")),
        classifier: None,
    });

    let first = run.run_with_evaluator(&changes, Some(&evaluator));
    let second = run.run_with_evaluator(&changes, Some(&evaluator));

    assert_eq!(first.cache_hits, 0);
    assert_eq!(first.cache_misses, 1);
    assert_eq!(second.cache_hits, 1);
    assert_eq!(second.cache_misses, 0);
    assert_eq!(evaluator.calls.load(Ordering::SeqCst), 1);
    assert_eq!(second.aggregate_status, AggregateStatus::Pass);
}

#[test]
fn log_capture_copies_sqlite_wal_and_session_jsonl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("tmp");
    let output = temp.path().join("logs");
    let session_dir = source.join("worker/sessions/2026/06/08");
    fs::create_dir_all(&session_dir).expect("session dir");
    fs::write(source.join("logs_1.sqlite"), "sqlite").expect("sqlite");
    fs::write(source.join("logs_1.sqlite-wal"), "wal").expect("wal");
    fs::write(session_dir.join("run.jsonl"), "{}").expect("jsonl");
    fs::write(source.join("ignore.txt"), "ignore").expect("ignore");

    let logs = copy_logs_from_config(Some(&LogCaptureConfig {
        temp_dir: source,
        output_dir: output,
        path_mode: LogPathMode::RelativeTo(temp.path().to_path_buf()),
    }))
    .expect("copy logs");

    assert_eq!(
        logs,
        vec![
            "logs/logs_1.sqlite".to_owned(),
            "logs/logs_1.sqlite-wal".to_owned(),
            "logs/worker/sessions/2026/06/08/run.jsonl".to_owned(),
        ]
    );
}

#[test]
fn log_capture_copy_error_is_reported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("tmp");
    fs::create_dir_all(&source).expect("source dir");
    fs::write(source.join("logs_1.sqlite"), "sqlite").expect("sqlite");
    let output_file = temp.path().join("logs");
    fs::write(&output_file, "not a directory").expect("output file");

    let run = BatchRubricRun::new(BatchRubricConfig {
        pool: PoolConfig {
            workers: 1,
            log_capture: Some(LogCaptureConfig {
                temp_dir: source,
                output_dir: output_file,
                path_mode: LogPathMode::Absolute,
            }),
            ..PoolConfig::default()
        },
        question: "question".to_owned(),
        selection_mode: SelectionMode::ChangedOnly,
        cache_dir: None,
        classifier: None,
    });

    let report = run.run(&[]);

    assert!(report.logs.is_empty());
    let error = report
        .log_capture_error
        .as_deref()
        .expect("log capture error");
    assert!(error.contains("copy configured ACP logs"));
}

struct TestClassifier;

impl IssueClassifier for TestClassifier {
    fn classify(&self, input: IssueClassificationInput<'_>) -> Vec<IssueRecommendation> {
        vec![IssueRecommendation {
            id: "text-clipping".to_owned(),
            class: "text_clipping".to_owned(),
            severity: RecommendationSeverity::Medium,
            affected_assets: vec![input.asset.path.clone()],
            evidence: vec![input.issue_text.to_owned()],
            suggested_fix: "reserve more margin".to_owned(),
            candidate_modules: vec!["renderer".to_owned()],
        }]
    }
}

#[test]
fn classifier_recommendations_are_merged_by_id() {
    let assets = vec![
        AssetRubricReport::selected(
            Path::new("a.png"),
            "changed",
            AssetRubricResult::Fail {
                reason: "label clipped".to_owned(),
                anomalies: Vec::new(),
            },
        ),
        AssetRubricReport::selected(
            Path::new("b.png"),
            "changed",
            AssetRubricResult::Fail {
                reason: "text clipped".to_owned(),
                anomalies: Vec::new(),
            },
        ),
    ];

    let recommendations = classify_recommendations(Some(&TestClassifier), &assets);

    assert_eq!(recommendations.len(), 1);
    assert_eq!(
        recommendations[0].affected_assets,
        vec!["a.png".to_owned(), "b.png".to_owned()]
    );
}
