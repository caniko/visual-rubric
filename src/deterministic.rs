//! Deterministic browser evidence used before semantic rubric evaluation.

use serde::{Deserialize, Serialize};

/// Current schema version for browser-side deterministic observations.
pub const DETERMINISTIC_EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Evidence collected by the browser without a model call.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeterministicEvidenceV1 {
    /// Schema discriminator.
    pub schema_version: u32,
    /// Normalized observations found in this capture.
    #[serde(default)]
    pub observations: Vec<DeterministicObservationV1>,
}

/// One deterministic observation from a browser capture.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeterministicObservationV1 {
    /// Shared observation class used by the visual-run report.
    pub kind: String,
    /// Factual summary of the evidence.
    pub summary: String,
}

impl DeterministicEvidenceV1 {
    /// Validates the browser-side evidence envelope.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut issues = Vec::new();
        if self.schema_version != DETERMINISTIC_EVIDENCE_SCHEMA_VERSION {
            issues.push(format!(
                "unsupported deterministic evidence schema_version {}; expected {}",
                self.schema_version, DETERMINISTIC_EVIDENCE_SCHEMA_VERSION
            ));
        }
        for (index, observation) in self.observations.iter().enumerate() {
            if observation.kind.trim().is_empty() {
                issues.push(format!(
                    "deterministic observation {index} has an empty kind"
                ));
            }
            if observation.summary.trim().is_empty() {
                issues.push(format!(
                    "deterministic observation {} has an empty summary",
                    observation.kind
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_clear_envelope() {
        DeterministicEvidenceV1 {
            schema_version: DETERMINISTIC_EVIDENCE_SCHEMA_VERSION,
            observations: Vec::new(),
        }
        .validate()
        .expect("clear evidence");
    }

    #[test]
    fn rejects_empty_observation_fields() {
        let issues = DeterministicEvidenceV1 {
            schema_version: DETERMINISTIC_EVIDENCE_SCHEMA_VERSION,
            observations: vec![DeterministicObservationV1 {
                kind: String::new(),
                summary: String::new(),
            }],
        }
        .validate()
        .expect_err("empty fields");
        assert_eq!(issues.len(), 2);
    }
}
