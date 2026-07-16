//! Versioned capture manifests emitted by project-owned visual producers.

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Current capture-manifest schema version.
pub const CAPTURE_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// A producer's complete description of one visual capture run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CaptureManifest {
    /// Manifest schema version.
    pub schema_version: u32,
    /// Stable visual target identifier.
    pub target: String,
    /// Source revision used to produce the captures.
    pub revision: String,
    /// Runtime and capture provenance.
    pub environment: CaptureEnvironment,
    /// Number of matrix cells the producer claims to have emitted.
    pub declared_cells: usize,
    /// Captures emitted by the producer.
    pub captures: Vec<CaptureCell>,
}

/// Runtime provenance for a capture run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CaptureEnvironment {
    /// Operating-system or platform identifier.
    pub platform: String,
    /// Renderer/backend identifier.
    pub renderer: String,
    /// Capture implementation identifier.
    pub capture_backend: String,
}

/// One required visual matrix cell and its evidence paths.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CaptureCell {
    /// Stable matrix-cell identifier.
    pub id: String,
    /// Relative path to the primary PNG artifact.
    pub image: PathBuf,
    /// Project-owned state or scenario identifier.
    pub state: String,
    /// Capture dimensions and device scale factor.
    pub viewport: Viewport,
    /// Visual theme identifier.
    pub theme: String,
    /// Locale identifier.
    pub locale: String,
    /// Additional producer metadata paths, such as DOM or accessibility data.
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, PathBuf>,
    /// Rubric presets requested for this cell.
    #[serde(default)]
    pub presets: Vec<String>,
}

/// Viewport dimensions and device scale factor.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Viewport {
    /// CSS or logical width in pixels.
    pub width: u32,
    /// CSS or logical height in pixels.
    pub height: u32,
    /// Device scale factor.
    pub dpr: f32,
}

/// Errors raised while loading or validating a capture manifest.
#[derive(Debug)]
pub enum ManifestError {
    /// The manifest could not be read.
    Read {
        /// Manifest path.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// The manifest was not valid JSON or did not match the schema.
    Parse {
        /// Manifest path.
        path: PathBuf,
        /// Underlying JSON error.
        source: serde_json::Error,
    },
    /// The manifest or its artifacts violated the capture contract.
    Invalid {
        /// Human-readable validation failures.
        issues: Vec<String>,
    },
}

impl Display for ManifestError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "read capture manifest {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(
                    formatter,
                    "parse capture manifest {}: {source}",
                    path.display()
                )
            }
            Self::Invalid { issues } => {
                write!(formatter, "capture manifest validation failed")?;
                for issue in issues {
                    write!(formatter, "; {issue}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Invalid { .. } => None,
        }
    }
}

impl CaptureManifest {
    /// Loads and validates a JSON manifest and all referenced artifacts.
    ///
    /// `root` is the directory against which every relative image and metadata
    /// path is resolved. Absolute paths and paths containing `..` are rejected
    /// so a producer cannot make a report depend on an unrelated checkout.
    pub fn load(path: &Path, root: &Path) -> Result<Self, ManifestError> {
        let raw = fs::read_to_string(path).map_err(|source| ManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let manifest: Self = serde_json::from_str(&raw).map_err(|source| ManifestError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        manifest.validate(root)?;
        Ok(manifest)
    }

    /// Validates schema invariants and all referenced artifact paths.
    pub fn validate(&self, root: &Path) -> Result<(), ManifestError> {
        let mut issues = Vec::new();
        if self.schema_version != CAPTURE_MANIFEST_SCHEMA_VERSION {
            issues.push(format!(
                "unsupported schema_version {}; expected {}",
                self.schema_version, CAPTURE_MANIFEST_SCHEMA_VERSION
            ));
        }
        if self.target.trim().is_empty() {
            issues.push("target must not be empty".to_owned());
        }
        if self.revision.trim().is_empty() {
            issues.push("revision must not be empty".to_owned());
        }
        validate_environment(&self.environment, &mut issues);
        if self.declared_cells != self.captures.len() {
            issues.push(format!(
                "declared_cells={} but captures contains {} entries",
                self.declared_cells,
                self.captures.len()
            ));
        }

        let mut ids = std::collections::BTreeSet::new();
        for capture in &self.captures {
            validate_capture(capture, root, &mut ids, &mut issues);
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ManifestError::Invalid { issues })
        }
    }
}

fn validate_environment(environment: &CaptureEnvironment, issues: &mut Vec<String>) {
    for (name, value) in [
        ("environment.platform", &environment.platform),
        ("environment.renderer", &environment.renderer),
        ("environment.capture_backend", &environment.capture_backend),
    ] {
        if value.trim().is_empty() {
            issues.push(format!("{name} must not be empty"));
        }
    }
}

fn validate_capture(
    capture: &CaptureCell,
    root: &Path,
    ids: &mut std::collections::BTreeSet<String>,
    issues: &mut Vec<String>,
) {
    if capture.id.trim().is_empty() {
        issues.push("capture id must not be empty".to_owned());
    } else if !ids.insert(capture.id.clone()) {
        issues.push(format!("duplicate capture id {:?}", capture.id));
    }
    if capture.state.trim().is_empty() {
        issues.push(format!("capture {:?} has empty state", capture.id));
    }
    if capture.theme.trim().is_empty() {
        issues.push(format!("capture {:?} has empty theme", capture.id));
    }
    if capture.locale.trim().is_empty() {
        issues.push(format!("capture {:?} has empty locale", capture.id));
    }
    if capture.viewport.width == 0 || capture.viewport.height == 0 {
        issues.push(format!(
            "capture {:?} has zero viewport dimensions",
            capture.id
        ));
    }
    if !capture.viewport.dpr.is_finite() || capture.viewport.dpr <= 0.0 {
        issues.push(format!(
            "capture {:?} has invalid device scale factor {}",
            capture.id, capture.viewport.dpr
        ));
    }

    validate_artifact(&capture.id, "image", &capture.image, root, issues);
    for (kind, path) in &capture.metadata {
        validate_artifact(&capture.id, kind, path, root, issues);
    }
}

fn validate_artifact(
    capture_id: &str,
    kind: &str,
    path: &Path,
    root: &Path,
    issues: &mut Vec<String>,
) {
    if path.as_os_str().is_empty() {
        issues.push(format!("capture {capture_id:?} has empty {kind} path"));
        return;
    }
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        issues.push(format!(
            "capture {capture_id:?} {kind} path {} must be relative and contained by the manifest root",
            path.display()
        ));
        return;
    }
    let resolved = root.join(path);
    match fs::metadata(&resolved) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {}
        Ok(metadata) if metadata.is_dir() => issues.push(format!(
            "capture {capture_id:?} {kind} artifact {} is a directory",
            resolved.display()
        )),
        Ok(_) => issues.push(format!(
            "capture {capture_id:?} {kind} artifact {} is empty",
            resolved.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => issues.push(format!(
            "capture {capture_id:?} {kind} artifact {} is missing",
            resolved.display()
        )),
        Err(error) => issues.push(format!(
            "capture {capture_id:?} {kind} artifact {} is unreadable: {error}",
            resolved.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn manifest(image: &str) -> CaptureManifest {
        CaptureManifest {
            schema_version: CAPTURE_MANIFEST_SCHEMA_VERSION,
            target: "demo/ui".to_owned(),
            revision: "deadbeef".to_owned(),
            environment: CaptureEnvironment {
                platform: "linux".to_owned(),
                renderer: "test".to_owned(),
                capture_backend: "fixture".to_owned(),
            },
            declared_cells: 1,
            captures: vec![CaptureCell {
                id: "menu/desktop/light/en-US".to_owned(),
                image: PathBuf::from(image),
                state: "menu".to_owned(),
                viewport: Viewport {
                    width: 1280,
                    height: 720,
                    dpr: 1.0,
                },
                theme: "light".to_owned(),
                locale: "en-US".to_owned(),
                metadata: BTreeMap::new(),
                presets: vec!["ui-regression".to_owned()],
            }],
        }
    }

    #[test]
    fn validates_manifest_and_metadata_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("menu.png"), [1, 2, 3]).expect("image");
        let mut value = manifest("menu.png");
        value
            .captures
            .first_mut()
            .expect("capture")
            .metadata
            .insert("a11y".to_owned(), PathBuf::from("a11y.json"));
        fs::write(temp.path().join("a11y.json"), "{}").expect("metadata");

        assert!(value.validate(temp.path()).is_ok());
    }

    #[test]
    fn rejects_duplicate_cells_and_missing_artifacts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut value = manifest("missing.png");
        value.captures.push(value.captures[0].clone());
        value.declared_cells = 2;

        let error = value.validate(temp.path()).expect_err("invalid manifest");
        let ManifestError::Invalid { issues } = error else {
            panic!("expected validation errors");
        };
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("duplicate capture id"))
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("artifact") && issue.contains("missing"))
        );
    }

    #[test]
    fn rejects_paths_that_escape_manifest_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let value = manifest("../outside.png");
        let error = value.validate(temp.path()).expect_err("escaping path");
        let ManifestError::Invalid { issues } = error else {
            panic!("expected validation errors");
        };
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("must be relative"))
        );
    }
}
