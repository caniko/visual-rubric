use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Content-hash snapshot for a caller-provided asset set.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AssetSnapshot {
    hashes: BTreeMap<PathBuf, String>,
}

impl AssetSnapshot {
    /// Captures hashes for the supplied asset paths.
    ///
    /// # Errors
    ///
    /// Returns an IO error when any asset cannot be read.
    pub fn capture<I, P, F>(paths: I, hasher: F) -> std::io::Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
        F: Fn(&[u8]) -> String,
    {
        let mut hashes = BTreeMap::new();
        for path in paths {
            let path = path.as_ref();
            let bytes = fs::read(path)?;
            hashes.insert(path.to_path_buf(), hasher(&bytes));
        }
        Ok(Self { hashes })
    }

    /// Builds a snapshot directly from path/hash pairs.
    #[must_use]
    pub fn from_hashes<I, P, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (P, S)>,
        P: Into<PathBuf>,
        S: Into<String>,
    {
        Self {
            hashes: entries
                .into_iter()
                .map(|(path, hash)| (path.into(), hash.into()))
                .collect(),
        }
    }
}

/// Content change for one asset between two snapshots.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "change", content = "path", rename_all = "snake_case")]
pub enum AssetChange {
    /// Asset exists only in the after snapshot.
    Added(PathBuf),
    /// Asset exists in both snapshots but has a different content hash.
    Changed(PathBuf),
    /// Asset exists only in the before snapshot.
    Deleted(PathBuf),
    /// Asset exists in both snapshots with the same content hash.
    Unchanged(PathBuf),
}

impl AssetChange {
    /// Returns the changed asset path.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Added(path)
            | Self::Changed(path)
            | Self::Deleted(path)
            | Self::Unchanged(path) => path,
        }
    }

    pub(super) fn status(&self) -> &'static str {
        match self {
            Self::Added(_) => "added",
            Self::Changed(_) => "changed",
            Self::Deleted(_) => "deleted",
            Self::Unchanged(_) => "unchanged",
        }
    }
}

/// Compares two asset snapshots.
#[must_use]
pub fn diff_snapshots(before: &AssetSnapshot, after: &AssetSnapshot) -> Vec<AssetChange> {
    let keys: BTreeSet<_> = before.hashes.keys().chain(after.hashes.keys()).collect();
    keys.into_iter()
        .map(
            |path| match (before.hashes.get(path), after.hashes.get(path)) {
                (None, Some(_)) => AssetChange::Added(path.clone()),
                (Some(_), None) => AssetChange::Deleted(path.clone()),
                (Some(old), Some(new)) if old == new => AssetChange::Unchanged(path.clone()),
                (Some(_), Some(_)) => AssetChange::Changed(path.clone()),
                (None, None) => unreachable!("path came from snapshot keys"),
            },
        )
        .collect()
}

/// Asset selection mode for a batch rubric run.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    /// Evaluate only added or changed assets.
    #[default]
    ChangedOnly,
    /// Evaluate added, changed, and unchanged assets.
    IncludeUnchanged,
}

/// Selects the assets that should be evaluated from a snapshot diff.
#[must_use]
pub fn select_changed(changes: &[AssetChange], mode: SelectionMode) -> Vec<PathBuf> {
    changes
        .iter()
        .filter_map(|change| match (change, mode) {
            (AssetChange::Added(path) | AssetChange::Changed(path), _) => Some(path.clone()),
            (AssetChange::Unchanged(path), SelectionMode::IncludeUnchanged) => Some(path.clone()),
            (AssetChange::Deleted(_) | AssetChange::Unchanged(_), SelectionMode::ChangedOnly) => {
                None
            }
            (AssetChange::Deleted(_), SelectionMode::IncludeUnchanged) => None,
        })
        .collect()
}
