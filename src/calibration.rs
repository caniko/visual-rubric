//! Checked-in calibration corpus contracts for visual-detector QA.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::manifest::Viewport;
use crate::qa::{CALIBRATION_SENTINEL_SCHEMA_VERSION, CalibrationSentinelV1};

/// Current checked-in calibration-corpus schema version.
pub const CALIBRATION_CORPUS_SCHEMA_VERSION: u32 = 1;

/// Defect classes that the shared visual gate must keep detecting.
pub const REQUIRED_CALIBRATION_CATEGORIES: &[&str] = &[
    "clipping",
    "overlap",
    "blank-content",
    "wrong-hierarchy",
    "low-contrast",
    "tiny-text",
    "missing-focus",
    "overflow",
    "responsive-layout",
    "stale-state",
    "incorrect-theme",
    "animation-instability",
    "missing-control",
];

/// A complete, versioned set of known-good and known-bad visual fixtures.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CalibrationCorpusV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Stable target identifier used by calibration reports.
    pub target: String,
    /// Fixture revision recorded in the capture job.
    pub revision: String,
    /// Capture cells containing the fixtures.
    pub cells: Vec<CalibrationCaptureV1>,
    /// Expected evaluator outcomes for the cells.
    pub sentinels: Vec<CalibrationSentinelV1>,
}

/// One capture cell in the calibration corpus.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CalibrationCaptureV1 {
    /// Stable capture-cell identifier.
    pub id: String,
    /// Local route or fixture path used to render the cell.
    pub path: String,
    /// Producer-owned visual state name.
    pub state: String,
    /// Capture viewport.
    pub viewport: Viewport,
    /// Theme selected for the cell.
    pub theme: String,
    /// Locale selected for the cell.
    pub locale: String,
    /// Selectors for controls that must exist in this visual state.
    #[serde(default)]
    pub required_controls: Vec<String>,
}

impl CalibrationCorpusV1 {
    /// Validates corpus structure and the complete required defect category set.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut issues = Vec::new();
        if self.schema_version != CALIBRATION_CORPUS_SCHEMA_VERSION {
            issues.push(format!(
                "unsupported calibration corpus schema_version {}; expected {}",
                self.schema_version, CALIBRATION_CORPUS_SCHEMA_VERSION
            ));
        }
        required("calibration target", &self.target, &mut issues);
        required("calibration revision", &self.revision, &mut issues);
        if self.cells.is_empty() {
            issues.push("calibration corpus must contain cells".to_owned());
        }

        let mut cell_ids = BTreeSet::new();
        for cell in &self.cells {
            required("calibration cell id", &cell.id, &mut issues);
            required("calibration cell path", &cell.path, &mut issues);
            required("calibration cell state", &cell.state, &mut issues);
            required("calibration cell theme", &cell.theme, &mut issues);
            required("calibration cell locale", &cell.locale, &mut issues);
            for selector in &cell.required_controls {
                required(
                    "calibration required control selector",
                    selector,
                    &mut issues,
                );
            }
            if !cell_ids.insert(cell.id.as_str()) {
                issues.push(format!("duplicate calibration cell id {:?}", cell.id));
            }
            if cell.viewport.width == 0 || cell.viewport.height == 0 {
                issues.push(format!("calibration cell {:?} has zero viewport", cell.id));
            }
            if !cell.viewport.dpr.is_finite() || cell.viewport.dpr <= 0.0 {
                issues.push(format!("calibration cell {:?} has invalid dpr", cell.id));
            }
        }

        if self.sentinels.is_empty() {
            issues.push("calibration corpus must contain sentinels".to_owned());
        }
        let mut sentinel_ids = BTreeSet::new();
        let mut categories = BTreeSet::new();
        let mut clear_count = 0;
        for sentinel in &self.sentinels {
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
            if !sentinel_ids.insert(sentinel.id.as_str()) {
                issues.push(format!(
                    "duplicate calibration sentinel id {:?}",
                    sentinel.id
                ));
            }
            if !cell_ids.contains(sentinel.capture_id.as_str()) {
                issues.push(format!(
                    "calibration sentinel {:?} references unknown capture {:?}",
                    sentinel.id, sentinel.capture_id
                ));
            }
            match &sentinel.expectation {
                crate::qa::CalibrationExpectation::Clear => clear_count += 1,
                crate::qa::CalibrationExpectation::Finding { observation_kind } => {
                    required(
                        "calibration observation_kind",
                        observation_kind,
                        &mut issues,
                    );
                    categories.insert(observation_kind.as_str());
                }
            }
        }
        if clear_count == 0 {
            issues.push("calibration corpus must contain a known-good sentinel".to_owned());
        }
        let required_categories = REQUIRED_CALIBRATION_CATEGORIES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if categories != required_categories {
            let missing = required_categories
                .difference(&categories)
                .copied()
                .collect::<Vec<_>>();
            let unexpected = categories
                .difference(&required_categories)
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(format!(
                    "calibration corpus is missing categories: {missing:?}"
                ));
            }
            if !unexpected.is_empty() {
                issues.push(format!(
                    "calibration corpus has unknown categories: {unexpected:?}"
                ));
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

fn required(name: &str, value: &str, issues: &mut Vec<String>) {
    if value.trim().is_empty() {
        issues.push(format!("{name} must not be empty"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_categories_are_unique() {
        let categories = REQUIRED_CALIBRATION_CATEGORIES
            .iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(categories.len(), REQUIRED_CALIBRATION_CATEGORIES.len());
    }
}
