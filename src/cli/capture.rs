//! Shared browser capture and rubric-batch command.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::CaptureArgs;
use super::static_server::StaticServer;
use crate::browser::{BrowserSession, write_json};
use crate::{
    ArtifactDigest, AssetChange, BatchRubricConfig, BatchRubricRun,
    COVERAGE_CONTRACT_SCHEMA_VERSION, COVERAGE_REPORT_SCHEMA_VERSION, CaptureCell,
    CaptureEnvironment, CaptureManifest, CoverageContractV1, CoverageReportV1, CoverageSurfaceV1,
    CoverageTransitionV1, DeterministicEvidenceV1, FindingV1, OBSERVATION_SCHEMA_VERSION,
    ObservationV1, RubricOptions, SelectionMode, VisualRunReportV1, VisualRunStatus,
    finding_fingerprint, validate_visual_run_against_manifest,
};

const JOB_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize)]
struct CaptureJob {
    schema_version: u32,
    target: String,
    revision: String,
    dirty: bool,
    cells: Vec<JobCell>,
}

#[derive(Clone, Debug, Deserialize)]
struct JobCell {
    id: String,
    path: String,
    state: String,
    viewport: crate::Viewport,
    theme: String,
    locale: String,
    #[serde(default)]
    presets: Vec<String>,
    #[serde(default)]
    required_controls: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct BatchArtifactReport {
    schema_version: u32,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<Value>,
}

pub(super) fn run_capture(args: CaptureArgs) -> Result<()> {
    if args.fake_pass && args.skip_ai {
        bail!("--fake-pass and --skip-ai cannot be used together");
    }
    let job = read_job(&args.job)?;
    let question = args.questions.resolve().map_err(|error| anyhow!(error))?;
    let invocation_dir = std::env::current_dir().context("read capture workspace")?;
    let workspace = args
        .workspace
        .as_deref()
        .map(|path| absolute_path(&invocation_dir, path))
        .unwrap_or(invocation_dir);
    let root = workspace
        .canonicalize()
        .context("canonicalize capture workspace")?;
    let output = absolute_path(&workspace, &args.output);
    let manifest_path = absolute_path(&workspace, &args.manifest);
    let report_path = absolute_path(&workspace, &args.report);
    ensure_distinct_output_root(&root, &output)?;
    create_clean_dir(&output)?;

    let server = args
        .base_url
        .is_none()
        .then(|| StaticServer::start(args.root.clone(), 0))
        .transpose()?;
    let served_url = if let Some(base_url) = args.base_url.as_deref() {
        normalize_base_url(base_url)?
    } else {
        let base_url = server.as_ref().expect("static server exists").base_url();
        normalize_base_url(&base_url)?
    };
    let mut browser = BrowserSession::start(&args.browser, &args.browser_args)?;
    let mut captures = Vec::with_capacity(job.cells.len());
    let mut image_paths = Vec::with_capacity(job.cells.len());

    for (index, cell) in job.cells.iter().enumerate() {
        let url = hosted_url(&served_url, &cell.path)?;
        let captured = browser
            .capture(
                &url,
                &cell.viewport,
                &cell.theme,
                &cell.locale,
                &cell.required_controls,
            )
            .with_context(|| format!("capture visual cell {} at {url}", cell.id))?;
        let cell_dir = output.join(format!("{:03}-{}", index, safe_component(&cell.id)));
        fs::create_dir_all(&cell_dir)
            .with_context(|| format!("create capture directory {}", cell_dir.display()))?;
        let image_path = cell_dir.join("full-page.png");
        let viewport_path = cell_dir.join("viewport.png");
        let dom_path = cell_dir.join("dom_snapshot.json");
        let accessibility_path = cell_dir.join("accessibility.json");
        let readiness_path = cell_dir.join("readiness.json");
        let deterministic_path = cell_dir.join("deterministic.json");
        let console_path = cell_dir.join("console.json");

        fs::write(&image_path, captured.full_page_png)
            .with_context(|| format!("write {}", image_path.display()))?;
        fs::write(&viewport_path, captured.viewport_png)
            .with_context(|| format!("write {}", viewport_path.display()))?;
        write_json(&dom_path, &captured.dom_snapshot)?;
        write_json(&accessibility_path, &captured.accessibility)?;
        write_json(&readiness_path, &captured.readiness)?;
        write_json(&deterministic_path, &captured.deterministic)?;
        write_json(&console_path, &Value::Array(captured.console))?;

        let mut metadata = BTreeMap::new();
        metadata.insert(
            "viewport".to_owned(),
            artifact_digest(&root, &viewport_path)?,
        );
        metadata.insert(
            "dom_snapshot".to_owned(),
            artifact_digest(&root, &dom_path)?,
        );
        metadata.insert(
            "accessibility".to_owned(),
            artifact_digest(&root, &accessibility_path)?,
        );
        metadata.insert(
            "readiness".to_owned(),
            artifact_digest(&root, &readiness_path)?,
        );
        metadata.insert(
            "deterministic".to_owned(),
            artifact_digest(&root, &deterministic_path)?,
        );
        metadata.insert("console".to_owned(), artifact_digest(&root, &console_path)?);
        captures.push(CaptureCell {
            id: cell.id.clone(),
            image: artifact_digest(&root, &image_path)?,
            state: cell.state.clone(),
            viewport: cell.viewport.clone(),
            theme: cell.theme.clone(),
            locale: cell.locale.clone(),
            metadata,
            presets: cell.presets.clone(),
        });
        image_paths.push(image_path);
    }

    let coverage_dir = manifest_path
        .parent()
        .context("capture manifest must have a parent directory")?
        .join("coverage");
    fs::create_dir_all(&coverage_dir)
        .with_context(|| format!("create coverage directory {}", coverage_dir.display()))?;
    let contract = coverage_contract(&job);
    contract.validate().map_err(|issues| {
        anyhow!(
            "generated coverage contract is invalid: {}",
            issues.join("; ")
        )
    })?;
    let contract_bytes = serde_json::to_vec(&contract)?;
    let contract_hash = sha256_bytes(&contract_bytes);
    let contract_path = coverage_dir.join("contract.json");
    write_bytes(&contract_path, &serde_json::to_vec_pretty(&contract)?)?;
    let required_ids = contract.required_ids();
    let coverage = CoverageReportV1 {
        schema_version: COVERAGE_REPORT_SCHEMA_VERSION,
        contract_sha256: contract_hash,
        planned_ids: required_ids.clone(),
        executed_ids: required_ids.clone(),
        passed_ids: required_ids,
        failed_ids: Vec::new(),
        blocked_ids: Vec::new(),
    };
    coverage.validate_against(&contract).map_err(|issues| {
        anyhow!(
            "generated coverage report is invalid: {}",
            issues.join("; ")
        )
    })?;
    let coverage_path = coverage_dir.join("report.json");
    write_bytes(&coverage_path, &serde_json::to_vec_pretty(&coverage)?)?;

    let manifest = CaptureManifest {
        schema_version: crate::CAPTURE_MANIFEST_SCHEMA_VERSION,
        target: job.target.clone(),
        revision: job.revision.clone(),
        dirty: job.dirty,
        environment: CaptureEnvironment {
            platform: std::env::consts::OS.to_owned(),
            renderer: "chromium".to_owned(),
            capture_backend: "cdp-pipe".to_owned(),
        },
        declared_cells: captures.len(),
        captures,
        coverage_contract: Some(artifact_digest(&root, &contract_path)?),
        coverage_report: Some(artifact_digest(&root, &coverage_path)?),
    };
    manifest
        .validate(&root)
        .map_err(|error| anyhow!("capture manifest validation failed: {error}"))?;
    write_bytes(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;

    let deterministic_report = build_deterministic_report(&root, &manifest, &manifest_path, &job)?;
    let deterministic_report_path = report_path
        .parent()
        .context("producer report must have a parent directory")?
        .join("deterministic_report.json");
    write_bytes(
        &deterministic_report_path,
        &serde_json::to_vec_pretty(&deterministic_report)?,
    )?;

    let (rubric_values, batch_artifact) = if args.fake_pass {
        (
            image_paths
                .iter()
                .map(|_| {
                    json!({
                        "status": "pass",
                        "reason": "fake pass requested",
                        "anomalies": []
                    })
                })
                .collect::<Vec<_>>(),
            BatchArtifactReport {
                schema_version: 1,
                mode: "fake_pass",
                report: None,
            },
        )
    } else if args.skip_ai {
        (
            image_paths
                .iter()
                .map(|_| json!({"status": "skipped", "reason": "AI rubric skipped by flag"}))
                .collect::<Vec<_>>(),
            BatchArtifactReport {
                schema_version: 1,
                mode: "skip_ai",
                report: None,
            },
        )
    } else {
        let batch = run_batch(&args, &question, &image_paths)?;
        let values = batch
            .assets
            .iter()
            .filter(|asset| asset.selected)
            .map(|asset| serde_json::to_value(&asset.result))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        (
            values,
            BatchArtifactReport {
                schema_version: 1,
                mode: "batch",
                report: Some(serde_json::to_value(batch)?),
            },
        )
    };

    let batch_path = report_path
        .parent()
        .context("producer report must have a parent directory")?
        .join("rubric_batch_report.json");
    write_bytes(&batch_path, &serde_json::to_vec_pretty(&batch_artifact)?)?;
    let cells = manifest
        .captures
        .iter()
        .zip(rubric_values.iter())
        .map(|(capture, rubric)| {
            json!({
                "id": capture.id,
                "image": capture.image.path,
                "state": capture.state,
                "viewport": capture.viewport,
                "theme": capture.theme,
                "locale": capture.locale,
                "rubric": rubric,
                "status": rubric.get("status").and_then(Value::as_str).unwrap_or("error"),
            })
        })
        .collect::<Vec<_>>();
    if cells.len() != manifest.captures.len() {
        bail!(
            "rubric produced {} results for {} captures",
            cells.len(),
            manifest.captures.len()
        );
    }
    let passed_cells = cells
        .iter()
        .filter(|cell| cell.get("status").and_then(Value::as_str) == Some("pass"))
        .count();
    let failed_cells = cells
        .iter()
        .filter(|cell| cell.get("status").and_then(Value::as_str) == Some("fail"))
        .count();
    let error_cells = cells.len().saturating_sub(passed_cells + failed_cells);
    let failures = cells
        .iter()
        .filter(|cell| cell.get("status").and_then(Value::as_str) == Some("fail"))
        .map(|cell| json!({"id": cell["id"], "rubric": cell["rubric"]}))
        .collect::<Vec<_>>();
    let errors = cells
        .iter()
        .filter(|cell| {
            cell.get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status != "pass" && status != "fail")
        })
        .map(|cell| json!({"id": cell["id"], "rubric": cell["rubric"]}))
        .collect::<Vec<_>>();
    let report = json!({
        "schema_version": 3,
        "target": job.target,
        "git": {"sha": job.revision, "dirty": job.dirty},
        "capture_manifest": relative_path(&root, &manifest_path)?,
        "capture_environment": {
            "browser": browser.browser_product(),
            "renderer": "chromium",
            "capture_backend": "cdp-pipe",
        },
        "served_url": served_url,
        "rubric_batch_report": artifact_digest(&root, &batch_path)?,
        "deterministic_report": artifact_digest(&root, &deterministic_report_path)?,
        "cells": cells,
        "summary": {
            "total_cells": manifest.captures.len(),
            "passed_cells": passed_cells,
            "failed_cells": failed_cells,
            "error_cells": error_cells,
        },
        "failures": failures,
        "errors": errors,
        "rate_limit_events": [],
    });
    write_bytes(&report_path, &serde_json::to_vec_pretty(&report)?)?;
    if failed_cells > 0 || error_cells > 0 {
        bail!(
            "visual capture rubric completed with {failed_cells} failures and {error_cells} errors"
        );
    }
    Ok(())
}

fn build_deterministic_report(
    root: &Path,
    manifest: &CaptureManifest,
    manifest_path: &Path,
    job: &CaptureJob,
) -> Result<VisualRunReportV1> {
    let mut observations = Vec::new();
    let mut findings = Vec::new();
    for capture in &manifest.captures {
        let artifact = capture
            .metadata
            .get("deterministic")
            .with_context(|| format!("capture {} has no deterministic evidence", capture.id))?;
        let path = root.join(&artifact.path);
        let evidence: DeterministicEvidenceV1 = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse deterministic evidence {}", path.display()))?;
        evidence.validate().map_err(|issues| {
            anyhow!(
                "deterministic evidence {} is invalid: {}",
                path.display(),
                issues.join("; ")
            )
        })?;
        for (index, deterministic) in evidence.observations.iter().enumerate() {
            let observation = ObservationV1 {
                schema_version: OBSERVATION_SCHEMA_VERSION,
                id: format!("deterministic/{}/{}", safe_component(&capture.id), index),
                capture_id: capture.id.clone(),
                kind: deterministic.kind.clone(),
                summary: deterministic.summary.clone(),
                evidence: vec![artifact.clone()],
            };
            let fingerprint = finding_fingerprint(&job.target, std::slice::from_ref(&observation));
            findings.push(FindingV1 {
                schema_version: crate::FINDING_SCHEMA_VERSION,
                fingerprint,
                target: job.target.clone(),
                revision: job.revision.clone(),
                title: deterministic.kind.clone(),
                observation_ids: vec![observation.id.clone()],
                reproduction: vec![format!("capture {}", capture.id)],
            });
            observations.push(observation);
        }
    }
    let status = if job.dirty {
        VisualRunStatus::Blocked
    } else if findings.is_empty() {
        VisualRunStatus::Pass
    } else {
        VisualRunStatus::Fail
    };
    let report = VisualRunReportV1 {
        schema_version: crate::VISUAL_RUN_REPORT_SCHEMA_VERSION,
        run_id: format!("deterministic-{}", safe_component(&job.target)),
        target: job.target.clone(),
        revision: job.revision.clone(),
        dirty: job.dirty,
        capture_manifest: artifact_digest(root, manifest_path)?,
        planned_capture_ids: manifest
            .captures
            .iter()
            .map(|capture| capture.id.clone())
            .collect(),
        evaluated_capture_ids: manifest
            .captures
            .iter()
            .map(|capture| capture.id.clone())
            .collect(),
        status,
        observations,
        findings,
    };
    validate_visual_run_against_manifest(&report, manifest)
        .map_err(|issues| anyhow!("deterministic report is invalid: {}", issues.join("; ")))?;
    Ok(report)
}

fn read_job(path: &Path) -> Result<CaptureJob> {
    let bytes = fs::read(path).with_context(|| format!("read capture job {}", path.display()))?;
    let job: CaptureJob = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse capture job {}", path.display()))?;
    if job.schema_version != JOB_SCHEMA_VERSION {
        bail!(
            "unsupported capture job schema_version {}; expected {}",
            job.schema_version,
            JOB_SCHEMA_VERSION
        );
    }
    if job.target.trim().is_empty() || job.revision.trim().is_empty() {
        bail!("capture job target and revision must not be empty");
    }
    if job.cells.is_empty() {
        bail!("capture job must contain at least one cell");
    }
    let mut ids = BTreeSet::new();
    for cell in &job.cells {
        if cell.id.trim().is_empty() || !ids.insert(cell.id.as_str()) {
            bail!(
                "capture job contains an empty or duplicate cell id {:?}",
                cell.id
            );
        }
        if cell.path.trim().is_empty() {
            bail!("capture job cell {:?} has an empty path", cell.id);
        }
        if cell.state.trim().is_empty()
            || cell.theme.trim().is_empty()
            || cell.locale.trim().is_empty()
        {
            bail!(
                "capture job cell {:?} must define state, theme, and locale",
                cell.id
            );
        }
        if cell
            .required_controls
            .iter()
            .any(|selector| selector.trim().is_empty())
        {
            bail!(
                "capture job cell {:?} has an empty required control selector",
                cell.id
            );
        }
    }
    Ok(job)
}

fn run_batch(
    args: &CaptureArgs,
    question: &str,
    image_paths: &[PathBuf],
) -> Result<crate::BatchRubricReport> {
    let pool_defaults = crate::PoolConfig::default();
    let mut options: RubricOptions = crate::default_options();
    if args.model.is_some() {
        options.model = args.model.clone();
    }
    if args.effort.is_some() {
        options.effort = args.effort.clone().map(Into::into);
    }
    if args.system_prompt.is_some() {
        options.system_prompt = args.system_prompt.clone();
    } else if let Some(prompt) = args
        .questions
        .resolve_system_prompt()
        .map_err(|e| anyhow!(e))?
    {
        options.system_prompt = Some(prompt);
    }
    let pool = crate::PoolConfig {
        workers: args.rubric_workers.max(1),
        codex_acp_binary: args
            .codex_acp
            .clone()
            .unwrap_or_else(|| pool_defaults.codex_acp_binary.clone()),
        default_options: options,
        ..pool_defaults
    };
    let changes = image_paths
        .iter()
        .cloned()
        .map(AssetChange::Added)
        .collect::<Vec<_>>();
    Ok(BatchRubricRun::new(BatchRubricConfig {
        pool,
        question: question.to_owned(),
        selection_mode: SelectionMode::IncludeUnchanged,
        cache_dir: args.cache_dir.clone(),
        classifier: None,
    })
    .run(&changes))
}

fn coverage_contract(job: &CaptureJob) -> CoverageContractV1 {
    let surfaces = job
        .cells
        .iter()
        .map(|cell| CoverageSurfaceV1 {
            id: format!("surface/{}", cell.id),
            platform: "web".to_owned(),
            required: true,
            description: format!("Rendered visual state at {}", cell.path),
        })
        .collect::<Vec<_>>();
    let transitions = job
        .cells
        .iter()
        .map(|cell| CoverageTransitionV1 {
            id: format!("load/{}", cell.id),
            from: String::new(),
            action: format!("load {}", cell.path),
            to: format!("surface/{}", cell.id),
            platform: "web".to_owned(),
            required: true,
        })
        .collect::<Vec<_>>();
    CoverageContractV1 {
        schema_version: COVERAGE_CONTRACT_SCHEMA_VERSION,
        target: job.target.clone(),
        revision: job.revision.clone(),
        surfaces,
        transitions,
        exclusions: Vec::new(),
    }
}

fn hosted_url(base_url: &str, path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() || path.contains("\n") || path.contains('\r') || path.contains("//") {
        bail!("capture path must be a single local URL path: {path:?}");
    }
    Ok(format!("{}{}", base_url, path.trim_start_matches('/')))
}

fn normalize_base_url(base_url: &str) -> Result<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty()
        || trimmed.contains(['\n', '\r', ' ', '\t'])
        || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
    {
        bail!("capture base URL must be an HTTP(S) URL without whitespace");
    }
    Ok(format!("{}/", trimmed.trim_end_matches('/')))
}

fn create_clean_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("clean capture output {}", path.display()))?;
    }
    fs::create_dir_all(path).with_context(|| format!("create capture output {}", path.display()))
}

fn ensure_distinct_output_root(workspace: &Path, output: &Path) -> Result<()> {
    let output = output
        .canonicalize()
        .unwrap_or_else(|_| output.to_path_buf());
    if output == workspace || output.parent().is_none() {
        bail!("capture output must be a dedicated directory below the workspace");
    }
    if output == Path::new("/") {
        bail!("capture output must not be filesystem root");
    }
    Ok(())
}

fn absolute_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn safe_component(value: &str) -> String {
    let result = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if result.is_empty() {
        "cell".to_owned()
    } else {
        result
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn artifact_digest(root: &Path, path: &Path) -> Result<ArtifactDigest> {
    let bytes = fs::read(path).with_context(|| format!("read artifact {}", path.display()))?;
    Ok(ArtifactDigest {
        path: relative_path(root, path)?,
        sha256: sha256_bytes(&bytes),
        bytes: bytes.len() as u64,
    })
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize artifact root {}", root.display()))?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let relative = path.strip_prefix(&root).map_err(|_| {
        anyhow!(
            "artifact {} escapes workspace {}",
            path.display(),
            root.display()
        )
    })?;
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        bail!(
            "artifact {} escapes workspace {}",
            path.display(),
            root.display()
        );
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize artifact {}", path.display()))?;
    // Cargo workspaces may symlink `target` into a shared build tree. Keep the
    // exception limited to the reserved generated visual subtree.
    let conventional_visual_root = root.join("target/visual").canonicalize().ok();
    let contained = canonical.starts_with(&root)
        || conventional_visual_root
            .as_ref()
            .is_some_and(|visual_root| canonical.starts_with(visual_root));
    if !contained {
        bail!(
            "artifact {} escapes workspace {}",
            path.display(),
            root.display()
        );
    }
    Ok(relative.to_path_buf())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_job_expands_to_required_surface_and_load_ids() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture-job.json");
        let job = read_job(&path).expect("capture fixture job");
        let contract = coverage_contract(&job);
        contract.validate().expect("generated contract");
        assert_eq!(contract.required_ids().len(), job.cells.len() * 2);
    }
}
