mod logs;
mod recommendations;
mod snapshot;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{PoolConfig, PoolError, RubricOptions, RubricPool, RubricVerdict};

use logs::copy_logs_from_config;
use recommendations::{aggregate_status, classify_recommendations};

pub use snapshot::{AssetChange, AssetSnapshot, SelectionMode, diff_snapshots, select_changed};

const REPORT_SCHEMA_VERSION: u32 = 2;
const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_KEY_DOMAIN: &[u8] = b"visual-rubric.batch.cache.v1";

/// Overall status for a batch rubric report.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AggregateStatus {
    /// At least one selected asset had an evaluation error or was not evaluated after a pool error.
    Error,
    /// At least one selected asset failed the rubric.
    Fail,
    /// At least one selected asset passed and no selected asset failed or errored.
    Pass,
    /// No asset was selected for evaluation.
    Skipped,
}

/// Serializable batch rubric report.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BatchRubricReport {
    /// Version of this JSON report schema.
    pub schema_version: u32,
    /// Aggregate status computed as `error > fail > pass > skipped`.
    pub aggregate_status: AggregateStatus,
    /// UNIX timestamp in seconds when evaluation started.
    pub started_at: String,
    /// UNIX timestamp in seconds when evaluation finished.
    pub finished_at: String,
    /// Worker count requested for the batch.
    pub workers: usize,
    /// Number of selected assets served by the persistent cache.
    #[serde(default)]
    pub cache_hits: u64,
    /// Number of selected assets that missed the persistent cache.
    #[serde(default)]
    pub cache_misses: u64,
    /// Effective default options supplied to the pool.
    pub options: RubricOptions,
    /// Copied ACP log paths.
    pub logs: Vec<String>,
    /// Log capture failure, when ACP logs were requested but could not be copied.
    pub log_capture_error: Option<String>,
    /// Per-asset results, including skipped unchanged/deleted assets.
    pub assets: Vec<AssetRubricReport>,
    /// Optional caller-classified recommendations derived from failed/error assets.
    pub recommendations: Vec<IssueRecommendation>,
}

/// Per-asset report item.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AssetRubricReport {
    /// Asset path, using caller-provided path representation.
    pub path: String,
    /// Snapshot change status.
    pub change: String,
    /// Whether this asset was submitted to the rubric evaluator.
    pub selected: bool,
    /// Rubric evaluation result or skipped reason.
    pub result: AssetRubricResult,
}

/// Per-asset rubric result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AssetRubricResult {
    /// Rubric passed.
    Pass {
        /// Verdict reason.
        reason: String,
        /// Reported anomalies.
        anomalies: Vec<String>,
    },
    /// Rubric failed.
    Fail {
        /// Verdict reason.
        reason: String,
        /// Reported anomalies.
        anomalies: Vec<String>,
    },
    /// Rubric evaluation errored.
    Error {
        /// Error message.
        message: String,
    },
    /// Asset was selected but not evaluated after a fatal pool-level error.
    NotEvaluatedAfterError {
        /// Root error that aborted the remaining batch.
        root_error: String,
        /// Suggested retry action.
        retry_hint: String,
    },
    /// Asset was not selected for evaluation.
    Skipped {
        /// Skip reason.
        reason: String,
    },
}

/// Severity for a caller-provided recommendation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationSeverity {
    /// Low severity.
    Low,
    /// Medium severity.
    Medium,
    /// High severity.
    High,
}

/// Caller-provided issue recommendation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IssueRecommendation {
    /// Stable recommendation id.
    pub id: String,
    /// Machine-readable recommendation class.
    pub class: String,
    /// Recommendation severity.
    pub severity: RecommendationSeverity,
    /// Affected asset paths.
    pub affected_assets: Vec<String>,
    /// Short evidence snippets.
    pub evidence: Vec<String>,
    /// Suggested generic fix.
    pub suggested_fix: String,
    /// Candidate modules or subsystems, if known to the caller.
    pub candidate_modules: Vec<String>,
}

/// Input passed to caller-provided issue classifiers.
#[derive(Debug)]
pub struct IssueClassificationInput<'a> {
    /// Failed or errored asset report.
    pub asset: &'a AssetRubricReport,
    /// Combined failure text available for classification.
    pub issue_text: &'a str,
}

/// Optional caller hook for mapping failed/error assets to reusable recommendations.
pub trait IssueClassifier {
    /// Classifies one failed/error asset.
    fn classify(&self, input: IssueClassificationInput<'_>) -> Vec<IssueRecommendation>;
}

/// Batch rubric runner configuration.
pub struct BatchRubricConfig<'a> {
    /// Pool configuration.
    pub pool: PoolConfig,
    /// Question sent for every selected asset.
    pub question: String,
    /// Selection mode for unchanged assets.
    pub selection_mode: SelectionMode,
    /// Optional content-addressed cache directory for completed verdicts.
    pub cache_dir: Option<PathBuf>,
    /// Optional issue classifier.
    pub classifier: Option<&'a dyn IssueClassifier>,
}

impl std::fmt::Debug for BatchRubricConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchRubricConfig")
            .field("pool", &self.pool)
            .field("question", &self.question)
            .field("selection_mode", &self.selection_mode)
            .field("cache_dir", &self.cache_dir)
            .field("classifier", &self.classifier.map(|_| "<classifier>"))
            .finish()
    }
}

/// Generic batch runner around [`RubricPool`].
#[derive(Debug)]
pub struct BatchRubricRun<'a> {
    config: BatchRubricConfig<'a>,
}

impl<'a> BatchRubricRun<'a> {
    /// Creates a batch runner.
    #[must_use]
    pub fn new(config: BatchRubricConfig<'a>) -> Self {
        Self { config }
    }

    /// Evaluates selected assets from a snapshot diff.
    ///
    /// Pool startup and per-asset failures are represented inside the returned
    /// report so callers can serialize the report before deciding whether to
    /// fail their own command.
    #[must_use]
    pub fn run(&self, changes: &[AssetChange]) -> BatchRubricReport {
        self.run_with_evaluator(changes, None)
    }

    fn run_with_evaluator(
        &self,
        changes: &[AssetChange],
        evaluator: Option<&dyn BatchEvaluator>,
    ) -> BatchRubricReport {
        let started_at = unix_timestamp();
        let selected = select_changed(changes, self.config.selection_mode);
        let mut assets = Vec::with_capacity(changes.len());
        let selected_evaluation = if selected.is_empty() {
            SelectedEvaluation::default()
        } else if let Some(evaluator) = evaluator {
            evaluate_selected(
                evaluator,
                changes,
                &selected,
                &self.config.question,
                self.config.pool.workers,
                self.config.cache_dir.as_deref(),
                &self.config.pool.default_options,
            )
        } else {
            match RubricPool::new(self.config.pool.clone()) {
                Ok(pool) => {
                    let evaluation = evaluate_selected(
                        &pool,
                        changes,
                        &selected,
                        &self.config.question,
                        self.config.pool.workers,
                        self.config.cache_dir.as_deref(),
                        &self.config.pool.default_options,
                    );
                    if evaluation.aborted {
                        drop(pool);
                    } else {
                        let _stats = pool.shutdown();
                    }
                    evaluation
                }
                Err(error) => selected
                    .iter()
                    .map(|path| {
                        AssetRubricReport::selected(
                            path,
                            status_for_selected(path, changes),
                            AssetRubricResult::Error {
                                message: format!("start visual-rubric worker pool: {error}"),
                            },
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
            }
        };
        assets.extend(selected_evaluation.reports);
        assets.extend(skipped_asset_reports(changes, self.config.selection_mode));
        assets.sort_by(|left, right| left.path.cmp(&right.path));
        let (logs, log_capture_error) =
            match copy_logs_from_config(self.config.pool.log_capture.as_ref()) {
                Ok(logs) => (logs, None),
                Err(error) => (
                    Vec::new(),
                    Some(format!(
                        "copy configured ACP logs into batch report: {error}"
                    )),
                ),
            };
        let recommendations = classify_recommendations(self.config.classifier, &assets);
        let aggregate_status = aggregate_status(&assets);
        BatchRubricReport {
            schema_version: REPORT_SCHEMA_VERSION,
            aggregate_status,
            started_at,
            finished_at: unix_timestamp(),
            workers: self.config.pool.workers,
            cache_hits: selected_evaluation.cache_hits,
            cache_misses: selected_evaluation.cache_misses,
            options: self.config.pool.default_options.clone(),
            logs,
            log_capture_error,
            assets,
            recommendations,
        }
    }
}

trait BatchEvaluator: Sync {
    fn submit_asset(&self, png_path: &Path, question: &str) -> Result<RubricVerdict, PoolError>;
}

impl BatchEvaluator for RubricPool {
    fn submit_asset(&self, png_path: &Path, question: &str) -> Result<RubricVerdict, PoolError> {
        self.submit(png_path, question, RubricOptions::default())
    }
}

#[derive(Default)]
struct SelectedEvaluation {
    reports: Vec<AssetRubricReport>,
    aborted: bool,
    cache_hits: u64,
    cache_misses: u64,
}

impl From<Vec<AssetRubricReport>> for SelectedEvaluation {
    fn from(reports: Vec<AssetRubricReport>) -> Self {
        Self {
            reports,
            aborted: false,
            cache_hits: 0,
            cache_misses: 0,
        }
    }
}

fn evaluate_selected(
    evaluator: &dyn BatchEvaluator,
    changes: &[AssetChange],
    selected: &[PathBuf],
    question: &str,
    workers: usize,
    cache_dir: Option<&Path>,
    cache_options: &RubricOptions,
) -> SelectedEvaluation {
    let worker_count = workers.max(1).min(selected.len());
    let state = Arc::new(Mutex::new(EvaluationState {
        pending: selected.iter().cloned().enumerate().collect(),
        completed: Vec::with_capacity(selected.len()),
        abort_message: None,
        aborted: false,
        cache_hits: 0,
        cache_misses: 0,
    }));

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let state = Arc::clone(&state);
            scope.spawn(move || {
                loop {
                    let next = {
                        let mut state = lock_state(&state);
                        if state.abort_message.is_some() {
                            None
                        } else {
                            state.pending.pop_front()
                        }
                    };
                    let Some((index, path)) = next else {
                        break;
                    };

                    let cache_key = if cache_dir.is_some() {
                        cache_key_for_asset(&path, question, cache_options)
                    } else {
                        None
                    };
                    if let (Some(dir), Some(key)) = (cache_dir, cache_key.as_deref()) {
                        if let Some(verdict) = read_cached_verdict(dir, key) {
                            let mut state = lock_state(&state);
                            state.cache_hits += 1;
                            state.completed.push(CompletedEvaluation {
                                index,
                                path,
                                result: result_from_verdict(verdict),
                            });
                            continue;
                        }
                    }
                    if cache_dir.is_some() {
                        let mut state = lock_state(&state);
                        state.cache_misses += 1;
                    }

                    let outcome = evaluator.submit_asset(&path, question);
                    if let (Some(dir), Some(key)) = (cache_dir, cache_key.as_deref()) {
                        if let Ok(verdict) = &outcome {
                            write_cached_verdict(dir, key, verdict);
                        }
                    }
                    let mut state = lock_state(&state);
                    match outcome {
                        Ok(verdict) => state.completed.push(CompletedEvaluation {
                            index,
                            path,
                            result: result_from_verdict(verdict),
                        }),
                        Err(error) => {
                            let message = error.to_string();
                            if should_abort_after_error(&error) && state.abort_message.is_none() {
                                state.aborted = true;
                                state.abort_message = Some(message.clone());
                            }
                            state.completed.push(CompletedEvaluation {
                                index,
                                path,
                                result: AssetRubricResult::Error { message },
                            });
                        }
                    }
                }
            });
        }
    });

    let mut state = lock_state(&state);
    let abort_message = state.abort_message.clone();
    let mut completed = std::mem::take(&mut state.completed);
    completed.sort_by_key(|evaluation| evaluation.index);
    let mut reports = completed
        .into_iter()
        .map(|evaluation| {
            AssetRubricReport::selected(
                &evaluation.path,
                status_for_selected(&evaluation.path, changes),
                evaluation.result,
            )
        })
        .collect::<Vec<_>>();

    if let Some(message) = abort_message {
        reports.extend(state.pending.drain(..).map(|(_, path)| {
            AssetRubricReport::selected(
                &path,
                status_for_selected(&path, changes),
                AssetRubricResult::NotEvaluatedAfterError {
                    root_error: message.clone(),
                    retry_hint: retry_hint_after_pool_error(&message).to_owned(),
                },
            )
        }));
    }

    SelectedEvaluation {
        reports,
        aborted: state.aborted,
        cache_hits: state.cache_hits,
        cache_misses: state.cache_misses,
    }
}

struct EvaluationState {
    pending: VecDeque<(usize, PathBuf)>,
    completed: Vec<CompletedEvaluation>,
    abort_message: Option<String>,
    aborted: bool,
    cache_hits: u64,
    cache_misses: u64,
}

struct CompletedEvaluation {
    index: usize,
    path: PathBuf,
    result: AssetRubricResult,
}

fn lock_state<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(|error| error.into_inner())
}

fn result_from_verdict(verdict: RubricVerdict) -> AssetRubricResult {
    if verdict.verdict.is_pass() {
        AssetRubricResult::Pass {
            reason: verdict.reason,
            anomalies: verdict.anomalies,
        }
    } else {
        AssetRubricResult::Fail {
            reason: verdict.reason,
            anomalies: verdict.anomalies,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct CachedVerdict {
    schema_version: u32,
    key: String,
    verdict: RubricVerdict,
}

fn cache_key_for_asset(path: &Path, question: &str, options: &RubricOptions) -> Option<String> {
    let image = fs::read(path).ok()?;
    let options = serde_json::to_vec(options).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(CACHE_KEY_DOMAIN);
    update_key_part(&mut hasher, &image);
    update_key_part(&mut hasher, question.as_bytes());
    update_key_part(&mut hasher, &options);
    Some(format!("{:x}", hasher.finalize()))
}

fn update_key_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn cached_verdict_path(cache_dir: &Path, key: &str) -> PathBuf {
    cache_dir.join(format!("{key}.json"))
}

fn read_cached_verdict(cache_dir: &Path, key: &str) -> Option<RubricVerdict> {
    let bytes = fs::read(cached_verdict_path(cache_dir, key)).ok()?;
    let cached = serde_json::from_slice::<CachedVerdict>(&bytes).ok()?;
    (cached.schema_version == CACHE_SCHEMA_VERSION && cached.key == key).then_some(cached.verdict)
}

fn write_cached_verdict(cache_dir: &Path, key: &str, verdict: &RubricVerdict) {
    if fs::create_dir_all(cache_dir).is_err() {
        return;
    }
    let Ok(mut temporary) = NamedTempFile::new_in(cache_dir) else {
        return;
    };
    let cached = CachedVerdict {
        schema_version: CACHE_SCHEMA_VERSION,
        key: key.to_owned(),
        verdict: verdict.clone(),
    };
    if serde_json::to_writer(&mut temporary, &cached).is_err()
        || temporary.flush().is_err()
        || temporary.as_file().sync_all().is_err()
    {
        return;
    }
    let _ = temporary.persist(cached_verdict_path(cache_dir, key));
}

fn should_abort_after_error(error: &PoolError) -> bool {
    matches!(
        error,
        PoolError::Timeout { .. }
            | PoolError::QuotaExceeded
            | PoolError::WorkerCrashed { .. }
            | PoolError::NoLiveWorkers
            | PoolError::Closed
    )
}

fn retry_hint_after_pool_error(message: &str) -> &'static str {
    if message.contains("timed out") || message.contains("timeout") {
        "Asset was not evaluated after an ACP timeout; inspect captured logs and rerun with fewer workers or a smaller asset scope."
    } else if message.contains("quota") {
        "Asset was not evaluated after an ACP quota error; inspect captured logs, wait for quota recovery or switch credentials, then rerun the batch."
    } else {
        "Asset was not evaluated after an ACP worker error; inspect captured logs and rerun with fewer workers or a smaller asset scope."
    }
}

fn status_for_selected(path: &Path, changes: &[AssetChange]) -> &'static str {
    changes
        .iter()
        .find_map(|change| (change.path() == path).then_some(change.status()))
        .map_or("selected", |status| status)
}

fn skipped_asset_reports(
    changes: &[AssetChange],
    selection_mode: SelectionMode,
) -> Vec<AssetRubricReport> {
    let mut reports = Vec::with_capacity(changes.len());
    for change in changes {
        match (change, selection_mode) {
            (AssetChange::Unchanged(path), SelectionMode::ChangedOnly) => {
                reports.push(AssetRubricReport::skipped(
                    path,
                    "unchanged",
                    "asset content did not change during this command",
                ));
            }
            (AssetChange::Deleted(path), _) => {
                reports.push(AssetRubricReport::skipped(
                    path,
                    "deleted",
                    "asset was deleted before evaluation",
                ));
            }
            (AssetChange::Added(_) | AssetChange::Changed(_) | AssetChange::Unchanged(_), _) => {}
        }
    }
    reports
}

impl AssetRubricReport {
    fn selected(path: &Path, change: &str, result: AssetRubricResult) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            change: change.to_owned(),
            selected: true,
            result,
        }
    }

    fn skipped(path: &Path, change: &str, reason: &str) -> Self {
        Self {
            path: path.to_string_lossy().into_owned(),
            change: change.to_owned(),
            selected: false,
            result: AssetRubricResult::Skipped {
                reason: reason.to_owned(),
            },
        }
    }
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_owned())
}
