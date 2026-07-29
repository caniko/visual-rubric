use std::fs;

use visual_rubric::{
    ArtifactDigest, CalibrationCorpusV1, CalibrationExpectation, FINDING_SCHEMA_VERSION, FindingV1,
    OBSERVATION_SCHEMA_VERSION, ObservationV1, VISUAL_RUN_REPORT_SCHEMA_VERSION, VisualRunReportV1,
    VisualRunStatus, finding_fingerprint, validate_calibration_sentinels, validate_visual_run,
};

fn corpus() -> CalibrationCorpusV1 {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/calibration/corpus.json");
    serde_json::from_str(&fs::read_to_string(path).expect("calibration corpus"))
        .expect("corpus JSON")
}

#[test]
fn checked_in_corpus_covers_every_required_defect_class() {
    corpus().validate().expect("valid calibration corpus");
}

#[test]
fn calibration_report_requires_every_known_bad_sentinel() {
    let corpus = corpus();
    let mut observations = Vec::new();
    let mut findings = Vec::new();
    for sentinel in &corpus.sentinels {
        let CalibrationExpectation::Finding { observation_kind } = &sentinel.expectation else {
            continue;
        };
        let observation = ObservationV1 {
            schema_version: OBSERVATION_SCHEMA_VERSION,
            id: format!("observation/{}", sentinel.id),
            capture_id: sentinel.capture_id.clone(),
            kind: observation_kind.clone(),
            summary: format!("calibration detector found {observation_kind}"),
            evidence: Vec::new(),
        };
        let fingerprint = finding_fingerprint(&corpus.target, std::slice::from_ref(&observation));
        findings.push(FindingV1 {
            schema_version: FINDING_SCHEMA_VERSION,
            fingerprint,
            target: corpus.target.clone(),
            revision: corpus.revision.clone(),
            title: format!("calibration {observation_kind}"),
            observation_ids: vec![observation.id.clone()],
            reproduction: vec![format!("capture {}", sentinel.capture_id)],
        });
        observations.push(observation);
    }

    let mut report = VisualRunReportV1 {
        schema_version: VISUAL_RUN_REPORT_SCHEMA_VERSION,
        run_id: "calibration-run".to_owned(),
        target: corpus.target.clone(),
        revision: corpus.revision.clone(),
        dirty: false,
        capture_manifest: ArtifactDigest {
            path: "capture_manifest.json".into(),
            sha256: "0".repeat(64),
            bytes: 1,
        },
        planned_capture_ids: corpus.cells.iter().map(|cell| cell.id.clone()).collect(),
        evaluated_capture_ids: corpus.cells.iter().map(|cell| cell.id.clone()).collect(),
        status: VisualRunStatus::Fail,
        observations,
        findings,
    };
    validate_visual_run(&report).expect("complete calibration report");
    validate_calibration_sentinels(&report, &corpus.sentinels)
        .expect("every sentinel is represented");

    report.findings.pop();
    let issues = validate_calibration_sentinels(&report, &corpus.sentinels)
        .expect_err("a missed sentinel must fail calibration");
    assert!(issues.iter().any(|issue| issue.contains("did not produce")));
}
