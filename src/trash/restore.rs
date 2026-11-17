// Restore and metadata twins: restore-without-replacing, publish and
// retire error custody.

use super::{PublishCustody, TrashEntry};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// Typed reason a restore-without-replacing attempt failed.
#[derive(Debug)]
pub(crate) enum RestoreError {
    // The recorded original path equals the current location.
    OriginalLocationUnknown,
    // The original parent is missing or not a directory.
    ParentUnavailable {
        parent: PathBuf,
    },
    // `renameat2(RENAME_NOREPLACE)` returned `EEXIST`.
    DestinationOccupied {
        at: PathBuf,
    },
    // The trash payload path is not representable as a C string.
    InvalidTrashPath,
    // The original path could not be represented as a C string.
    InvalidOriginalPath,
    // Any other underlying OS error from `renameat2`.
    Io(io::Error),
    // The descriptor-bound restore partially completed.
    //
    // Unless `custody` is `Source`, bytes may be at the original path: the caller
    // must treat the restore as unsettled and keep the metadata twin.
    Pending {
        // Which side still holds bytes; from framework `MoveCustody`.
        custody: PublishCustody,
        // Original I/O message from the framework, for logs.
        diagnostic: String,
    },
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OriginalLocationUnknown => write!(f, "the original location is unknown"),
            Self::ParentUnavailable { parent } => write!(
                f,
                "the original folder {} is no longer available",
                parent.display()
            ),
            Self::DestinationOccupied { at } => {
                write!(f, "the original path {} is now occupied", at.display())
            }
            Self::InvalidTrashPath => write!(f, "invalid trash path"),
            Self::InvalidOriginalPath => write!(f, "invalid original path"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Pending {
                custody,
                diagnostic,
            } => write!(
                f,
                "restore did not settle cleanly (custody={custody:?}): {diagnostic}"
            ),
        }
    }
}

// Journal restore retains the metadata twin for rollback and refuses missing parents without recreating them.
pub(crate) fn restore_payload_without_replacing(entry: &TrashEntry) -> Result<(), RestoreError> {
    if entry.original_path == entry.trashed_path {
        return Err(RestoreError::OriginalLocationUnknown);
    }
    let Some(parent) = entry.original_path.parent() else {
        return Err(RestoreError::ParentUnavailable {
            parent: entry.original_path.clone(),
        });
    };
    if !parent.is_dir() {
        return Err(RestoreError::ParentUnavailable {
            parent: parent.to_path_buf(),
        });
    }

    if let Some(authority) = entry.retained_root.as_ref() {
        return restore_payload_descriptor_bound(entry, authority, parent);
    }
    restore_payload_legacy_renameat2(entry)
}

fn restore_payload_descriptor_bound(
    entry: &TrashEntry,
    authority: &crate::private_data::TrashRootAuthority,
    original_parent: &Path,
) -> Result<(), RestoreError> {
    use enzui_security::secure_dir::RetainedDir;

    let files_root = authority.files_root();
    let Some(payload_name) = entry.trashed_path.file_name() else {
        return Err(RestoreError::InvalidTrashPath);
    };
    let Some(original_name) = entry.original_path.file_name() else {
        return Err(RestoreError::InvalidOriginalPath);
    };
    let files_ord = files_root.ordinary_directory().map_err(RestoreError::Io)?;
    let source_entry = files_ord
        .retain_entry(payload_name)
        .map_err(RestoreError::Io)?;
    let dest_parent = RetainedDir::open(original_parent).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            RestoreError::ParentUnavailable {
                parent: original_parent.to_path_buf(),
            }
        } else {
            RestoreError::Io(e)
        }
    })?;
    match source_entry.move_noreplace(&dest_parent, original_name) {
        Ok(_moved) => Ok(()),
        Err(failure) => {
            use enzui_security::secure_dir::MoveCustody;
            // Source custody: nothing reached the destination, so retiring the twin
            // is safe.
            if matches!(*failure.custody, MoveCustody::Source(_)) {
                return Err(if failure.error.kind() == io::ErrorKind::AlreadyExists {
                    RestoreError::DestinationOccupied {
                        at: entry.original_path.clone(),
                    }
                } else {
                    RestoreError::Io(failure.error)
                });
            }
            // Bytes may be at the original path; report `Pending` so callers keep
            // the metadata twin.
            let custody = match *failure.custody {
                MoveCustody::Source(_) => PublishCustody::Source,
                MoveCustody::Destination(_) => PublishCustody::Destination,
                MoveCustody::Both { .. } => PublishCustody::Both,
                MoveCustody::Uncertain { .. } => PublishCustody::Uncertain,
            };
            Err(RestoreError::Pending {
                custody,
                diagnostic: failure.error.to_string(),
            })
        }
    }
}

#[allow(
    unsafe_code,
    reason = "libc renameat2 FFI (legacy None-authority fallback)."
)]
fn restore_payload_legacy_renameat2(entry: &TrashEntry) -> Result<(), RestoreError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let src = CString::new(entry.trashed_path.as_os_str().as_bytes())
        .map_err(|_| RestoreError::InvalidTrashPath)?;
    let dst = CString::new(entry.original_path.as_os_str().as_bytes())
        .map_err(|_| RestoreError::InvalidOriginalPath)?;
    // SAFETY: both NUL-terminated paths outlive the syscall; NOREPLACE protects occupied destinations.
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            src.as_ptr(),
            libc::AT_FDCWD,
            dst.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        let error = io::Error::last_os_error();
        return Err(if error.kind() == io::ErrorKind::AlreadyExists {
            RestoreError::DestinationOccupied {
                at: entry.original_path.clone(),
            }
        } else {
            RestoreError::Io(error)
        });
    }
    Ok(())
}

// Restores the payload and retires the `.trashinfo` twin.
//
// A [`RestoreError::Pending`] returns before retirement, so the twin is unlinked
// only after a settled restore.
pub(crate) fn restore_without_replacing(entry: &TrashEntry) -> Result<(), RestoreError> {
    restore_payload_without_replacing(entry)?;
    // Descriptor-relative unlink, so a substituted `<trash>/info` cannot redirect it.
    if let Some(authority) = entry.retained_root.as_ref() {
        if let Some(info_name) = entry.info_path.file_name() {
            let _ = authority.info_root().unlink_entry(info_name);
        } else {
            let _ = fs::remove_file(&entry.info_path);
        }
    } else {
        let _ = fs::remove_file(&entry.info_path);
    }
    Ok(())
}

// Why a metadata-twin publication failed; each variant names which paths hold
// whose bytes.
#[derive(Debug)]
pub(crate) enum PublishError {
    // Pre-publish state intact at `info_path`.
    NothingPublished { detail: String },
    // Foreign entry occupies `info_path`; its bytes untouched.
    Occupied,
    // Post-link settlement failed and cleanup settled; pre-publish state intact.
    UnpublishedAfterSettlementFailure { detail: String },
    // Post-link settlement failed and cleanup did not settle; pre-publish state is
    // not guaranteed.
    UncertainAfterSettlementFailure { detail: String },
    // Final name denotes a foreign inode; its bytes never removed.
    SubstitutedForeignTwin { detail: String },
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NothingPublished { detail } => write!(f, "{detail}"),
            Self::Occupied => write!(f, "metadata twin path is occupied by a foreign entry"),
            Self::UnpublishedAfterSettlementFailure { detail } => write!(
                f,
                "published twin was unpublished by identity after settlement failure: {detail}"
            ),
            Self::UncertainAfterSettlementFailure { detail } => write!(
                f,
                "published twin custody uncertain after settlement failure: {detail}"
            ),
            Self::SubstitutedForeignTwin { detail } => write!(
                f,
                "final twin path was substituted by a foreign inode: {detail}"
            ),
        }
    }
}

// Admission-time byte cap for the `.trashinfo` twin.
pub(crate) const MAX_ADMITTED_TRASHINFO_BYTES: usize = 8 * 1024;

// Why a metadata-twin retire attempt failed; each variant names which paths hold
// whose bytes.
#[derive(Debug)]
pub(crate) enum RetireError {
    // Twin identity mismatches admitted; foreign bytes untouched.
    Substituted { detail: String },
    // Twin already gone; caller treats as success.
    Vanished,
    // Identity matched but unlink failed; twin bytes still present.
    RemovalFailed { detail: String },
    // The identity-bound unlink landed but its parent sync did not settle.
    ParentSyncFailed { detail: String },
}

impl fmt::Display for RetireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Substituted { detail } => write!(f, "substituted `.trashinfo` twin: {detail}"),
            Self::Vanished => write!(f, "`.trashinfo` twin already gone"),
            Self::RemovalFailed { detail } => {
                write!(f, "identity-bound twin removal failed: {detail}")
            }
            Self::ParentSyncFailed { detail } => write!(
                f,
                "twin parent-directory sync failed after removal: {detail}"
            ),
        }
    }
}

// Sabotage seams for `retire_metadata_twin`; no-ops in product builds.
#[cfg(test)]
pub(crate) mod retire_fault {
    use std::cell::RefCell;
    use std::path::Path;

    type PathAction = Box<dyn FnOnce(&Path)>;

    thread_local! {
        static AFTER_IDENTITY: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static BETWEEN_UNLINK_AND_SYNC: RefCell<Option<PathAction>> =
            const { RefCell::new(None) };
    }

    pub(super) fn take_after_identity() -> Option<PathAction> {
        AFTER_IDENTITY.with(|s| s.borrow_mut().take())
    }

    pub(super) fn take_between_unlink_and_sync() -> Option<PathAction> {
        BETWEEN_UNLINK_AND_SYNC.with(|s| s.borrow_mut().take())
    }

    pub(crate) fn clear() {
        AFTER_IDENTITY.with(|s| *s.borrow_mut() = None);
        BETWEEN_UNLINK_AND_SYNC.with(|s| *s.borrow_mut() = None);
    }
}

// Bounded nofollow read of the `.trashinfo` twin, bound to the descriptor's
// `(dev, ino)` so a later substitution is detected. Fails on any I/O error, a
// non-regular file, or more than [`MAX_ADMITTED_TRASHINFO_BYTES`].
pub(crate) fn capture_admitted_metadata_twin(
    info_path: &Path,
) -> io::Result<(crate::journal::InodeIdent, Vec<u8>)> {
    use enzui_security::platform;
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // `O_NONBLOCK` so a FIFO cannot block `open(2)` before the kind check.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | platform::libc_o_nonblock())
        .open(info_path)?;
    let ident = crate::journal::InodeIdent::from_file(&file)?;
    if ident.kind != crate::journal::FileKind::Regular {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            crate::i18n::tr("`.trashinfo` twin is not a regular file"),
        ));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_ADMITTED_TRASHINFO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_ADMITTED_TRASHINFO_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            crate::i18n::tr("`.trashinfo` twin exceeds admission byte cap"),
        ));
    }
    Ok((ident, bytes))
}

// Retires the `.trashinfo` twin only while its bytes match `expected_bytes`,
// binding the unlink to the held descriptor; see [`RetireError`].
pub(crate) fn retire_metadata_twin(
    info_path: &Path,
    expected_bytes: &[u8],
) -> Result<(), RetireError> {
    use enzui_core::tempstage;
    use enzui_security::{durable, platform};
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    // `O_NONBLOCK`: see [`capture_admitted_metadata_twin`].
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | platform::libc_o_nonblock())
        .open(info_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(RetireError::Vanished),
        Err(e) => {
            return Err(RetireError::Substituted {
                detail: e.to_string(),
            });
        }
    };
    let ident =
        crate::journal::InodeIdent::from_file(&file).map_err(|e| RetireError::Substituted {
            detail: e.to_string(),
        })?;
    if ident.kind != crate::journal::FileKind::Regular {
        return Err(RetireError::Substituted {
            detail: crate::i18n::tr("twin path no longer denotes a regular file").into(),
        });
    }
    let mut current = Vec::with_capacity(expected_bytes.len());
    if let Err(e) = (&file)
        .take(MAX_ADMITTED_TRASHINFO_BYTES as u64 + 1)
        .read_to_end(&mut current)
    {
        return Err(RetireError::Substituted {
            detail: e.to_string(),
        });
    }
    if current != expected_bytes {
        return Err(RetireError::Substituted {
            detail: crate::i18n::format(
                "files-admitted-bytes-but-path-now-denotes-2b14af28",
                &[
                    ("arg0", (expected_bytes.len()).to_string()),
                    ("arg1", (current.len()).to_string()),
                ],
            ),
        });
    }

    #[cfg(test)]
    if let Some(action) = retire_fault::take_after_identity() {
        action(info_path);
    }

    if let Err(e) = tempstage::remove_if_path_names_file(info_path, &file) {
        return Err(RetireError::RemovalFailed {
            detail: e.to_string(),
        });
    }

    #[cfg(test)]
    if let Some(action) = retire_fault::take_between_unlink_and_sync() {
        action(info_path);
    }

    if let Err(e) = durable::sync_parent_dir(info_path) {
        return Err(RetireError::ParentSyncFailed {
            detail: e.to_string(),
        });
    }
    Ok(())
}

// Sabotage seams for `publish_metadata_twin`; no-ops in product builds.
#[cfg(test)]
pub(crate) mod publish_fault {
    use std::cell::RefCell;
    use std::path::Path;

    type PathAction = Box<dyn FnOnce(&Path)>;

    thread_local! {
        static AFTER_LINK: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static BETWEEN_CLEANUP_AND_SYNC: RefCell<Option<PathAction>> =
            const { RefCell::new(None) };
    }

    pub(crate) fn arm_after_link(action: PathAction) {
        AFTER_LINK.with(|s| *s.borrow_mut() = Some(action));
    }

    pub(super) fn take_after_link() -> Option<PathAction> {
        AFTER_LINK.with(|s| s.borrow_mut().take())
    }

    pub(super) fn take_between_cleanup_and_sync() -> Option<PathAction> {
        BETWEEN_CLEANUP_AND_SYNC.with(|s| s.borrow_mut().take())
    }

    pub(crate) fn clear() {
        AFTER_LINK.with(|s| *s.borrow_mut() = None);
        BETWEEN_CLEANUP_AND_SYNC.with(|s| *s.borrow_mut() = None);
    }
}

// Links `bytes` at `info_path` without overwriting, via a held inode, so the name
// either holds exactly `bytes` or [`PublishError`] is returned. A foreign inode at
// `info_path` is never removed.
pub(crate) fn publish_metadata_twin(info_path: &Path, bytes: &[u8]) -> Result<(), PublishError> {
    use enzui_core::tempstage;
    use enzui_security::durable;

    // Two-phase: [`TempStage::commit`] syncs and renames to a private sibling (the
    // only path that fires `fault::arm_sync`), then `linkat` publishes the held
    // inode at `info_path` without replacing an occupied slot.
    let staging_dest =
        tempstage::unique_temp_sibling(info_path).map_err(|e| PublishError::NothingPublished {
            detail: e.to_string(),
        })?;

    let mut stage = tempstage::TempStage::create_document(&staging_dest).map_err(|e| {
        PublishError::NothingPublished {
            detail: e.to_string(),
        }
    })?;
    stage
        .write_all(bytes)
        .map_err(|e| PublishError::NothingPublished {
            detail: e.to_string(),
        })?;

    // The duplicated fd keeps the inode addressable after `commit`, so cleanup and
    // `linkat` never reopen a substitutable pathname.
    let held = stage
        .file_mut()
        .try_clone()
        .map_err(|e| PublishError::NothingPublished {
            detail: e.to_string(),
        })?;

    if let Err(commit_err) = stage.commit() {
        // The identity-bound cleanup tells the cases apart: `NotFound` means
        // `rename` never ran; `Ok` means it ran and the parent `fsync` failed.
        return Err(
            match tempstage::remove_if_path_names_file(&staging_dest, &held) {
                Err(rm) if rm.kind() == io::ErrorKind::NotFound => PublishError::NothingPublished {
                    detail: commit_err.to_string(),
                },
                Ok(()) => {
                    // Same `Unpublished` / `Uncertain` split as
                    // `unpublish_after_settlement_failure`.
                    #[cfg(test)]
                    if let Some(action) = publish_fault::take_between_cleanup_and_sync() {
                        action(&staging_dest);
                    }
                    let detail = crate::i18n::format(
                        "files-staging-parent-sync-failed-commit-err-0cb4f9e0",
                        &[("commit_err", (commit_err).to_string())],
                    );
                    match durable::sync_parent_dir(&staging_dest) {
                        Ok(()) => PublishError::UnpublishedAfterSettlementFailure { detail },
                        Err(barrier) => PublishError::UncertainAfterSettlementFailure {
                            detail: crate::i18n::format(
                                "files-detail-identity-bound-removal-succeeded-but-0e76eba8",
                                &[("detail", (detail)), ("barrier", (barrier).to_string())],
                            ),
                        },
                    }
                }
                Err(other) => PublishError::UncertainAfterSettlementFailure {
                    detail: crate::i18n::format(
                        "files-commit-err-identity-bound-cleanup-refused-05f5b1e0",
                        &[
                            ("commit_err", (commit_err).to_string()),
                            ("other", (other).to_string()),
                        ],
                    ),
                },
            },
        );
    }

    // `linkat` exclusive-create keeps the occupied-slot refusal a `rename` lacks.
    if let Err(e) = tempstage::link_new_by_identity(&held, info_path) {
        // Best-effort identity-bound cleanup of the staged sibling.
        let _ = tempstage::remove_if_path_names_file(&staging_dest, &held);
        return Err(match e.kind() {
            io::ErrorKind::AlreadyExists => PublishError::Occupied,
            _ => PublishError::NothingPublished {
                detail: e.to_string(),
            },
        });
    }

    #[cfg(test)]
    if let Some(action) = publish_fault::take_after_link() {
        action(info_path);
    }

    if let Err(e) = durable::sync_parent_dir(info_path) {
        let _ = tempstage::remove_if_path_names_file(&staging_dest, &held);
        let mut cleanup_file = held;
        return Err(unpublish_after_settlement_failure(
            &mut cleanup_file,
            info_path,
            crate::i18n::format(
                "files-parent-directory-sync-failed-e-b4541116",
                &[("e", (e).to_string())],
            ),
        ));
    }

    // Refuse a substituted foreign inode without removing it.
    if let Err(e) = tempstage::verify_path_names_file(&held, info_path) {
        let _ = tempstage::remove_if_path_names_file(&staging_dest, &held);
        return Err(PublishError::SubstitutedForeignTwin {
            detail: e.to_string(),
        });
    }

    // The staged sibling is redundant; a crash before this unlink settles leaves
    // only a private sibling sharing the published bytes.
    let _ = tempstage::remove_if_path_names_file(&staging_dest, &held);
    Ok(())
}

// Any failure here surfaces uncertainty, not a false pre-publish claim.
fn unpublish_after_settlement_failure(
    file: &mut std::fs::File,
    info_path: &Path,
    detail: String,
) -> PublishError {
    if let Err(e) = enzui_core::tempstage::remove_if_path_names_file(info_path, file) {
        return PublishError::UncertainAfterSettlementFailure {
            detail: crate::i18n::format(
                "files-detail-identity-bound-cleanup-refused-e-5bb035ac",
                &[("detail", (detail)), ("e", (e).to_string())],
            ),
        };
    }
    #[cfg(test)]
    if let Some(action) = publish_fault::take_between_cleanup_and_sync() {
        action(info_path);
    }
    if let Err(e) = enzui_security::durable::sync_parent_dir(info_path) {
        return PublishError::UncertainAfterSettlementFailure {
            detail: crate::i18n::format(
                "files-detail-identity-bound-removal-succeeded-but-3c5b0128",
                &[("detail", (detail)), ("e", (e).to_string())],
            ),
        };
    }
    PublishError::UnpublishedAfterSettlementFailure { detail }
}
