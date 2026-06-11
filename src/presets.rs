//! Question preset interface for standardized rubric questions.
//!
//! A "preset" is a named set of one or more rubric questions that end-users
//! can select via `--preset <name>` instead of providing `--question` on
//! every invocation.  A preset may also carry a standard system prompt that
//! is used when the caller does not override `--system-prompt`.
//!
//! Presets are deliberately generic — they describe a *kind* of screenshot
//! (an application UI, a website install section, a manuscript figure), not
//! a specific project.  Projects layer their own context on top with
//! [`extend`], or implement [`QuestionPreset`] delegating to a registered
//! preset.  Register new presets in [`all`].

use std::fmt;

/// JSON verdict schema demanded by every preset system prompt.
macro_rules! verdict_schema {
    () => {
        "{ \"verdict\": \"pass\" | \"fail\", \"reason\": string, \"anomalies\": string[] }"
    };
}

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

/// Appends caller-specific context to a preset question or system prompt.
///
/// Presets are generic; use this to layer project context on top, such as
/// naming the application under review or adding extra fail criteria.
#[must_use]
pub fn extend(base: &str, extension: &str) -> String {
    format!("{base}\n\n{extension}")
}

/// Question for [`UiRegression`]: scenario-agnostic UI defect check.
pub const UI_REGRESSION_QUESTION: &str = "\
Checklist: the visible UI is complete and readable. Fail for text clipped or overflowing \
its container, overlapping interactive elements, missing or blank regions where content should \
appear, illegible contrast, or visibly broken layout.";

/// System prompt for [`UiRegression`].
///
/// Also the crate-wide [`crate::DEFAULT_SYSTEM_PROMPT`].
pub const UI_REGRESSION_SYSTEM_PROMPT: &str = concat!(
    "\
You are a UI regression auditor. \
You will be shown one screenshot and asked a specific question. Reply with strict \
JSON matching this schema and nothing else:\n",
    verdict_schema!(),
    "\nFail criteria: text clipped or overflowing its container, overlapping interactive \
elements, missing/blank regions where content should appear, illegible contrast, \
visibly broken layout. Cosmetic differences from previous runs are NOT failures \
unless they make the UI worse by the criteria above."
);

/// Question for [`WebsiteInstall`]: website install-section UX audit.
pub const WEBSITE_INSTALL_QUESTION: &str = "\
Does this page make the install section easy to find, choose from, and act on without layout or \
responsive UX defects?";

/// System prompt for [`WebsiteInstall`].
pub const WEBSITE_INSTALL_SYSTEM_PROMPT: &str = concat!(
    "\
You are auditing a software project website install section. Focus on install flow clarity, \
scanability, heading hierarchy, call-to-action placement, prerequisite visibility, command-copy \
ergonomics, responsive layout, text clipping, overlapping UI, and whether the next step is obvious. \
Reply with strict JSON matching this schema and nothing else:\n",
    verdict_schema!()
);

/// Question for [`ManuscriptFigure`]: publication QA for manuscript figures.
pub const MANUSCRIPT_FIGURE_QUESTION: &str =
    "Does this manuscript figure asset pass publication visual QA?";

/// System prompt for [`ManuscriptFigure`].
pub const MANUSCRIPT_FIGURE_SYSTEM_PROMPT: &str = concat!(
    "\
You are auditing scientific manuscript figure PNGs at their rendered print size. \
Reply with strict JSON only:\n",
    verdict_schema!(),
    "\nDo not call tools, inspect files, run commands, or browse. Evaluate only the supplied image. \
Fail if any of these are visible: clipped text or labels, overlapping labels or \
plot marks, illegible axis/legend/caption text, poor contrast for important text, \
blank or placeholder panels, broken legends, missing axes where axes are expected, \
malformed TikZ/rasterization artifacts, cropped arrows/connectors, or composite \
assembly errors such as missing panels, wrong panel order, or inconsistent panel \
lettering. When an asset fails, phrase anomalies so they identify the reusable \
failure class when possible, such as bottom legend clipping, left label margin, \
heatmap label density, annotation collision, TikZ crop margin, or composite \
assembly. Do not fail for minor aesthetic preferences when the asset is readable \
and complete."
);

/// Preset `ui-regression`: generic defect check for one application UI
/// screenshot without a scenario-specific checklist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiRegression;

impl QuestionPreset for UiRegression {
    fn name(&self) -> &'static str {
        "ui-regression"
    }

    fn questions(&self) -> Vec<String> {
        vec![UI_REGRESSION_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(UI_REGRESSION_SYSTEM_PROMPT)
    }
}

/// Preset `website-install`: install-section UX audit for project websites.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebsiteInstall;

impl QuestionPreset for WebsiteInstall {
    fn name(&self) -> &'static str {
        "website-install"
    }

    fn questions(&self) -> Vec<String> {
        vec![WEBSITE_INSTALL_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(WEBSITE_INSTALL_SYSTEM_PROMPT)
    }
}

/// Preset `manuscript-figure`: publication QA for scientific manuscript
/// figure PNGs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManuscriptFigure;

impl QuestionPreset for ManuscriptFigure {
    fn name(&self) -> &'static str {
        "manuscript-figure"
    }

    fn questions(&self) -> Vec<String> {
        vec![MANUSCRIPT_FIGURE_QUESTION.to_owned()]
    }

    fn system_prompt(&self) -> Option<&'static str> {
        Some(MANUSCRIPT_FIGURE_SYSTEM_PROMPT)
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
    [&UiRegression, &WebsiteInstall, &ManuscriptFigure]
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
    fn registered_presets_carry_system_prompts_with_verdict_schema() {
        for preset in all() {
            let prompt = preset.system_prompt().expect("preset has a system prompt");
            assert!(
                prompt.contains(verdict_schema!()),
                "{} prompt demands the JSON verdict schema",
                preset.name()
            );
        }
    }

    #[test]
    fn preset_texts_stay_project_agnostic() {
        for preset in all() {
            for text in preset
                .questions()
                .iter()
                .map(String::as_str)
                .chain(preset.system_prompt())
            {
                for project in ["plinth", "Chessbender", "SynDB"] {
                    assert!(
                        !text.to_lowercase().contains(&project.to_lowercase()),
                        "{} preset must not mention {project}",
                        preset.name()
                    );
                }
            }
        }
    }

    #[test]
    fn extend_appends_context_paragraph() {
        let extended = extend(UI_REGRESSION_SYSTEM_PROMPT, "The screenshots show MyApp.");
        assert!(extended.starts_with(UI_REGRESSION_SYSTEM_PROMPT));
        assert!(extended.ends_with("\n\nThe screenshots show MyApp."));
    }

    #[test]
    fn unknown_preset_error_lists_available_names() {
        let error = resolve("nope").expect_err("unknown preset must fail");
        let message = error.to_string();
        assert!(message.contains("\"nope\""));
        assert!(message.contains("ui-regression"));
        assert!(message.contains("website-install"));
        assert!(message.contains("manuscript-figure"));
    }
}
