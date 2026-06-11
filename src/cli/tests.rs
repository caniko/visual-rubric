#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use clap::Parser as _;

use super::{AuditReport, AuditStatus, Cli, Commands, ImageArgs, PathBuf, QuestionSource, run};

#[test]
fn parses_custom_system_prompt() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "image",
        "--image",
        "shot.png",
        "--question",
        "Is it readable?",
        "--system-prompt",
        "Use this rubric.",
        "--json",
    ]);
    let Some(Commands::Image(image)) = cli.command else {
        panic!("expected image command");
    };
    assert_eq!(image.system_prompt.as_deref(), Some("Use this rubric."));
    assert!(image.json);
}

#[test]
fn parses_legacy_image_args() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "--image",
        "shot.png",
        "--question",
        "Is it readable?",
    ]);
    assert!(cli.command.is_none());
    let image: ImageArgs = cli.image.try_into().unwrap();
    assert_eq!(image.image, PathBuf::from("shot.png"));
}

#[test]
fn legacy_image_args_require_image_and_question() {
    let cli = Cli::parse_from(["visual-rubric", "--question", "Is it readable?"]);
    let err = ImageArgs::try_from(cli.image).unwrap_err();
    assert!(err.to_string().contains("--image is required"));

    let cli = Cli::parse_from(["visual-rubric", "--image", "shot.png"]);
    let err = ImageArgs::try_from(cli.image).unwrap_err();
    assert!(
        err.to_string()
            .contains("--question or --preset is required")
    );
}

#[test]
fn parses_preset_flag_on_image_command() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "image",
        "--image",
        "shot.png",
        "--preset",
        "ux-consistency",
    ]);
    let Some(Commands::Image(image)) = cli.command else {
        panic!("expected image command");
    };
    assert_eq!(image.questions.preset.as_deref(), Some("ux-consistency"));
    assert!(image.questions.question.is_none());
}

#[test]
fn parses_legacy_preset_flag() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "--image",
        "shot.png",
        "--preset",
        "accessibility",
    ]);
    assert!(cli.command.is_none());
    let image: ImageArgs = cli.image.try_into().unwrap();
    assert_eq!(image.questions.preset.as_deref(), Some("accessibility"));
    assert!(image.questions.question.is_none());
}

#[test]
fn registered_preset_resolves_question_and_system_prompt() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "image",
        "--image",
        "shot.png",
        "--preset",
        "manuscript-figure",
    ]);
    let Some(Commands::Image(image)) = cli.command else {
        panic!("expected image command");
    };
    assert_eq!(
        image.questions.resolve().unwrap(),
        crate::presets::MANUSCRIPT_FIGURE_QUESTION
    );
    assert_eq!(
        image.questions.resolve_system_prompt().unwrap().as_deref(),
        Some(crate::presets::MANUSCRIPT_FIGURE_SYSTEM_PROMPT)
    );
}

#[test]
fn unknown_preset_resolution_lists_available_presets() {
    let source = QuestionSource {
        question: None,
        preset: Some("ux-consistency".into()),
    };
    let err = source.resolve().unwrap_err();
    assert!(err.to_string().contains("website-install"));
}

#[test]
fn parses_audit_viewports() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        "public",
        "--question",
        "Is it usable?",
        "--viewport",
        "wide=1440x900",
    ]);
    let Some(Commands::Audit(audit)) = cli.command else {
        panic!("expected audit command");
    };
    assert_eq!(audit.viewports[0].name, "wide");
    assert_eq!(audit.viewports[0].width, 1440);
}

#[test]
fn rejects_invalid_audit_viewports() {
    for viewport in ["=1440x900", "../wide=1440x900", "wide=0x900", "wide=1440x0"] {
        let err = Cli::try_parse_from([
            "visual-rubric",
            "audit",
            "--root",
            "public",
            "--question",
            "Is it usable?",
            "--viewport",
            viewport,
        ])
        .expect_err("invalid viewport should fail");

        assert!(
            err.to_string().contains("viewport"),
            "error should name viewport for {viewport:?}: {err}"
        );
    }
}

#[cfg(unix)]
#[test]
fn audit_hosts_static_site_and_writes_report_with_fake_browser() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
    let browser = temp.path().join("fake-browser");
    write_fake_browser(&browser);
    let report = temp.path().join("report.json");
    let screenshots = temp.path().join("shots");

    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        public.to_str().unwrap(),
        "--question",
        "Does it render?",
        "--browser",
        browser.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--screenshots",
        screenshots.to_str().unwrap(),
        "--fake-pass",
        "--viewport",
        "tiny=320x240",
    ]);

    run(cli).unwrap();
    assert!(screenshots.join("tiny.png").exists());
    let report: AuditReport =
        serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.aggregate_status, AuditStatus::Pass);
    assert_eq!(report.screenshots[0].name, "tiny");
    assert!(matches!(
        report.screenshots[0].rubric,
        super::RubricReport::Pass { .. }
    ));
}

#[cfg(unix)]
#[test]
fn audit_skip_ai_uses_default_viewports_and_report_contract() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
    let browser = temp.path().join("fake-browser");
    write_fake_browser(&browser);
    let report = temp.path().join("report.json");
    let screenshots = temp.path().join("shots");

    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        public.to_str().unwrap(),
        "--question",
        "Does it render?",
        "--browser",
        browser.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--screenshots",
        screenshots.to_str().unwrap(),
        "--skip-ai",
    ]);

    run(cli).unwrap();
    let report: AuditReport =
        serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.aggregate_status, AuditStatus::Skipped);
    assert_eq!(report.screenshots.len(), 2);
    assert_eq!(report.screenshots[0].name, "desktop");
    assert_eq!(report.screenshots[0].width, 1440);
    assert_eq!(report.screenshots[1].name, "mobile");
    assert_eq!(report.screenshots[1].width, 390);
    assert!(matches!(
        report.screenshots[0].rubric,
        super::RubricReport::Skipped { .. }
    ));
}

#[cfg(unix)]
#[test]
fn audit_preserves_multiple_viewport_order_and_custom_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(public.join("__audit")).unwrap();
    std::fs::write(public.join("__audit/install.html"), "<h1>Install</h1>").unwrap();
    let browser = temp.path().join("fake-browser");
    write_fake_browser(&browser);
    let report = temp.path().join("report.json");

    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        public.to_str().unwrap(),
        "--path",
        "__audit/install.html",
        "--question",
        "Does it render?",
        "--browser",
        browser.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--skip-ai",
        "--viewport",
        "wide=1200x800",
        "--viewport",
        "narrow=320x700",
    ]);

    run(cli).unwrap();
    let report: AuditReport =
        serde_json::from_str(&std::fs::read_to_string(report).unwrap()).unwrap();
    assert!(report.url.ends_with("/__audit/install.html"));
    assert_eq!(report.screenshots[0].name, "wide");
    assert_eq!(report.screenshots[1].name, "narrow");
}

#[cfg(unix)]
#[test]
fn audit_errors_when_browser_fails_or_writes_no_screenshot() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
    let failing_browser = temp.path().join("failing-browser");
    write_fake_browser_script(&failing_browser, "#!/usr/bin/env bash\nexit 7\n");
    let report = temp.path().join("report.json");

    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        public.to_str().unwrap(),
        "--question",
        "Does it render?",
        "--browser",
        failing_browser.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--skip-ai",
        "--viewport",
        "tiny=320x240",
    ]);
    let err = run(cli).unwrap_err();
    assert!(err.to_string().contains("browser"));

    let silent_browser = temp.path().join("silent-browser");
    write_fake_browser_script(&silent_browser, "#!/usr/bin/env bash\nexit 0\n");
    let cli = Cli::parse_from([
        "visual-rubric",
        "audit",
        "--root",
        public.to_str().unwrap(),
        "--question",
        "Does it render?",
        "--browser",
        silent_browser.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--skip-ai",
        "--viewport",
        "tiny=320x240",
    ]);
    let err = run(cli).unwrap_err();
    assert!(err.to_string().contains("did not write"));
}

#[test]
fn static_path_resolution_and_content_types_are_strict() {
    let root = PathBuf::from("/tmp/site");
    assert_eq!(
        super::resolve_static_path(&root, "/"),
        root.join("index.html")
    );
    assert_eq!(
        super::resolve_static_path(&root, "/docs/?v=1"),
        root.join("docs").join("index.html")
    );
    assert_eq!(
        super::resolve_static_path(&root, "/assets%2Fapp.js"),
        root.join("assets").join("app.js")
    );
    assert_eq!(
        super::resolve_static_path(&root, "/%2e%2e/secret.txt"),
        root.join("__invalid__")
    );
    assert_eq!(
        super::resolve_static_path(&root, "/bad%zz"),
        root.join("__invalid__")
    );
    assert_eq!(
        super::content_type(&root.join("app.js")),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        super::content_type(&root.join("data.json")),
        "application/json; charset=utf-8"
    );
}

#[cfg(unix)]
fn write_fake_browser(path: &std::path::Path) {
    write_fake_browser_script(
        path,
        r#"
set -eu
out=
for arg
do
    case "$arg" in
        --screenshot=*) out="${arg#--screenshot=}" ;;
    esac
done
test -n "$out"
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=' | base64 -d > "$out"
"#,
    );
}

#[cfg(unix)]
fn write_fake_browser_script(path: &std::path::Path, script: impl AsRef<[u8]>) {
    let mut file = std::fs::File::create(path).unwrap();
    writeln!(file, "#!{}", test_shell()).unwrap();
    file.write_all(script.as_ref()).unwrap();
    drop(file);
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn test_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
