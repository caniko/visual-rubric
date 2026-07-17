//! Shared semantic coverage contracts for autonomous visual QA.
//!
//! A producer owns the vocabulary of visible surfaces and interaction
//! transitions.  The controller only consumes this versioned contract and
//! proves that every required identifier was executed and evaluated.  Keeping
//! this data model in the rubric crate prevents project-specific registries
//! from becoming disconnected, false-green bookkeeping.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Current semantic coverage-contract schema version.
pub const COVERAGE_CONTRACT_SCHEMA_VERSION: u32 = 1;
/// Current semantic coverage-report schema version.
pub const COVERAGE_REPORT_SCHEMA_VERSION: u32 = 1;

/// A complete producer-owned inventory of semantic UI and interaction work.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoverageContractV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Stable visual target identifier.
    pub target: String,
    /// Source revision used to create the contract.
    pub revision: String,
    /// Visible semantic surfaces that must be observed.
    pub surfaces: Vec<CoverageSurfaceV1>,
    /// User actions and state transitions that must be exercised.
    pub transitions: Vec<CoverageTransitionV1>,
    /// Explicitly excluded work, with an accountable owner and expiry.
    #[serde(default)]
    pub exclusions: Vec<CoverageExclusionV1>,
}

/// One visible semantic surface in the coverage contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoverageSurfaceV1 {
    /// Stable producer-owned surface identifier.
    pub id: String,
    /// Platform scope, for example `portable`, `steam_desktop`, or `android`.
    pub platform: String,
    /// Whether this surface is required for a complete pass.
    pub required: bool,
    /// Human-readable description of the visible state.
    pub description: String,
}

/// One semantic interaction transition in the coverage contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoverageTransitionV1 {
    /// Stable producer-owned transition identifier.
    pub id: String,
    /// Surface before the action, or an empty string for an external entry.
    pub from: String,
    /// Stable action identifier.
    pub action: String,
    /// Surface after the action, or an empty string for an external exit.
    pub to: String,
    /// Platform scope on which the transition is supported.
    pub platform: String,
    /// Whether this transition is required for a complete pass.
    pub required: bool,
}

/// A time-bounded exclusion from the semantic coverage contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoverageExclusionV1 {
    /// Stable exclusion identifier.
    pub id: String,
    /// Why the item cannot be covered yet.
    pub reason: String,
    /// Person or team accountable for removing the exclusion.
    pub owner: String,
    /// ISO-8601 calendar date after which the exclusion is invalid.
    pub review_after: String,
}

/// Deterministic execution result for a coverage contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CoverageReportV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// SHA-256 of the canonical contract JSON.
    pub contract_sha256: String,
    /// Required IDs planned by the producer.
    pub planned_ids: Vec<String>,
    /// IDs for which a journey actually executed.
    pub executed_ids: Vec<String>,
    /// IDs whose deterministic and semantic checks passed.
    pub passed_ids: Vec<String>,
    /// IDs that executed and produced a failure.
    #[serde(default)]
    pub failed_ids: Vec<String>,
    /// IDs that could not execute because required infrastructure was absent.
    #[serde(default)]
    pub blocked_ids: Vec<String>,
}

impl CoverageContractV1 {
    /// Returns all required surface and transition identifiers in stable order.
    pub fn required_ids(&self) -> Vec<String> {
        self.surfaces
            .iter()
            .filter(|surface| surface.required)
            .map(|surface| surface.id.clone())
            .chain(
                self.transitions
                    .iter()
                    .filter(|transition| transition.required)
                    .map(|transition| transition.id.clone()),
            )
            .collect()
    }

    /// Validates the contract without requiring filesystem access.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut issues = Vec::new();
        if self.schema_version != COVERAGE_CONTRACT_SCHEMA_VERSION {
            issues.push(format!(
                "unsupported coverage contract schema_version {}; expected {}",
                self.schema_version, COVERAGE_CONTRACT_SCHEMA_VERSION
            ));
        }
        required("target", &self.target, &mut issues);
        required("revision", &self.revision, &mut issues);
        if self.surfaces.is_empty() {
            issues.push("coverage contract must contain at least one surface".to_owned());
        }
        if self.transitions.is_empty() {
            issues.push("coverage contract must contain at least one transition".to_owned());
        }

        let surface_ids = unique_ids(
            self.surfaces.iter().map(|surface| surface.id.as_str()),
            "surface",
            &mut issues,
        );
        unique_ids(
            self.transitions
                .iter()
                .map(|transition| transition.id.as_str()),
            "transition",
            &mut issues,
        );
        unique_ids(
            self.surfaces
                .iter()
                .map(|surface| surface.id.as_str())
                .chain(
                    self.transitions
                        .iter()
                        .map(|transition| transition.id.as_str()),
                ),
            "contract",
            &mut issues,
        );
        for surface in &self.surfaces {
            required("surface.id", &surface.id, &mut issues);
            required("surface.platform", &surface.platform, &mut issues);
            required("surface.description", &surface.description, &mut issues);
        }
        for transition in &self.transitions {
            required("transition.id", &transition.id, &mut issues);
            required("transition.action", &transition.action, &mut issues);
            required("transition.platform", &transition.platform, &mut issues);
            if !transition.from.is_empty() && !surface_ids.contains(transition.from.as_str()) {
                issues.push(format!(
                    "transition {:?} references unknown from surface {:?}",
                    transition.id, transition.from
                ));
            }
            if !transition.to.is_empty() && !surface_ids.contains(transition.to.as_str()) {
                issues.push(format!(
                    "transition {:?} references unknown to surface {:?}",
                    transition.id, transition.to
                ));
            }
        }
        for exclusion in &self.exclusions {
            required("exclusion.id", &exclusion.id, &mut issues);
            required("exclusion.reason", &exclusion.reason, &mut issues);
            required("exclusion.owner", &exclusion.owner, &mut issues);
            if !is_iso_date(&exclusion.review_after) {
                issues.push(format!(
                    "exclusion {:?} review_after must be YYYY-MM-DD",
                    exclusion.id
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

impl CoverageReportV1 {
    /// Validates execution accounting against a coverage contract.
    pub fn validate_against(&self, contract: &CoverageContractV1) -> Result<(), Vec<String>> {
        let mut issues = Vec::new();
        if self.schema_version != COVERAGE_REPORT_SCHEMA_VERSION {
            issues.push(format!(
                "unsupported coverage report schema_version {}; expected {}",
                self.schema_version, COVERAGE_REPORT_SCHEMA_VERSION
            ));
        }
        if self.contract_sha256.len() != 64
            || !self
                .contract_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            issues.push("coverage report contract_sha256 must be lowercase SHA-256".to_owned());
        }
        let required = contract.required_ids().into_iter().collect::<BTreeSet<_>>();
        let planned = unique_set("planned", &self.planned_ids, &mut issues);
        let executed = unique_set("executed", &self.executed_ids, &mut issues);
        let passed = unique_set("passed", &self.passed_ids, &mut issues);
        let failed = unique_set("failed", &self.failed_ids, &mut issues);
        let blocked = unique_set("blocked", &self.blocked_ids, &mut issues);
        if planned != required {
            issues.push("planned coverage IDs do not match required contract IDs".to_owned());
        }
        if !executed.is_subset(&planned) {
            issues.push("executed coverage IDs must be planned IDs".to_owned());
        }
        if !passed.is_subset(&executed) {
            issues.push("passed coverage IDs must be executed IDs".to_owned());
        }
        if !failed.is_subset(&executed) {
            issues.push("failed coverage IDs must be executed IDs".to_owned());
        }
        if !blocked.is_subset(&planned) {
            issues.push("blocked coverage IDs must be planned IDs".to_owned());
        }
        if !passed.is_disjoint(&failed)
            || !passed.is_disjoint(&blocked)
            || !failed.is_disjoint(&blocked)
        {
            issues.push("passed, failed, and blocked coverage IDs must be disjoint".to_owned());
        }
        if passed.len() + failed.len() != executed.len() {
            issues.push("every executed coverage ID must be exactly passed or failed".to_owned());
        }
        let mut accounted = executed.clone();
        accounted.extend(blocked.iter().cloned());
        if accounted != planned {
            issues.push("every planned coverage ID must be exactly executed or blocked".to_owned());
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    /// Returns true only when every required item executed and passed.
    pub fn is_complete_pass(&self, contract: &CoverageContractV1) -> bool {
        self.validate_against(contract).is_ok()
            && self.executed_ids.len() == self.passed_ids.len()
            && self.blocked_ids.is_empty()
            && self.failed_ids.is_empty()
    }
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    kind: &str,
    issues: &mut Vec<String>,
) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for id in ids {
        if !result.insert(id.to_owned()) {
            issues.push(format!("duplicate {kind} id {id:?}"));
        }
    }
    result
}

fn unique_set(kind: &str, ids: &[String], issues: &mut Vec<String>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() {
            issues.push(format!("{kind} coverage IDs must not be empty"));
        }
        if !result.insert(id.clone()) {
            issues.push(format!("duplicate {kind} coverage ID {id:?}"));
        }
    }
    result
}

fn required(name: &str, value: &str, issues: &mut Vec<String>) {
    if value.trim().is_empty() {
        issues.push(format!("{name} must not be empty"));
    }
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> CoverageContractV1 {
        CoverageContractV1 {
            schema_version: COVERAGE_CONTRACT_SCHEMA_VERSION,
            target: "demo/game".to_owned(),
            revision: "abc123".to_owned(),
            surfaces: vec![
                CoverageSurfaceV1 {
                    id: "menu".to_owned(),
                    platform: "portable".to_owned(),
                    required: true,
                    description: "Main menu".to_owned(),
                },
                CoverageSurfaceV1 {
                    id: "lobby".to_owned(),
                    platform: "portable".to_owned(),
                    required: true,
                    description: "Lobby".to_owned(),
                },
            ],
            transitions: vec![CoverageTransitionV1 {
                id: "menu_to_lobby".to_owned(),
                from: "menu".to_owned(),
                action: "start".to_owned(),
                to: "lobby".to_owned(),
                platform: "portable".to_owned(),
                required: true,
            }],
            exclusions: vec![],
        }
    }

    #[test]
    fn rejects_unknown_transition_surfaces() {
        let mut value = contract();
        value.transitions[0].to = "missing".to_owned();
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_surface_and_transition_id_collisions() {
        let mut value = contract();
        value.transitions[0].id = value.surfaces[0].id.clone();
        assert!(value.validate().is_err());
    }

    #[test]
    fn requires_exact_execution_accounting() {
        let value = contract();
        let report = CoverageReportV1 {
            schema_version: COVERAGE_REPORT_SCHEMA_VERSION,
            contract_sha256: "a".repeat(64),
            planned_ids: value.required_ids(),
            executed_ids: value.required_ids(),
            passed_ids: value.required_ids(),
            failed_ids: vec![],
            blocked_ids: vec![],
        };
        assert!(report.validate_against(&value).is_ok());
        assert!(report.is_complete_pass(&value));
    }

    #[test]
    fn rejects_unclassified_execution() {
        let value = contract();
        let report = CoverageReportV1 {
            schema_version: COVERAGE_REPORT_SCHEMA_VERSION,
            contract_sha256: "a".repeat(64),
            planned_ids: value.required_ids(),
            executed_ids: value.required_ids(),
            passed_ids: vec!["menu".to_owned()],
            failed_ids: vec![],
            blocked_ids: vec![],
        };
        assert!(report.validate_against(&value).is_err());
    }

    #[test]
    fn allows_explicit_blocked_planned_ids_without_calling_them_executed() {
        let value = contract();
        let required = value.required_ids();
        let report = CoverageReportV1 {
            schema_version: COVERAGE_REPORT_SCHEMA_VERSION,
            contract_sha256: "a".repeat(64),
            planned_ids: required.clone(),
            executed_ids: vec![required[0].clone()],
            passed_ids: vec![required[0].clone()],
            failed_ids: vec![],
            blocked_ids: required.into_iter().skip(1).collect(),
        };
        assert!(report.validate_against(&value).is_ok());
        assert!(!report.is_complete_pass(&value));
    }
}
