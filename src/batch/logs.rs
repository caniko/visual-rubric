use std::fs;
use std::path::{Component, Path};

use crate::{LogCaptureConfig, LogPathMode};

pub(super) fn copy_logs_from_config(
    config: Option<&LogCaptureConfig>,
) -> std::io::Result<Vec<String>> {
    let Some(config) = config else {
        return Ok(Vec::new());
    };
    let mut copied = Vec::new();
    copy_selected_logs(
        &config.temp_dir,
        &config.temp_dir,
        &config.output_dir,
        &config.path_mode,
        &mut copied,
    )?;
    copied.sort();
    Ok(copied)
}

fn copy_selected_logs(
    source_root: &Path,
    current_source: &Path,
    destination_root: &Path,
    path_mode: &LogPathMode,
    copied: &mut Vec<String>,
) -> std::io::Result<()> {
    if !current_source.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(current_source)? {
        let entry = entry?;
        let source = entry.path();
        if source.is_dir() {
            copy_selected_logs(source_root, &source, destination_root, path_mode, copied)?;
        } else if should_copy_acp_log(&source) {
            let relative = source.strip_prefix(source_root).unwrap_or(source.as_path());
            let destination = destination_root.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &destination)?;
            copied.push(report_path(path_mode, &destination));
        }
    }
    Ok(())
}

fn should_copy_acp_log(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    file_name.starts_with("logs_")
        || file_name.ends_with(".jsonl") && path.components().any(is_sessions_component)
}

fn is_sessions_component(component: Component<'_>) -> bool {
    matches!(component, Component::Normal(value) if value == "sessions")
}

fn report_path(path_mode: &LogPathMode, destination: &Path) -> String {
    match path_mode {
        LogPathMode::RelativeTo(root) => destination
            .strip_prefix(root)
            .unwrap_or(destination)
            .to_string_lossy()
            .into_owned(),
        LogPathMode::Absolute => destination.to_string_lossy().into_owned(),
    }
}
