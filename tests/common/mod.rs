use std::path::PathBuf;

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
