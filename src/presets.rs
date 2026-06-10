//! Question preset interface for standardized rubric questions.
//!
//! A "preset" is a named set of one or more rubric questions that end-users
//! can select via `--preset <name>` instead of providing `--question` on
//! every invocation.  A preset may also carry a standard system prompt that
//! is used when the caller does not override `--system-prompt`.  Implement
//! [`QuestionPreset`] and register it in [`all`] to make a preset available.

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
    /// Standard system prompt paired with this preset's questions.
    ///
    /// Used when the caller does not provide an explicit system prompt.
    fn system_prompt(&self) -> Option<&'static str> {
        None
    }
}

/// Question for [`PlinthWebsite`]: plinth-generated project website audits.
pub const PLINTH_WEBSITE_QUESTION: &str = "\
Does this page make the install section easy to find, choose from, and act on without layout or \
responsive UX defects?";

/// System prompt for [`PlinthWebsite`].
pub const PLINTH_WEBSITE_SYSTEM_PROMPT: &str = "\
You are auditing a software project website install section. Focus on install flow clarity, \
scanability, heading hierarchy, call-to-action placement, prerequisite visibility, command-copy \
ergonomics, responsive layout, text clipping, overlapping UI, and whether the next step is obvious. \
Reply with strict JSON matching this schema and nothing else:
{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }";

/// Question for [`ChessbenderUi`]: generic Chessbender screenshot checks.
pub const CHESSBENDER_UI_QUESTION: &str = "\
Checklist: the visible game UI is complete and readable. Fail for text clipped or overflowing \
its container, overlapping interactive elements, missing or blank regions where content should \
appear, illegible contrast, or visibly broken layout.";

/// System prompt for [`ChessbenderUi`].
pub const CHESSBENDER_UI_SYSTEM_PROMPT: &str = "\
You are a UI regression auditor for a turn-based strategy game called Chessbender. \
You will be shown one screenshot and asked a specific question. Reply with strict \
JSON matching this schema and nothing else:
{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }
Fail criteria: text clipped or overflowing its container, overlapping interactive \
elements, missing/blank regions where content should appear, illegible contrast, \
visibly broken layout. Cosmetic differences from previous runs are NOT failures \
unless they make the UI worse by the criteria above.";

/// Question for [`SyndbFigures`]: SynDB manuscript figure QA.
pub const SYNDB_FIGURES_QUESTION: &str =
    "Does this SynDB manuscript figure asset pass publication visual QA?";

/// System prompt for [`SyndbFigures`].
pub const SYNDB_FIGURES_SYSTEM_PROMPT: &str = "\
You are auditing SynDB scientific manuscript figure PNGs at their rendered print size. \
Reply with strict JSON only:
{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }
Do not call tools, inspect files, run commands, or browse. Evaluate only the supplied image. \
Fail if any of these are visible: clipped text or labels, overlapping labels or \
plot marks, illegible axis/legend/caption text, poor contrast for important text, \
blank or placeholder panels, broken legends, missing axes where axes are expected, \
malformed TikZ/rasterization artifacts, cropped arrows/connectors, or composite \
assembly errors such as missing panels, wrong panel order, or inconsistent panel \
lettering. When an asset fails, phrase anomalies so they identify the reusable \
failure class when possible, such as bottom legend clipping, left label margin, \
heatmap label density, annotation collision, TikZ crop margin, or composite \
assembly. Do not fail for minor aesthetic preferences when the asset is readable \
and complete.";

/// Preset `plinth-website`: install-section UX audit for plinth project sites.
pub struct PlinthWebsite;

impl QuestionPreset for PlinthWebsite {
    fn name(&self) -> &'static str {
        "plinth-website"
    }

    fn questions(&self) -> Vec<String> {
        vec![PLINTH_WEBSITE_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(PLINTH_WEBSITE_SYSTEM_PROMPT)
    }
}

/// Preset `chessbender-ui`: generic UI regression check for Chessbender
/// (regicide) screenshots without a scenario-specific checklist.
pub struct ChessbenderUi;

impl QuestionPreset for ChessbenderUi {
    fn name(&self) -> &'static str {
        "chessbender-ui"
    }

    fn questions(&self) -> Vec<String> {
        vec![CHESSBENDER_UI_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(CHESSBENDER_UI_SYSTEM_PROMPT)
    }
}

/// Preset `syndb-figures`: publication QA for SynDB manuscript figure PNGs.
pub struct SyndbFigures;

impl QuestionPreset for SyndbFigures {
    fn name(&self) -> &'static str {
        "syndb-figures"
    }

    fn questions(&self) -> Vec<String> {
        vec![SYNDB_FIGURES_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(SYNDB_FIGURES_SYSTEM_PROMPT)
    }
}

/// Errors from preset name resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
                let available = all().map(QuestionPreset::name).join(", ");
                write!(
                    f,
                    "unknown question preset {name:?} (available: {available})"
                )
            }
        }
    }
}

impl std::error::Error for PresetError {}

/// Returns every registered preset.
#[must_use]
pub fn all() -> [&'static dyn QuestionPreset; 3] {
    [&PlinthWebsite, &ChessbenderUi, &SyndbFigures]
}

/// Resolves a preset name to the registered preset.
///
/// # Errors
///
/// Returns [`PresetError::NotFound`] when no preset has that name.
pub fn find(name: &str) -> Result<&'static dyn QuestionPreset, PresetError> {
    all()
        .into_iter()
        .find(|preset| preset.name() == name)
        .ok_or_else(|| PresetError::NotFound {
            name: name.to_owned(),
        })
}

/// Resolves a preset name to its question text(s).
///
/// # Errors
///
/// Returns [`PresetError::NotFound`] when no preset has that name.
pub fn resolve(name: &str) -> Result<Vec<String>, PresetError> {
    find(name).map(QuestionPreset::questions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names_are_unique() {
        let mut names: Vec<_> = all().map(QuestionPreset::name).to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all().len());
    }

    #[test]
    fn resolves_each_registered_preset() {
        for preset in all() {
            let questions = resolve(preset.name()).expect("registered preset resolves");
            assert_eq!(questions, preset.questions());
            assert!(!questions.is_empty());
        }
    }

    #[test]
    fn registered_presets_carry_system_prompts() {
        for preset in all() {
            let prompt = preset.system_prompt().expect("preset has a system prompt");
            assert!(
                prompt.contains("verdict"),
                "{} prompt demands JSON",
                preset.name()
            );
        }
    }

    #[test]
    fn unknown_preset_error_lists_available_names() {
        let error = resolve("nope").expect_err("unknown preset must fail");
        let message = error.to_string();
        assert!(message.contains("\"nope\""));
        assert!(message.contains("plinth-website"));
        assert!(message.contains("chessbender-ui"));
        assert!(message.contains("syndb-figures"));
    }
}
