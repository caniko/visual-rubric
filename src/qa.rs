//! Portable, versioned contracts for autonomous visual QA evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{ArtifactDigest, CAPTURE_MANIFEST_SCHEMA_VERSION, CaptureManifest};

/// Current observation schema version.
pub const OBSERVATION_SCHEMA_VERSION: u32 = 1;
/// Current finding schema version.
pub const FINDING_SCHEMA_VERSION: u32 = 1;
/// Current visual-run schema version.
pub const VISUAL_RUN_REPORT_SCHEMA_VERSION: u32 = 1;
/// Current calibration-sentinel schema version.
pub const CALIBRATION_SENTINEL_SCHEMA_VERSION: u32 = 1;

/// A normalized observation produced by deterministic or semantic inspection.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ObservationV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Stable observation identifier within the run.
    pub id: String,
    /// Capture cell that was inspected.
    pub capture_id: String,
    /// Producer-defined observation class.
    pub kind: String,
    /// Concise factual description.
    pub summary: String,
    /// Evidence artifacts supporting the observation.
    #[serde(default)]
    pub evidence: Vec<ArtifactDigest>,
}

/// A deduplicatable issue derived from one or more observations.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct FindingV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Stable content-derived fingerprint used by issue trackers.
    pub fingerprint: String,
    /// Visual target identifier.
    pub target: String,
    /// Exact source revision that was inspected.
    pub revision: String,
    /// Human-readable issue title.
    pub title: String,
    /// Observation identifiers supporting this finding.
    pub observation_ids: Vec<String>,
    /// Reproduction steps or command.
    pub reproduction: Vec<String>,
}

/// Result of a complete producer/evaluator visual QA run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VisualRunReportV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Unique run identifier.
    pub run_id: String,
    /// Visual target identifier.
    pub target: String,
    /// Exact source revision under test.
    pub revision: String,
    /// Whether the producer checkout contained uncommitted changes.
    pub dirty: bool,
    /// Content-addressed capture manifest used by this run.
    pub capture_manifest: ArtifactDigest,
    /// Capture cells declared by the producer as part of this run.
    #[serde(default)]
    pub planned_capture_ids: Vec<String>,
    /// Capture cells for which deterministic and semantic evaluation completed.
    #[serde(default)]
    pub evaluated_capture_ids: Vec<String>,
    /// Semantic result of the run.
    pub status: VisualRunStatus,
    /// Normalized observations.
    #[serde(default)]
    pub observations: Vec<ObservationV1>,
    /// Deduplicatable findings.
    #[serde(default)]
    pub findings: Vec<FindingV1>,
}

/// Semantic visual-run result.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisualRunStatus {
    /// Every planned cell was evaluated and passed.
    Pass,
    /// At least one complete evaluation produced a finding.
    Fail,
    /// Required inputs or infrastructure were unavailable.
    Blocked,
}

/// A known-good or known-bad capture used to detect evaluator drift.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CalibrationSentinelV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Stable sentinel identifier.
    pub id: String,
    /// Capture cell containing the calibrated fixture.
    pub capture_id: String,
    /// Result the evaluator must reproduce.
    pub expectation: CalibrationExpectation,
}

/// Expected evaluator behavior for a calibration fixture.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalibrationExpectation {
    /// The known-good fixture must not support a finding.
    Clear,
    /// The known-bad fixture must produce a finding backed by this observation kind.
    Finding {
        /// Expected observation class, such as `layout_overlap`.
        observation_kind: String,
    },
}

impl VisualRunReportV1 {
    /// Serializes this run report into deterministic compact JSON.
    ///
    /// Set-like record collections are normalized while ordered reproduction
    /// steps retain their producer-defined order.
    pub fn to_canonical_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut normalized = self.clone();
        normalized.planned_capture_ids.sort();
        normalized.evaluated_capture_ids.sort();
        normalized
            .observations
            .sort_by(|left, right| left.id.cmp(&right.id));
        for observation in &mut normalized.observations {
            observation
                .evidence
                .sort_by(|left, right| left.path.cmp(&right.path));
        }
        normalized
            .findings
            .sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
        for finding in &mut normalized.findings {
            finding.observation_ids.sort();
        }
        serde_json::to_vec(&normalized)
    }
}

/// Computes the revision-independent fingerprint for one finding.
///
/// The identity is derived from the target plus the sorted set of capture
/// cells and observation kinds. Titles, summaries, and source revisions are
/// deliberately excluded so the same defect deduplicates across runs.
pub fn finding_fingerprint(target: &str, observations: &[ObservationV1]) -> String {
    let mut identities = observations
        .iter()
        .map(|observation| (observation.capture_id.trim(), observation.kind.trim()))
        .collect::<Vec<_>>();
    identities.sort_unstable();
    identities.dedup();

    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"visual-rubric:finding:v1");
    hash_field(&mut hasher, target.trim().as_bytes());
    for (capture_id, kind) in identities {
        hash_field(&mut hasher, capture_id.as_bytes());
        hash_field(&mut hasher, kind.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

/// Validates cross-record invariants that do not require filesystem access.
pub fn validate_visual_run(report: &VisualRunReportV1) -> Result<(), Vec<String>> {
    let mut issues = Vec::new();
    if report.schema_version != VISUAL_RUN_REPORT_SCHEMA_VERSION {
        issues.push("unsupported visual run schema_version".to_owned());
    }
    required("run_id", &report.run_id, &mut issues);
    required("target", &report.target, &mut issues);
    required("revision", &report.revision, &mut issues);
    validate_digest(&report.capture_manifest, &mut issues);
    if matches!(report.status, VisualRunStatus::Pass) && report.dirty {
        issues.push("passing visual run must not be dirty".to_owned());
    }
    validate_cell_coverage(
        report,
        &report.planned_capture_ids,
        &report.evaluated_capture_ids,
        &mut issues,
    );

    let mut observation_ids = BTreeSet::new();
    let mut observations_by_id = BTreeMap::new();
    for observation in &report.observations {
        if observation.schema_version != OBSERVATION_SCHEMA_VERSION {
            issues.push(format!(
                "observation {:?} has unsupported schema_version",
                observation.id
            ));
        }
        required("observation.id", &observation.id, &mut issues);
        required(
            "observation.capture_id",
            &observation.capture_id,
            &mut issues,
        );
        required("observation.kind", &observation.kind, &mut issues);
        required("observation.summary", &observation.summary, &mut issues);
        if !observation_ids.insert(observation.id.as_str()) {
            issues.push(format!("duplicate observation id {:?}", observation.id));
        } else {
            observations_by_id.insert(observation.id.as_str(), observation);
        }
        for artifact in &observation.evidence {
            validate_digest(artifact, &mut issues);
        }
    }

    let mut fingerprints = BTreeSet::new();
    for finding in &report.findings {
        if finding.schema_version != FINDING_SCHEMA_VERSION {
            issues.push(format!(
                "finding {:?} has unsupported schema_version",
                finding.fingerprint
            ));
        }
        required("finding.fingerprint", &finding.fingerprint, &mut issues);
        if finding.fingerprint.len() != 64
            || !finding
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            issues.push(format!(
                "finding {:?} fingerprint must be lowercase SHA-256 hex",
                finding.fingerprint
            ));
        }
        required("finding.target", &finding.target, &mut issues);
        required("finding.revision", &finding.revision, &mut issues);
        required("finding.title", &finding.title, &mut issues);
        if finding.target != report.target || finding.revision != report.revision {
            issues.push(format!(
                "finding {:?} target/revision does not match its run",
                finding.fingerprint
            ));
        }
        if !fingerprints.insert(finding.fingerprint.as_str()) {
            issues.push(format!(
                "duplicate finding fingerprint {:?}",
                finding.fingerprint
            ));
        }
        let unique_observation_ids = finding
            .observation_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let references_missing = finding.observation_ids.is_empty()
            || unique_observation_ids.len() != finding.observation_ids.len()
            || finding
                .observation_ids
                .iter()
                .any(|id| !observation_ids.contains(id.as_str()));
        if references_missing {
            issues.push(format!(
                "finding {:?} references missing or duplicate observations",
                finding.fingerprint
            ));
        } else {
            let referenced = finding
                .observation_ids
                .iter()
                .filter_map(|id| observations_by_id.get(id.as_str()).copied())
                .cloned()
                .collect::<Vec<_>>();
            let expected = finding_fingerprint(&finding.target, &referenced);
            if finding.fingerprint != expected {
                issues.push(format!(
                    "finding {:?} fingerprint does not match its observation identity; expected {expected}",
                    finding.fingerprint
                ));
            }
        }
        if finding.reproduction.is_empty() {
            issues.push(format!(
                "finding {:?} requires reproduction steps",
                finding.fingerprint
            ));
        }
    }

    if matches!(report.status, VisualRunStatus::Pass) && !report.findings.is_empty() {
        issues.push("passing visual run cannot contain findings".to_owned());
    }
    if matches!(report.status, VisualRunStatus::Fail) && report.findings.is_empty() {
        issues.push("failed visual run requires at least one finding".to_owned());
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// Validates a report and proves that it covers exactly the cells in the
/// referenced capture manifest. Callers should use this at the controller
/// boundary after loading the manifest with [`CaptureManifest::load`].
pub fn validate_visual_run_against_manifest(
    report: &VisualRunReportV1,
    manifest: &CaptureManifest,
) -> Result<(), Vec<String>> {
    let mut issues = validate_visual_run(report).err().unwrap_or_default();
    if manifest.schema_version != CAPTURE_MANIFEST_SCHEMA_VERSION {
        issues.push("capture manifest has unsupported schema_version".to_owned());
    }
    if report.target != manifest.target {
        issues.push("visual run target does not match capture manifest".to_owned());
    }
    if report.revision != manifest.revision {
        issues.push("visual run revision does not match capture manifest".to_owned());
    }
    if report.dirty != manifest.dirty {
        issues.push("visual run dirty state does not match capture manifest".to_owned());
    }
    let manifest_ids: BTreeSet<_> = manifest
        .captures
        .iter()
        .map(|capture| capture.id.as_str())
        .collect();
    let planned_ids: BTreeSet<_> = report
        .planned_capture_ids
        .iter()
        .map(String::as_str)
        .collect();
    let evaluated_ids: BTreeSet<_> = report
        .evaluated_capture_ids
        .iter()
        .map(String::as_str)
        .collect();
    if planned_ids != manifest_ids {
        issues.push("visual run planned cells do not match capture manifest".to_owned());
    }
    if evaluated_ids != manifest_ids {
        issues.push("visual run evaluated cells do not match capture manifest".to_owned());
    }
    let observed_ids = report
        .observations
        .iter()
        .map(|observation| observation.capture_id.as_str())
        .collect::<BTreeSet<_>>();
    for unknown in observed_ids.difference(&manifest_ids) {
        issues.push(format!(
            "visual run observation references unknown capture {unknown:?}"
        ));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// Validates known-good and known-bad calibration fixtures against a run.
pub fn validate_calibration_sentinels(
    report: &VisualRunReportV1,
    sentinels: &[CalibrationSentinelV1],
) -> Result<(), Vec<String>> {
    let mut issues = Vec::new();
    if sentinels.is_empty() {
        issues.push("calibration requires at least one sentinel".to_owned());
    }
    let planned = report
        .planned_capture_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let evaluated = report
        .evaluated_capture_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let observations = report
        .observations
        .iter()
        .map(|observation| (observation.id.as_str(), observation))
        .collect::<BTreeMap<_, _>>();
    let finding_observations = report
        .findings
        .iter()
        .flat_map(|finding| finding.observation_ids.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();

    let mut ids = BTreeSet::new();
    for sentinel in sentinels {
        if sentinel.schema_version != CALIBRATION_SENTINEL_SCHEMA_VERSION {
            issues.push(format!(
                "calibration sentinel {:?} has unsupported schema_version",
                sentinel.id
            ));
        }
        required("calibration sentinel id", &sentinel.id, &mut issues);
        required(
            "calibration sentinel capture_id",
            &sentinel.capture_id,
            &mut issues,
        );
        if !ids.insert(sentinel.id.as_str()) {
            issues.push(format!(
                "duplicate calibration sentinel id {:?}",
                sentinel.id
            ));
        }
        if !planned.contains(sentinel.capture_id.as_str())
            || !evaluated.contains(sentinel.capture_id.as_str())
        {
            issues.push(format!(
                "calibration sentinel {:?} capture was not planned and evaluated",
                sentinel.id
            ));
            continue;
        }

        let supporting = observations.values().filter(|observation| {
            observation.capture_id == sentinel.capture_id
                && finding_observations.contains(observation.id.as_str())
        });
        match &sentinel.expectation {
            CalibrationExpectation::Clear => {
                if supporting.count() != 0 {
                    issues.push(format!(
                        "known-good calibration sentinel {:?} produced a finding",
                        sentinel.id
                    ));
                }
            }
            CalibrationExpectation::Finding { observation_kind } => {
                required(
                    "calibration observation_kind",
                    observation_kind,
                    &mut issues,
                );
                if !supporting
                    .into_iter()
                    .any(|observation| observation.kind == *observation_kind)
                {
                    issues.push(format!(
                        "known-bad calibration sentinel {:?} did not produce finding kind {:?}",
                        sentinel.id, observation_kind
                    ));
                }
            }
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn validate_cell_coverage(
    report: &VisualRunReportV1,
    planned: &[String],
    evaluated: &[String],
    issues: &mut Vec<String>,
) {
    let planned_set: BTreeSet<_> = planned.iter().map(String::as_str).collect();
    let evaluated_set: BTreeSet<_> = evaluated.iter().map(String::as_str).collect();
    if planned.is_empty() {
        issues.push("visual run must declare at least one planned capture cell".to_owned());
    }
    if planned_set.len() != planned.len() {
        issues.push("visual run planned capture ids must be unique".to_owned());
    }
    if evaluated_set.len() != evaluated.len() {
        issues.push("visual run evaluated capture ids must be unique".to_owned());
    }
    if !evaluated_set.is_subset(&planned_set) {
        issues.push("visual run evaluated cells must be planned cells".to_owned());
    }
    if matches!(report.status, VisualRunStatus::Pass) && planned_set != evaluated_set {
        issues.push("passing visual run must evaluate every planned capture cell".to_owned());
    }
}

fn required(name: &str, value: &str, issues: &mut Vec<String>) {
    if value.trim().is_empty() {
        issues.push(format!("{name} must not be empty"));
    }
}

fn validate_digest(artifact: &ArtifactDigest, issues: &mut Vec<String>) {
    validate_relative_path("artifact", &artifact.path, issues);
    if artifact.bytes == 0 {
        issues.push(format!("artifact {} is empty", artifact.path.display()));
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        issues.push(format!(
            "artifact {} has invalid sha256",
            artifact.path.display()
        ));
    }
}

fn validate_relative_path(name: &str, path: &Path, issues: &mut Vec<String>) {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        issues.push(format!(
            "{name} path {} must be relative and contained",
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_pass_with_findings_or_dirty_source() {
        let report = VisualRunReportV1 {
            schema_version: VISUAL_RUN_REPORT_SCHEMA_VERSION,
            run_id: "run-1".to_owned(),
            target: "regicide/game".to_owned(),
            revision: "abc123".to_owned(),
            dirty: true,
            capture_manifest: ArtifactDigest {
                path: "capture_manifest.json".into(),
                sha256: "0".repeat(64),
                bytes: 1,
            },
            planned_capture_ids: vec!["menu/desktop".to_owned()],
            evaluated_capture_ids: vec!["menu/desktop".to_owned()],
            status: VisualRunStatus::Pass,
            observations: vec![],
            findings: vec![FindingV1 {
                schema_version: FINDING_SCHEMA_VERSION,
                fingerprint: "layout/main-menu".to_owned(),
                target: "regicide/game".to_owned(),
                revision: "abc123".to_owned(),
                title: "Menu overlaps".to_owned(),
                observation_ids: vec!["missing".to_owned()],
                reproduction: vec!["run visual suite".to_owned()],
            }],
        };
        let issues = validate_visual_run(&report).expect_err("invalid pass");
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("must not be dirty"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("cannot contain findings"))
        );
    }

    #[test]
    fn rejects_empty_passing_coverage() {
        let report = VisualRunReportV1 {
            schema_version: VISUAL_RUN_REPORT_SCHEMA_VERSION,
            run_id: "run-1".to_owned(),
            target: "regicide/game".to_owned(),
            revision: "abc123".to_owned(),
            dirty: false,
            capture_manifest: ArtifactDigest {
                path: "capture_manifest.json".into(),
                sha256: "0".repeat(64),
                bytes: 1,
            },
            planned_capture_ids: Vec::new(),
            evaluated_capture_ids: Vec::new(),
            status: VisualRunStatus::Pass,
            observations: Vec::new(),
            findings: Vec::new(),
        };
        let issues = validate_visual_run(&report).expect_err("empty pass must fail");
        assert!(issues.iter().any(|issue| issue.contains("planned capture")));
    }

    #[test]
    fn finding_fingerprint_is_stable_and_verified() {
        let observations = vec![
            ObservationV1 {
                schema_version: OBSERVATION_SCHEMA_VERSION,
                id: "obs-b".to_owned(),
                capture_id: "menu/mobile".to_owned(),
                kind: "layout-overlap".to_owned(),
                summary: "buttons overlap".to_owned(),
                evidence: Vec::new(),
            },
            ObservationV1 {
                schema_version: OBSERVATION_SCHEMA_VERSION,
                id: "obs-a".to_owned(),
                capture_id: "menu/desktop".to_owned(),
                kind: "layout-overlap".to_owned(),
                summary: "buttons overlap".to_owned(),
                evidence: Vec::new(),
            },
        ];
        let mut reversed = observations.clone();
        reversed.reverse();
        let fingerprint = finding_fingerprint("regicide/game", &observations);
        assert_eq!(fingerprint, finding_fingerprint("regicide/game", &reversed));

        let report = VisualRunReportV1 {
            schema_version: VISUAL_RUN_REPORT_SCHEMA_VERSION,
            run_id: "run-1".to_owned(),
            target: "regicide/game".to_owned(),
            revision: "abc123".to_owned(),
            dirty: false,
            capture_manifest: ArtifactDigest {
                path: "capture_manifest.json".into(),
                sha256: "0".repeat(64),
                bytes: 1,
            },
            planned_capture_ids: vec!["menu/desktop".to_owned(), "menu/mobile".to_owned()],
            evaluated_capture_ids: vec!["menu/desktop".to_owned(), "menu/mobile".to_owned()],
            status: VisualRunStatus::Fail,
            observations,
            findings: vec![FindingV1 {
                schema_version: FINDING_SCHEMA_VERSION,
                fingerprint,
                target: "regicide/game".to_owned(),
                revision: "abc123".to_owned(),
                title: "Menu buttons overlap".to_owned(),
                observation_ids: vec!["obs-a".to_owned(), "obs-b".to_owned()],
                reproduction: vec!["run the visual matrix".to_owned()],
            }],
        };
        assert!(validate_visual_run(&report).is_ok());

        let mut reordered = report.clone();
        reordered.planned_capture_ids.reverse();
        reordered.evaluated_capture_ids.reverse();
        reordered.observations.reverse();
        reordered.findings[0].observation_ids.reverse();
        assert_eq!(
            report.to_canonical_json().expect("canonical report"),
            reordered.to_canonical_json().expect("canonical report")
        );

        let sentinel = CalibrationSentinelV1 {
            schema_version: CALIBRATION_SENTINEL_SCHEMA_VERSION,
            id: "known-overlap".to_owned(),
            capture_id: "menu/desktop".to_owned(),
            expectation: CalibrationExpectation::Finding {
                observation_kind: "layout-overlap".to_owned(),
            },
        };
        assert!(validate_calibration_sentinels(&report, &[sentinel]).is_ok());
    }

    #[test]
    fn calibration_fails_when_a_negative_sentinel_is_missed() {
        let report = VisualRunReportV1 {
            schema_version: VISUAL_RUN_REPORT_SCHEMA_VERSION,
            run_id: "run-1".to_owned(),
            target: "regicide/game".to_owned(),
            revision: "abc123".to_owned(),
            dirty: false,
            capture_manifest: ArtifactDigest {
                path: "capture_manifest.json".into(),
                sha256: "0".repeat(64),
                bytes: 1,
            },
            planned_capture_ids: vec!["menu/desktop".to_owned()],
            evaluated_capture_ids: vec!["menu/desktop".to_owned()],
            status: VisualRunStatus::Pass,
            observations: Vec::new(),
            findings: Vec::new(),
        };
        let sentinel = CalibrationSentinelV1 {
            schema_version: CALIBRATION_SENTINEL_SCHEMA_VERSION,
            id: "known-overlap".to_owned(),
            capture_id: "menu/desktop".to_owned(),
            expectation: CalibrationExpectation::Finding {
                observation_kind: "layout-overlap".to_owned(),
            },
        };
        let issues = validate_calibration_sentinels(&report, &[sentinel])
            .expect_err("missed negative sentinel must fail calibration");
        assert!(issues.iter().any(|issue| issue.contains("did not produce")));
    }
}
