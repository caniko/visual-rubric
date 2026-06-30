use std::io::{Read as _, Write as _};
#[cfg(all(unix, feature = "audit"))]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(all(unix, feature = "audit"))]
use std::sync::{Mutex, MutexGuard, OnceLock};

use clap::Parser as _;

#[cfg(feature = "codex-acp")]
use super::ImageArgs;
#[cfg(feature = "audit")]
use super::run;
#[cfg(feature = "audit")]
use super::{AuditReport, AuditStatus};
use super::{Cli, Commands, PathBuf, QuestionSource};

#[cfg(all(unix, feature = "audit"))]
fn audit_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("audit test lock poisoned")
}

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
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

#[cfg(feature = "codex-acp")]
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
fn parses_configured_mode_override() {
    let cli = Cli::parse_from([
        "visual-rubric",
        "configured",
        "--image",
        "shot.png",
        "--question",
        "Is it readable?",
        "--mode",
        "pipeline",
    ]);
    let Some(Commands::Configured(args)) = cli.command else {
        panic!("expected configured command");
    };
    assert_eq!(args.mode, Some(crate::ConfigMode::Pipeline));
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

#[cfg(feature = "audit")]
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

#[cfg(feature = "audit")]
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

#[cfg(all(unix, feature = "audit"))]
#[test]
fn audit_hosts_static_site_and_writes_report_with_fake_browser() {
    let _guard = audit_test_lock();
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

#[cfg(all(unix, feature = "audit"))]
#[test]
fn audit_skip_ai_uses_default_viewports_and_report_contract() {
    let _guard = audit_test_lock();
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

#[cfg(all(unix, feature = "audit"))]
#[test]
fn audit_preserves_multiple_viewport_order_and_custom_path() {
    let _guard = audit_test_lock();
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

#[cfg(all(unix, feature = "audit"))]
#[test]
fn audit_errors_when_browser_fails_or_writes_no_screenshot() {
    let _guard = audit_test_lock();
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

#[test]
fn static_server_serves_get_head_and_decoded_paths() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(public.join("assets")).unwrap();
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
    std::fs::write(public.join("style.css"), "body{color:#111}").unwrap();
    std::fs::write(public.join("assets").join("app.js"), "console.log('ok');").unwrap();
    let server = super::StaticServer::start(public, 0).unwrap();

    let response = http_request(&server, "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK"));
    assert!(headers.contains("Content-Type: text/html; charset=utf-8"));
    assert!(headers.contains("Content-Length: 16"));
    assert_eq!(body, "<h1>Install</h1>");

    let response = http_request(
        &server,
        "HEAD /style.css HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK"));
    assert!(headers.contains("Content-Type: text/css; charset=utf-8"));
    assert!(headers.contains("Content-Length: 16"));
    assert_eq!(body, "");

    let response = http_request(
        &server,
        "GET /assets%2Fapp.js?cache=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK"));
    assert!(headers.contains("Content-Type: text/javascript; charset=utf-8"));
    assert!(headers.contains("Content-Length: 18"));
    assert_eq!(body, "console.log('ok');");
}

#[test]
fn static_server_returns_errors_for_missing_method_and_traversal() {
    let temp = tempfile::TempDir::new().unwrap();
    let public = temp.path().join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("index.html"), "<h1>Install</h1>").unwrap();
    let server = super::StaticServer::start(public, 0).unwrap();

    let response = http_request(
        &server,
        "GET /missing.txt HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 404 Not Found"));
    assert!(headers.contains("Content-Type: text/plain; charset=utf-8"));
    assert!(headers.contains("Content-Length: 9"));
    assert_eq!(body, "not found");

    let response = http_request(&server, "POST / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 405 Method Not Allowed"));
    assert!(headers.contains("Content-Type: text/plain"));
    assert!(headers.contains("Content-Length: 18"));
    assert_eq!(body, "method not allowed");

    let response = http_request(
        &server,
        "GET /%2e%2e/secret.txt HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    let (headers, body) = split_response(&response);
    assert!(headers.starts_with("HTTP/1.1 404 Not Found"));
    assert_eq!(body, "not found");
}

fn http_request(server: &super::StaticServer, request: &str) -> String {
    let port = server
        .base_url()
        .strip_prefix("http://127.0.0.1:")
        .and_then(|url| url.strip_suffix('/'))
        .expect("static server base URL should include localhost port")
        .parse::<u16>()
        .expect("static server port should parse");
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn split_response(response: &str) -> (&str, &str) {
    response
        .split_once("\r\n\r\n")
        .expect("HTTP response should contain header terminator")
}

#[cfg(all(unix, feature = "audit"))]
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

#[cfg(all(unix, feature = "audit"))]
fn write_fake_browser_script(path: &std::path::Path, script: impl AsRef<[u8]>) {
    let mut file = std::fs::File::create(path).unwrap();
    writeln!(file, "#!{}", test_shell()).unwrap();
    file.write_all(script.as_ref()).unwrap();
    drop(file);
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(all(unix, feature = "audit"))]
fn test_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
