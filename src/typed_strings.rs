use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize};

/// Validated rubric verdict status.
#[derive(Clone, Debug, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RubricVerdictStatus(String);

impl RubricVerdictStatus {
    /// Returns the status as the JSON wire value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns true when the status is `pass`.
    #[must_use]
    pub fn is_pass(&self) -> bool {
        self.0 == "pass"
    }
}

impl<'de> Deserialize<'de> for RubricVerdictStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "pass" | "fail" => Ok(Self(value)),
            _ => Err(serde::de::Error::custom(format!(
                "unknown rubric verdict status {value:?}"
            ))),
        }
    }
}

impl From<&str> for RubricVerdictStatus {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for RubricVerdictStatus {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Deref for RubricVerdictStatus {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for RubricVerdictStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<&str> for RubricVerdictStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Validated Codex ACP reasoning effort value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct RubricEffort(String);

impl RubricEffort {
    /// Returns the effort as the Codex ACP wire value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<&str> for RubricEffort {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for RubricEffort {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Deref for RubricEffort {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for RubricEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
