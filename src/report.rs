//! Page-level and aggregate visual rubric reports.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::RubricVerdict;

/// Result from evaluating one page screenshot.
#[derive(Clone, Debug, Serialize)]
pub struct PageResult {
    /// Human-readable label (e.g. `search`, `entity_plan`).
    pub label: String,
    /// Route or URL that was screenshotted.
    pub route: Option<String>,
    /// Viewport dimensions (width, height) when set.
    pub viewport: Option<(u64, u64)>,
    /// Absolute path to the saved PNG screenshot.
    pub screenshot_path: PathBuf,
    /// Text description produced by the vision model.
    pub vision_description: String,
    /// Rubric verdict from the ACP model.
    pub verdict: RubricVerdict,
}

/// LLM-optimized report aggregating multiple page evaluations.
#[derive(Clone, Debug, Serialize)]
pub struct RubricReport {
    /// ISO 8601 timestamp when the report was generated.
    pub generated_at: String,
    /// Project-level description context.
    pub context: String,
    /// Sorted list of page-level results.
    pub pages: Vec<PageResult>,
    /// Number of passed pages.
    pub passed: usize,
    /// Number of failed pages.
    pub failed: usize,
    /// Total pages evaluated.
    pub total: usize,
}

impl RubricReport {
    /// Build a report from page results and render it.
    #[must_use]
    pub fn from_pages(context: &str, pages: Vec<PageResult>) -> Self {
        let total = pages.len();
        let passed = pages.iter().filter(|p| p.verdict.verdict.is_pass()).count();
        let failed = total - passed;
        let generated_at = {
            let dur = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            format!("{:?}", dur.as_secs())
        };
        Self {
            generated_at,
            context: context.to_string(),
            pages,
            passed,
            failed,
            total,
        }
    }

    /// Render the report as LLM-optimized structured markdown.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Visual Rubric Report\n\n");
        md.push_str(&format!("Generated: {}\n\n", self.generated_at));
        md.push_str(&format!("{}\n\n", self.context));
        md.push_str("## Summary\n\n");
        md.push_str(&format!("- **Total pages**: {}\n", self.total));
        md.push_str(&format!("- **Passed**: {}\n", self.passed));
        md.push_str(&format!("- **Failed**: {}\n", self.failed));
        md.push_str(&format!(
            "- **Pass rate**: {}%\n\n",
            if self.total > 0 {
                (self.passed * 100) / self.total
            } else {
                0
            }
        ));

        self.push_defect_patterns(&mut md);
        self.push_page_details(&mut md);
        md
    }

    fn push_defect_patterns(&self, md: &mut String) {
        let mut anomaly_counts: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for page in &self.pages {
            for anomaly in &page.verdict.anomalies {
                let key = anomaly.split('.').next().unwrap_or(anomaly).to_string();
                anomaly_counts.entry(key).or_default().push(&page.label);
            }
        }
        if anomaly_counts.is_empty() {
            return;
        }

        md.push_str("### Defect Patterns\n\n");
        md.push_str("| Pattern | Count | Pages |\n");
        md.push_str("|---------|-------|-------|\n");
        for (pattern, labels) in &anomaly_counts {
            let pages_list = labels
                .iter()
                .map(|l| format!("`{}`", l))
                .collect::<Vec<_>>()
                .join(", ");
            md.push_str(&format!(
                "| {} | {} | {} |\n",
                pattern,
                labels.len(),
                pages_list
            ));
        }
        md.push('\n');
    }

    fn push_page_details(&self, md: &mut String) {
        for page in &self.pages {
            let status = if page.verdict.verdict.is_pass() {
                "PASS"
            } else {
                "FAIL"
            };
            md.push_str(&format!("---\n\n### {}: {}\n\n", status, page.label));
            if let Some(ref route) = page.route {
                md.push_str(&format!("**Route:** `{}`\n\n", route));
            }
            md.push_str(&format!(
                "**Screenshot:** `{}`\n\n",
                page.screenshot_path.display()
            ));
            if let Some((w, h)) = page.viewport {
                md.push_str(&format!("**Viewport:** {}×{}\n\n", w, h));
            }
            md.push_str("**Vision Description:**\n\n");
            md.push_str(&format!(
                "> {}\n\n",
                page.vision_description.replace('\n', "\n> ")
            ));
            md.push_str(&format!("**Rubric Result:** {}\n\n", status));
            md.push_str(&format!("**Reason:** {}\n\n", page.verdict.reason));
            if !page.verdict.anomalies.is_empty() {
                md.push_str("**Anomalies:**\n");
                for anomaly in &page.verdict.anomalies {
                    md.push_str(&format!("- {}\n", anomaly));
                }
                md.push('\n');
            }
        }
    }

    /// Render the report as JSON.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Write markdown report to `path`.
    pub fn save_markdown(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_markdown())
    }

    /// Write JSON report to `path`.
    pub fn save_json(&self, path: &Path) -> std::io::Result<()> {
        let json = self
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }
}
