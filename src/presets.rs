//! Question preset interface for standardized rubric questions.
//!
//! A "preset" is a named set of one or more rubric questions that end-users
//! can select via `--preset <name>` instead of providing `--question` on
//! every invocation.  Implement [`QuestionPreset`] and register it in
//! [`resolve`] to make a preset available.

use std::fmt;

/// A named preset that produces one or more standardized rubric questions.
///
/// Implement this trait to register a preset that end-users can select
/// via `--preset <name>` instead of providing `--question` directly.
pub trait QuestionPreset {
    /// Unique identifier used as the `--preset` argument value.
    fn name(&self) -> &'static str;
    /// The question text(s) this preset produces.
    fn questions(&self) -> Vec<String>;
}

/// Errors from preset name resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetError {
    /// The named preset was not found.
    NotFound {
        /// The unrecognised preset name.
        name: String,
    },
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { name } => {
                write!(f, "unknown question preset {name:?}")
            }
        }
    }
}

impl std::error::Error for PresetError {}

/// Resolves a preset name to its question text(s).
///
/// Implementations should match `name` against registered presets and defer
/// to [`QuestionPreset::questions`].
pub fn resolve(name: &str) -> Result<Vec<String>, PresetError> {
    Err(PresetError::NotFound {
        name: name.to_owned(),
    })
}
