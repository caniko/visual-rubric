//! Calibration-corpus gate for deterministic browser evidence.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

use super::CalibrateArgs;
use crate::{
    CalibrationCorpusV1, CaptureManifest, VisualRunReportV1, validate_calibration_sentinels,
    validate_visual_run_against_manifest,
};

pub(super) fn run_calibration(args: CalibrateArgs) -> Result<()> {
    let workspace = std::env::current_dir().context("read calibration workspace")?;
    let root = workspace
        .canonicalize()
        .context("canonicalize calibration workspace")?;
    let corpus_path = absolute_path(&workspace, &args.corpus);
    let manifest_path = absolute_path(&workspace, &args.manifest);
    let report_path = absolute_path(&workspace, &args.report);

    let corpus: CalibrationCorpusV1 = serde_json::from_slice(
        &fs::read(&corpus_path)
            .with_context(|| format!("read calibration corpus {}", corpus_path.display()))?,
    )
    .with_context(|| format!("parse calibration corpus {}", corpus_path.display()))?;
    corpus
        .validate()
        .map_err(|issues| anyhow!("calibration corpus is invalid: {}", issues.join("; ")))?;

    let manifest = CaptureManifest::load(&manifest_path, &root)
        .map_err(|error| anyhow!("load calibration manifest: {error}"))?;
    let report: VisualRunReportV1 = serde_json::from_slice(
        &fs::read(&report_path)
            .with_context(|| format!("read deterministic report {}", report_path.display()))?,
    )
    .with_context(|| format!("parse deterministic report {}", report_path.display()))?;

    validate_visual_run_against_manifest(&report, &manifest)
        .map_err(|issues| anyhow!("deterministic report is invalid: {}", issues.join("; ")))?;
    validate_calibration_sentinels(&report, &corpus.sentinels)
        .map_err(|issues| anyhow!("calibration gate failed: {}", issues.join("; ")))?;

    if report.dirty {
        bail!("calibration report is blocked because the producer revision is dirty");
    }
    println!(
        "calibration passed: {} sentinels across {} captures",
        corpus.sentinels.len(),
        corpus.cells.len()
    );
    Ok(())
}

fn absolute_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}
