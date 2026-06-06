#![allow(dead_code)]

use std::path::PathBuf;

pub fn fake_codex_acp_binary() -> Option<PathBuf> {
    if !cfg!(feature = "fake-codex-acp") {
        return None;
    }
    let path = option_env!("CARGO_BIN_EXE_fake-codex-acp").map(PathBuf::from)?;
    path.exists().then_some(path)
}

pub fn write_fixture_png(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("fixture.png");
    std::fs::write(path.as_path(), fixture_png()).expect("write fixture png");
    path
}

fn fixture_png() -> &'static [u8] {
    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4,
        0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31, 0, 3, 3,
        2, 0, 239, 191, 167, 219, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    PNG
}

#[cfg(unix)]
pub fn write_fake_browser(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let mut file = std::fs::File::create(path).expect("create fake browser");
    writeln!(file, "#!{}", test_shell()).expect("write fake browser shebang");
    file.write_all(body.as_bytes()).expect("write fake browser");
    drop(file);
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
pub fn fake_browser_success_script() -> &'static str {
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
"#
}

#[cfg(unix)]
fn test_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}
