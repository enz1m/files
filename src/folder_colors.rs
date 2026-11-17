// Colour tags bind to device/inode/birth-time; without birth time, the recorded path must also match.

use std::path::{Path, PathBuf};

use enzui_theme::WorkspaceAccentKey;

pub(crate) const MAX_FOLDER_COLORS: usize = 512;

// Which folder a tag belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FolderIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    // Birth time as (seconds, nanoseconds) since the epoch, when known.
    pub(crate) birth: Option<(i64, u32)>,
}

impl FolderIdentity {
    // The identity of the entry at `path` itself (a symlink is not
    // followed: a tag on a link belongs to the link).
    pub(crate) fn of(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path)?;
        let birth = meta
            .created()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|since| Some((i64::try_from(since.as_secs()).ok()?, since.subsec_nanos())));
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            birth,
        })
    }

    // Whether `other` is the same folder as `self`, given the paths each was
    // seen at.
    fn same_folder(&self, path: &Path, other: &Self, other_path: &Path) -> bool {
        if self.dev != other.dev || self.ino != other.ino {
            return false;
        }
        match (self.birth, other.birth) {
            (Some(a), Some(b)) => a == b,
            _ => path == other_path,
        }
    }
}

// One folder's tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderColor {
    pub(crate) path: PathBuf,
    pub(crate) identity: FolderIdentity,
    pub(crate) color: WorkspaceAccentKey,
}

// Every folder tag, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FolderColors {
    records: Vec<FolderColor>,
}

impl FolderColors {
    pub(crate) fn from_records(mut records: Vec<FolderColor>) -> Self {
        let excess = records.len().saturating_sub(MAX_FOLDER_COLORS);
        records.drain(..excess);
        Self { records }
    }

    pub(crate) fn records(&self) -> &[FolderColor] {
        &self.records
    }

    // The tag of the folder at `path` with `identity`.
    pub(crate) fn color_for(
        &self,
        path: &Path,
        identity: &FolderIdentity,
    ) -> Option<WorkspaceAccentKey> {
        self.records
            .iter()
            .find(|record| record.identity.same_folder(&record.path, identity, path))
            .map(|record| record.color)
    }

    // Tag the folder at `path` with `color`, or clear it with `None`.
    // Returns whether anything changed.
    pub(crate) fn set(
        &mut self,
        path: &Path,
        identity: FolderIdentity,
        color: Option<WorkspaceAccentKey>,
    ) -> bool {
        let before = self.records.clone();
        // This folder's old record, and any stale one left at its path by a
        // folder that no longer lives there.
        self.records.retain(|record| {
            !record.identity.same_folder(&record.path, &identity, path) && record.path != path
        });
        if let Some(color) = color {
            self.records.push(FolderColor {
                path: path.to_path_buf(),
                identity,
                color,
            });
            let excess = self.records.len().saturating_sub(MAX_FOLDER_COLORS);
            self.records.drain(..excess);
        }
        self.records != before
    }

    // Fold in folders just listed: follow a tagged folder to its new path,
    // and drop a record whose path now holds a different folder. Returns
    // whether anything changed.
    pub(crate) fn reconcile(&mut self, listed: &[(PathBuf, FolderIdentity)]) -> bool {
        let mut changed = false;
        for (path, identity) in listed {
            let before = self.records.len();
            self.records.retain(|record| {
                record.path != *path || record.identity.same_folder(&record.path, identity, path)
            });
            changed |= self.records.len() != before;
            for record in &mut self.records {
                if record.path != *path && record.identity.same_folder(&record.path, identity, path)
                {
                    record.path.clone_from(path);
                    changed = true;
                }
            }
        }
        changed
    }
}

// Stat the listed folders off the UI thread.
pub(crate) fn identify(
    paths: Vec<PathBuf>,
) -> impl std::future::Future<Output = Vec<(PathBuf, FolderIdentity)>> + Send {
    crate::shell_work::run(move || {
        paths
            .into_iter()
            .filter_map(|path| FolderIdentity::of(&path).ok().map(|id| (path, id)))
            .collect()
    })
}
