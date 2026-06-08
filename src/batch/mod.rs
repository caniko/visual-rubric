mod logs;
mod recommendations;
mod snapshot;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{PoolConfig, PoolError, RubricOptions, RubricPool, RubricVerdict};

use logs::copy_logs_from_config;
use recommendations::{aggregate_status, classify_recommendations};

pub use snapshot::{AssetChange, AssetSnapshot, SelectionMode, diff_snapshots, select_changed};

const REPORT_SCHEMA_VERSION: u32 = 1;

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
    /// Effective default options supplied to the pool.
    pub options: RubricOptions,
    /// Copied ACP log paths.
    pub logs: Vec<String>,
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
    /// Optional issue classifier.
    pub classifier: Option<&'a dyn IssueClassifier>,
}

/// Generic batch runner around [`RubricPool`].
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
            evaluate_selected(evaluator, changes, &selected, &self.config.question)
        } else {
            match RubricPool::new(self.config.pool.clone()) {
                Ok(pool) => {
                    let evaluation =
                        evaluate_selected(&pool, changes, &selected, &self.config.question);
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
        let logs = copy_logs_from_config(self.config.pool.log_capture.as_ref()).unwrap_or_default();
        let recommendations = classify_recommendations(self.config.classifier, &assets);
        let aggregate_status = aggregate_status(&assets);
        BatchRubricReport {
            schema_version: REPORT_SCHEMA_VERSION,
            aggregate_status,
            started_at,
            finished_at: unix_timestamp(),
            workers: self.config.pool.workers,
            options: self.config.pool.default_options.clone(),
            logs,
            assets,
            recommendations,
        }
    }
}

trait BatchEvaluator {
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
}

impl From<Vec<AssetRubricReport>> for SelectedEvaluation {
    fn from(reports: Vec<AssetRubricReport>) -> Self {
        Self {
            reports,
            aborted: false,
        }
    }
}

fn evaluate_selected(
    evaluator: &dyn BatchEvaluator,
    changes: &[AssetChange],
    selected: &[PathBuf],
    question: &str,
) -> SelectedEvaluation {
    let mut reports = Vec::with_capacity(selected.len());
    let mut abort_message = None;
    let mut aborted = false;
    for (index, path) in selected.iter().enumerate() {
        let Some(message) = abort_message.as_ref() else {
            let result = match evaluator.submit_asset(path, question) {
                Ok(verdict) => result_from_verdict(verdict),
                Err(error) => {
                    let message = error.to_string();
                    if should_abort_after_error(&error) {
                        aborted = true;
                        abort_message = Some(message.clone());
                    }
                    AssetRubricResult::Error { message }
                }
            };
            reports.push(AssetRubricReport::selected(
                path,
                status_for_selected(path, changes),
                result,
            ));
            continue;
        };

        reports.extend(selected[index..].iter().map(|remaining| {
            AssetRubricReport::selected(
                remaining,
                status_for_selected(remaining, changes),
                AssetRubricResult::NotEvaluatedAfterError {
                    root_error: message.clone(),
                    retry_hint: retry_hint_after_pool_error(message).to_owned(),
                },
            )
        }));
        break;
    }
    SelectedEvaluation { reports, aborted }
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
        .unwrap_or("selected")
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
