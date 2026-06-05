use std::path::{Path, PathBuf};

use crate::PoolError;

pub(super) fn seed_codex_home(
    worker_home: &Path,
    configured_source_home: Option<&Path>,
) -> Result<(), PoolError> {
    let Some(source_home) = source_codex_home(configured_source_home) else {
        return Ok(());
    };
    for file_name in [
        "auth.json",
        "config.toml",
        "installation_id",
        "models_cache.json",
        "version.json",
    ] {
        let source = source_home.join(file_name);
        if source.exists() {
            let target = worker_home.join(file_name);
            std::fs::copy(&source, &target).map_err(|e| {
                PoolError::Spawn(format!(
                    "seed worker CODEX_HOME file {} from {}: {e}",
                    target.display(),
                    source.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn source_codex_home(configured_source_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured_source_home {
        return Some(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(path));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex"))
}
