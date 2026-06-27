#![cfg(feature = "audit")]

mod common;

use std::process::Command;

#[cfg(unix)]
#[test]
fn audit_report_records_failures_and_fail_on_rubric_exits_nonzero() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).expect("public");
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").expect("html");
    let browser = temp.path().join("fake-browser");
    common::write_fake_browser(&browser, common::fake_browser_success_script());
    let report = temp.path().join("report.json");

    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("audit")
        .arg("--root")
        .arg(&public)
        .arg("--question")
        .arg("Does it pass?")
        .arg("--browser")
        .arg(&browser)
        .arg("--report")
        .arg(&report)
        .arg("--codex-acp")
        .arg(fake)
        .arg("--viewport")
        .arg("tiny=320x240")
        .arg("--fail-on-rubric")
        .env("FAKE_CODEX_ACP_MODE", "fail")
        .output()
        .expect("run audit");

    assert!(!output.status.success(), "{output:?}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report).expect("report")).expect("json");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["aggregate_status"], "fail");
    assert_eq!(report["screenshots"][0]["rubric"]["status"], "fail");
    assert_eq!(report["screenshots"][0]["rubric"]["reason"], "fake fail");
}

#[cfg(unix)]
#[test]
fn audit_report_records_rubric_errors_without_failing_by_default() {
    let Some(fake) = common::fake_codex_acp_binary() else {
        eprintln!("skipping: fake-codex-acp feature is not enabled");
        return;
    };
    let temp = tempfile::TempDir::new().expect("tempdir");
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).expect("public");
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").expect("html");
    let browser = temp.path().join("fake-browser");
    common::write_fake_browser(&browser, common::fake_browser_success_script());
    let report = temp.path().join("report.json");

    let output = Command::new(env!("CARGO_BIN_EXE_visual-rubric"))
        .arg("audit")
        .arg("--root")
        .arg(&public)
        .arg("--question")
        .arg("Does it pass?")
        .arg("--browser")
        .arg(&browser)
        .arg("--report")
        .arg(&report)
        .arg("--codex-acp")
        .arg(fake)
        .arg("--viewport")
        .arg("tiny=320x240")
        .env("FAKE_CODEX_ACP_MODE", "malformed")
        .output()
        .expect("run audit");

    assert!(output.status.success(), "{output:?}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report).expect("report")).expect("json");
    assert_eq!(report["aggregate_status"], "error");
    assert_eq!(report["screenshots"][0]["rubric"]["status"], "error");
    assert!(
        report["screenshots"][0]["rubric"]["message"]
            .as_str()
            .unwrap()
            .contains("parse rubric verdict")
    );
}
