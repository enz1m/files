// Trash and thumbnail authority binds verified descriptors; unavailable roots have no sentinel path.

use enzui_security::secure_dir::{SecureDirRefusal, SecureRoot};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Freedesktop shared Trash directory name under `$XDG_DATA_HOME`.
pub(crate) const HOME_TRASH_DIR_NAME: &str = "Trash";

// Freedesktop shared thumbnail directory name under `$XDG_CACHE_HOME`.
pub(crate) const THUMBNAIL_DIR_NAME: &str = "thumbnails";

// Roots captured at boot for the two freedesktop directories Files uses;
// injectable so tests need not mutate the process environment.
#[derive(Clone, Debug)]
pub(crate) struct AppPrivateRoots {
    data_home: Option<PathBuf>,
    cache_home: Option<PathBuf>,
}

impl AppPrivateRoots {
    // Snapshot `$XDG_DATA_HOME` (fallback `$HOME/.local/share`) and
    // `$XDG_CACHE_HOME` (fallback `$HOME/.cache`). Relative roots are
    // discarded per the XDG spec.
    #[must_use]
    pub(crate) fn from_env() -> Self {
        let home = non_empty_absolute("HOME");
        let data_home = non_empty_absolute("XDG_DATA_HOME")
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let cache_home = non_empty_absolute("XDG_CACHE_HOME")
            .or_else(|| home.as_ref().map(|h| h.join(".cache")));
        Self {
            data_home: data_home.filter(|p| p.is_absolute()),
            cache_home: cache_home.filter(|p| p.is_absolute()),
        }
    }

    // Open the home Trash root under a descriptor-bound [`SecureRoot`].
    // `revalidate()` fails closed on a pathname-inode substitution.
    pub(crate) fn resolve_home_trash(&self) -> HomeTrashAuthority {
        let Some(data_home) = self.data_home.as_ref() else {
            return HomeTrashAuthority::Unavailable(HomeTrashUnavailable::NoAuthority);
        };
        let trash_path = data_home.join(HOME_TRASH_DIR_NAME);
        match SecureRoot::open(&trash_path) {
            Ok(root) => HomeTrashAuthority::Available(Arc::new(root)),
            Err(_) => HomeTrashAuthority::Unavailable(HomeTrashUnavailable::RootRefused),
        }
    }

    // Open the freedesktop shared thumbnails root under a
    // descriptor-bound [`SecureRoot`]. Durable writes must gate on the
    // returned authority; in-memory thumbnails remain available.
    pub(crate) fn resolve_thumbnail_cache(&self) -> ThumbnailCacheAuthority {
        let Some(cache_home) = self.cache_home.as_ref() else {
            return ThumbnailCacheAuthority::Unavailable;
        };
        let thumbs_path = cache_home.join(THUMBNAIL_DIR_NAME);
        match SecureRoot::open(&thumbs_path) {
            Ok(root) => ThumbnailCacheAuthority::Available(Arc::new(root)),
            Err(_) => ThumbnailCacheAuthority::Unavailable,
        }
    }
}

fn non_empty_absolute(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() { Some(path) } else { None }
}

// Descriptor-bound home-Trash authority; Trash tasks receive the handle,
// not the pathname, and revalidate before touching disk.
#[derive(Clone, Debug)]
pub(crate) enum HomeTrashAuthority {
    Available(Arc<SecureRoot>),
    Unavailable(HomeTrashUnavailable),
}

impl HomeTrashAuthority {
    // The verified Trash root, if the authority is still available. Callers
    // must call [`SecureRoot::revalidate`] before dispatching a filesystem
    // task and transition the authority to `Unavailable` on refusal.
    #[must_use]
    pub(crate) fn root(&self) -> Option<&Arc<SecureRoot>> {
        match self {
            Self::Available(root) => Some(root),
            Self::Unavailable(_) => None,
        }
    }

    // Parent directory of the verified Trash root, for pathname-based APIs;
    // `None` when unavailable.
    #[must_use]
    pub(crate) fn data_home(&self) -> Option<PathBuf> {
        self.root()
            .and_then(|r| r.path().parent().map(Path::to_path_buf))
    }
}

// The Trash root and its `files/` and `info/` children as retained
// `SecureRoot` fds; every operation is an `*at` call relative to one of them.
#[derive(Clone, Debug)]
pub(crate) struct TrashRootHandles {
    pub(crate) trash: Arc<SecureRoot>,
    pub(crate) files: Arc<SecureRoot>,
    pub(crate) info: Arc<SecureRoot>,
}

impl TrashRootHandles {
    // Open `<trash>/files` and `<trash>/info` through
    // [`SecureRoot::ensure_subroot`] on the retained parent fd, creating them
    // (mode `0700`) or adopting them with the same private-adoption checks as
    // the parent.
    pub(crate) fn from_trash(trash: Arc<SecureRoot>) -> Result<Self, SecureDirRefusal> {
        let files = Arc::new(
            trash
                .ensure_subroot(std::ffi::OsStr::new("files"))
                .map_err(|_| SecureDirRefusal::CreateFailed)?,
        );
        let info = Arc::new(
            trash
                .ensure_subroot(std::ffi::OsStr::new("info"))
                .map_err(|_| SecureDirRefusal::CreateFailed)?,
        );
        Ok(Self { trash, files, info })
    }
}

// Per-entry / per-source Trash root authority: the process-wide home
// Trash handles.
#[derive(Clone, Debug)]
pub(crate) enum TrashRootAuthority {
    Home(TrashRootHandles),
}

impl TrashRootAuthority {
    #[must_use]
    pub(crate) fn handles(&self) -> &TrashRootHandles {
        match self {
            Self::Home(h) => h,
        }
    }

    #[must_use]
    pub(crate) fn files_root(&self) -> Arc<SecureRoot> {
        std::sync::Arc::clone(&self.handles().files)
    }

    #[must_use]
    pub(crate) fn info_root(&self) -> Arc<SecureRoot> {
        std::sync::Arc::clone(&self.handles().info)
    }

    // Revalidate the retained root fd against its pathname, failing closed on
    // a substituted inode. Operations stay bound to the pinned fd either way.
    pub(crate) fn revalidate(&self) -> Result<(), SecureDirRefusal> {
        self.handles().trash.revalidate()?;
        self.handles().files.revalidate()?;
        self.handles().info.revalidate()
    }
}

// Path-free reason a home-Trash authority is unavailable.
#[derive(Clone, Debug)]
pub(crate) enum HomeTrashUnavailable {
    // No usable absolute `$XDG_DATA_HOME` and no usable absolute `$HOME`.
    NoAuthority,
    // The framework's private-directory capability refused the
    // freedesktop Trash root (symlink, non-directory, wrong owner,
    // group/world writable, or a replacement race).
    RootRefused,
}

impl HomeTrashUnavailable {
    #[must_use]
    pub(crate) fn user_summary(&self) -> &'static str {
        match self {
            Self::NoAuthority => crate::i18n::tr("Home Trash is disabled: no XDG data authority"),
            Self::RootRefused => {
                crate::i18n::tr("Home Trash is disabled: the Trash directory failed validation")
            }
        }
    }
}

// Descriptor-bound durable-thumbnail authority.
#[derive(Clone, Debug)]
pub(crate) enum ThumbnailCacheAuthority {
    Available(Arc<SecureRoot>),
    Unavailable,
}

// Thumbnail-cache roots under `<cache>/thumbnails` as descriptor-bound
// `SecureRoot`s: `normal/` and `fail/<FAILURE_CACHE_NAMESPACE>/`, opened via
// [`SecureRoot::ensure_subroot`]. `thumbnail::load` still writes through
// pathnames rather than these fds.
#[derive(Clone, Debug)]
pub(crate) struct ThumbnailNestedRoots {
    pub(crate) normal: Arc<SecureRoot>,
    pub(crate) failure: Arc<SecureRoot>,
}

impl ThumbnailCacheAuthority {
    #[must_use]
    pub(crate) fn root(&self) -> Option<&Arc<SecureRoot>> {
        match self {
            Self::Available(root) => Some(root),
            Self::Unavailable => None,
        }
    }

    // Open `normal/` and `fail/<namespace>/` under the thumbnails root via
    // [`SecureRoot::ensure_subroot`]. `None` if the parent is unavailable or
    // any child is refused; the cache is never partially available.
    #[must_use]
    pub(crate) fn nested_roots(&self, failure_namespace: &str) -> Option<ThumbnailNestedRoots> {
        let parent = std::sync::Arc::clone(self.root()?);
        let normal = parent
            .ensure_subroot(std::ffi::OsStr::new("normal"))
            .ok()
            .map(Arc::new)?;
        let fail = parent.ensure_subroot(std::ffi::OsStr::new("fail")).ok()?;
        let failure = fail
            .ensure_subroot(std::ffi::OsStr::new(failure_namespace))
            .ok()
            .map(Arc::new)?;
        Some(ThumbnailNestedRoots { normal, failure })
    }
}

// The status a revalidation refusal posts when the durable thumbnail cache
// turns unavailable.
pub(crate) const THUMBNAIL_CACHE_REFUSED: &str =
    "Durable thumbnail cache is disabled: the thumbnails directory failed validation";

#[cfg(test)]
impl AppPrivateRoots {
    #[must_use]
    pub(crate) fn new(data_home: Option<PathBuf>, cache_home: Option<PathBuf>) -> Self {
        Self {
            data_home: data_home.filter(|p| p.is_absolute()),
            cache_home: cache_home.filter(|p| p.is_absolute()),
        }
    }
}

#[cfg(test)]
impl HomeTrashAuthority {}

#[cfg(test)]
impl ThumbnailCacheAuthority {}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_test_support::test_scratch::ScratchDir;
    use std::os::unix::fs::DirBuilderExt;

    #[test]
    fn symlink_at_trash_leaf_is_refused_and_left_in_place() {
        let scratch = ScratchDir::new("private-data-trash-symlink");
        let data = scratch.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let foreign = scratch.path().join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, data.join(HOME_TRASH_DIR_NAME)).unwrap();

        let roots = AppPrivateRoots::new(Some(data.clone()), None);
        let authority = roots.resolve_home_trash();
        assert!(matches!(
            authority,
            HomeTrashAuthority::Unavailable(HomeTrashUnavailable::RootRefused)
        ));
        let md = std::fs::symlink_metadata(data.join(HOME_TRASH_DIR_NAME)).unwrap();
        assert!(md.file_type().is_symlink(), "planted symlink preserved");
        assert!(foreign.is_dir(), "symlink target preserved");
    }

    #[test]
    fn revalidate_reports_root_identity_changed_when_pathname_is_replaced() {
        let scratch = ScratchDir::new("private-data-revalidate");
        let data = scratch.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let roots = AppPrivateRoots::new(Some(data.clone()), None);
        let authority = roots.resolve_home_trash();
        let root = std::sync::Arc::clone(authority.root().expect("Available"));
        assert!(root.revalidate().is_ok());

        let trash_path = data.join(HOME_TRASH_DIR_NAME);
        let displaced = data.join("Trash-displaced");
        std::fs::rename(&trash_path, &displaced).unwrap();
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&trash_path)
            .unwrap();

        let err = root.revalidate().expect_err("substituted inode is refused");
        assert!(matches!(err, SecureDirRefusal::RootIdentityChanged { .. }));
        // Neither directory is touched by the check.
        assert!(trash_path.is_dir());
        assert!(displaced.is_dir());
    }
}
