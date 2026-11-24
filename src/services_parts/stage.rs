// Staging, seams, and cross-filesystem move replacing.

use super::publish::{
    CopiedSources, copy_tree_into, fsync_dir, fsync_staged, marker_sibling, publish_with_backup,
    remove_stage_residue_fallible, remove_stage_residue_report, reserve_unused_sibling,
    source_kept,
};
use super::*;

// `Merge` and `Replace` stay separate functions: a merge that truncated a
// directory would be destructive and invisible in a screenshot.

// Moves `src` over `dst`, replacing it, with stage-and-publish safety and a
// copy fallback across filesystems.
//
// Used for `ConflictChoice::Replace` on a file conflict or type mismatch. The
// existing destination survives until the replacement is fully staged, no
// partial final path is visible, and the source is removed only after
// publication completes (see [`move_replacing_with`]).
pub(crate) fn move_replacing(src: &Path, dst: &Path) -> Result<(), String> {
    move_replacing_with(src, dst, MoveSeams::PRODUCTION)
}

// Production renames use NOREPLACE except the initial replacement attempt; injected seams preserve that boundary.
#[derive(Clone, Copy)]
pub(crate) struct MoveSeams {
    pub(crate) initial_rename: fn(&Path, &Path) -> io::Result<()>,
    pub(crate) publish_rename: fn(&Path, &Path) -> io::Result<()>,
    pub(crate) stage_copy_override: Option<fn(&Path, &Path) -> io::Result<()>>,
    pub(crate) after_stage_flush: fn(&Path, &Path) -> io::Result<()>,
    pub(crate) post_publish_fsync: fn(&Path) -> io::Result<()>,
    pub(crate) backup_cleanup: fn(&Path) -> io::Result<()>,
    // Whether an existing destination may be replaced. A paste item planned
    // onto a free name runs with `false`: whatever appeared there since the
    // plan is reported, never overwritten.
    pub(crate) replace: bool,
}

impl MoveSeams {
    // Real renames, staging, fsync and backup cleanup.
    pub(crate) const PRODUCTION: Self = Self {
        initial_rename: rename_paths,
        publish_rename: rename_paths_no_replace,
        stage_copy_override: None,
        after_stage_flush: noop_after_stage_flush,
        post_publish_fsync: fsync_dir,
        backup_cleanup: remove_stage_residue_fallible,
        replace: true,
    };

    // Production seams for a destination planned as free: every rename refuses
    // to overwrite, so a name that appeared after planning fails the item.
    pub(crate) const PLACE: Self = Self {
        initial_rename: rename_no_replace_portable,
        replace: false,
        ..Self::PRODUCTION
    };
}

// The message for a destination that must stay untouched but exists.
pub(crate) fn destination_exists(dst: &Path) -> String {
    let name = dst.file_name().map_or_else(
        || dst.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    crate::i18n::format("files-already-exists-f61b07b3", &[("arg0", name)])
}

// A no-replace rename that still works where `RENAME_NOREPLACE` is missing
// (some FUSE and network filesystems): there it checks, then renames. The
// check is racy, but only on filesystems that offer nothing better.
fn rename_no_replace_portable(src: &Path, dst: &Path) -> io::Result<()> {
    match rename_paths_no_replace(src, dst) {
        Err(e)
            if e.kind() == io::ErrorKind::Unsupported || e.raw_os_error() == Some(libc::EINVAL) =>
        {
            if dst.symlink_metadata().is_ok() {
                return Err(io::Error::from_raw_os_error(libc::EEXIST));
            }
            std::fs::rename(src, dst)
        }
        other => other,
    }
}

// Renames `from` back to `to` within one parent, refusing to overwrite.
pub(crate) fn rename_reversing(from: &Path, to: &Path) -> Result<(), String> {
    rename_paths_no_replace(from, to).map_err(|e| e.to_string())
}

// Same-filesystem no-replace rename that returns the raw `io::Result`, so
// callers can distinguish `EXDEV`.
pub(crate) fn same_fs_no_replace_rename(from: &Path, to: &Path) -> io::Result<()> {
    rename_paths_no_replace(from, to)
}

// `true` when `err` came from a `rename(2)` that crossed a mount
// point (`EXDEV`).
pub(crate) fn is_cross_filesystem_error(err: &io::Error) -> bool {
    is_exdev(err)
}

// A named fn keeps `MoveSeams::PRODUCTION` const: closure-to-fn coercion is
// not stable in const context.
fn noop_after_stage_flush(_: &Path, _: &Path) -> io::Result<()> {
    Ok(())
}

// Post-condition of a [`publish_with_backup`] call.
//
// Each variant names which paths exist afterwards, so callers decide on
// source rollback, source removal or backup retention without parsing text.
pub(crate) enum PublishOutcome {
    // `dst` holds the new bytes, its parent is fsynced and any backup is gone.
    Published,
    // Published durably, but the backup could not be removed and remains as
    // residue for manual cleanup; the source may still be removed.
    PublishedBackupRetained {
        backup: PathBuf,
        cleanup_error: String,
    },
    // `dst` holds the new bytes but the parent fsync failed; the backup is kept
    // so the old bytes survive a lost dirent, and the source must be kept.
    PublishedUncertain {
        backup: Option<PathBuf>,
        fsync_error: String,
    },
    // `dst` is in its pre-publish state, the staged bytes remain, and no backup
    // residue survives.
    NotPublished {
        publish_error: String,
    },
    // `dst` is missing, the staged bytes remain, and the old bytes are at
    // `backup` after a failed rollback; manual recovery is required.
    Catastrophic {
        publish_error: String,
        rollback_error: String,
        backup: PathBuf,
    },
}

// Stage before replacement; retain recovery backups and remove sources only after durable publication.
pub(crate) fn move_replacing_with(src: &Path, dst: &Path, seams: MoveSeams) -> Result<(), String> {
    // FIFOs and sockets would block the first `open(2)`; devices have no
    // copyable byte stream.
    if let SourceKind::Unsupported(msg) = classify_source(src).map_err(|e| e.to_string())? {
        return Err(msg);
    }
    match (seams.initial_rename)(src, dst) {
        Ok(()) => finish_fast_path_move(src, dst, &seams),
        Err(e) if !seams.replace && e.raw_os_error() == Some(libc::EEXIST) => {
            Err(destination_exists(dst))
        }
        Err(e) if is_exdev(&e) => stage_cross_fs_replace(src, dst, seams),
        Err(e) if is_stageable_same_fs_error(&e) => stage_same_fs_replace(src, dst, seams),
        Err(e) => Err(e.to_string()),
    }
}

// Fsyncs each distinct parent after a fast-path rename, since rename
// atomicity is not crash durability.
//
// A failed fsync cannot be rolled back, so it is reported as uncertainty.
fn finish_fast_path_move(src: &Path, dst: &Path, seams: &MoveSeams) -> Result<(), String> {
    let src_parent = src.parent();
    let dst_parent = dst.parent();
    let same_parent = matches!((src_parent, dst_parent), (Some(a), Some(b)) if a == b);
    let parents: [Option<&Path>; 2] = if same_parent {
        [src_parent, None]
    } else {
        [src_parent, dst_parent]
    };
    for parent in parents.into_iter().flatten() {
        if let Err(e) = (seams.post_publish_fsync)(parent) {
            return Err(crate::i18n::format(
                "files-renamed-to-but-durability-of-the-9258400c",
                &[
                    ("arg0", (src.display()).to_string()),
                    ("arg1", (dst.display()).to_string()),
                    ("arg2", (parent.display()).to_string()),
                    ("e", (e).to_string()),
                ],
            ));
        }
    }
    Ok(())
}

// Generic `std::fs::rename` does not coerce to the `MoveSeams` fn-pointer type.
fn rename_paths(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::rename(src, dst)
}

// No-replace rename via `renameat2(RENAME_NOREPLACE)`.
//
// Every rename inside [`publish_with_backup`] and [`stage_same_fs_replace`]
// targets a name that must not exist; a racing sibling yields `EEXIST`
// instead of being overwritten. Without kernel support (`ENOSYS`) this
// returns `ErrorKind::Unsupported` rather than a racy check-then-rename.
pub(crate) fn rename_paths_no_replace(src: &Path, dst: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags};
    use rustix::io::Errno;
    match rustix::fs::renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::NOSYS) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            crate::i18n::format(
                "files-renameat2-rename-noreplace-is-not-available-fdf380c3",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("arg1", (src.display()).to_string()),
                ],
            ),
        )),
        Err(errno) => Err(io::Error::from(errno)),
    }
}

// Decided on the raw errno, not a localised message.
fn is_exdev(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EXDEV)
}

// Errnos the stage-and-publish branch can resolve. `EBUSY` and `EPERM` are
// excluded: staging in the same parent would hit the same wall.
fn is_stageable_same_fs_error(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOTEMPTY | libc::EEXIST | libc::EISDIR | libc::ENOTDIR)
    )
}

// Source entries the move contract handles. FIFOs, sockets and devices are
// rejected up front: a FIFO `open(2)` blocks until a reader appears.
enum SourceKind {
    File,
    Dir,
    Symlink,
    Unsupported(String),
}

fn classify_source(src: &Path) -> io::Result<SourceKind> {
    let md = src.symlink_metadata()?;
    let ft = md.file_type();
    if ft.is_symlink() {
        Ok(SourceKind::Symlink)
    } else if ft.is_dir() {
        Ok(SourceKind::Dir)
    } else if ft.is_file() {
        Ok(SourceKind::File)
    } else {
        Ok(SourceKind::Unsupported(crate::i18n::format(
            "files-cannot-move-unsupported-source-file-type-58ae7b19",
            &[
                ("arg0", (src.display()).to_string()),
                ("arg1", (describe_special_type(ft)).to_owned()),
            ],
        )))
    }
}

pub(crate) fn describe_special_type(ft: std::fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt;
    if ft.is_fifo() {
        crate::i18n::tr("named pipe (FIFO)")
    } else if ft.is_socket() {
        crate::i18n::tr("unix-domain socket")
    } else if ft.is_block_device() {
        crate::i18n::tr("block device")
    } else if ft.is_char_device() {
        crate::i18n::tr("character device")
    } else {
        crate::i18n::tr("special file")
    }
}

// Retry cap for reserving a staging or backup name; `EEXIST` from the
// exclusive create, not the PID/time label, decides when to retry.
pub(crate) const STAGE_RESERVE_RETRIES: usize = 8;

// Reserves and fills a staging entry next to `dst` with exclusive creation
// (`O_EXCL`, `mkdir(2)` or `symlink(2)`), copying through the owned fd so a
// symlink race cannot redirect the copy.
//
// Retries on `EEXIST` up to [`STAGE_RESERVE_RETRIES`] times; other errors
// abort. Returns the reserved path and the source entries it copied.
fn stage_source_exclusive(
    src_kind: &SourceKind,
    src: &Path,
    dst: &Path,
    entry_copied: &mut dyn FnMut(),
) -> io::Result<(PathBuf, CopiedSources)> {
    for _ in 0..STAGE_RESERVE_RETRIES {
        let candidate = marker_sibling(dst, "files-app-stage")?;
        match copy_source_exclusive(src_kind, src, &candidate, entry_copied) {
            Ok(copied) => return Ok((candidate, copied)),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(crate::i18n::format(
        "files-could-not-reserve-a-unique-staging-d7b94590",
        &[
            ("arg0", (dst.display()).to_string()),
            ("arg1", (STAGE_RESERVE_RETRIES).to_string()),
        ],
    )))
}

// Copies `src` to the new name `staged`, created exclusively, and removes
// whatever it created on failure. `EEXIST` means the name was taken.
fn copy_source_exclusive(
    src_kind: &SourceKind,
    src: &Path,
    staged: &Path,
    entry_copied: &mut dyn FnMut(),
) -> io::Result<CopiedSources> {
    let mut copied = CopiedSources::default();
    match src_kind {
        SourceKind::File => reserve_and_copy_file(src, staged, &mut copied)?,
        SourceKind::Dir => reserve_and_copy_dir(src, staged, entry_copied, &mut copied)?,
        SourceKind::Symlink => {
            let md = src.symlink_metadata()?;
            std::os::unix::fs::symlink(std::fs::read_link(src)?, staged)?;
            copied.record(src, &md);
        }
        SourceKind::Unsupported(msg) => {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, msg.clone()));
        }
    }
    Ok(copied)
}

// Copies `src` to the new path `dst` and syncs it, refusing FIFOs, sockets and
// devices. Returns the source entries copied, for removing exactly those.
pub(crate) fn copy_new_for_move(src: &Path, dst: &Path) -> Result<CopiedSources, String> {
    let src_kind = classify_source(src).map_err(|e| e.to_string())?;
    if let SourceKind::Unsupported(msg) = &src_kind {
        return Err(msg.clone());
    }
    let copied =
        copy_source_exclusive(&src_kind, src, dst, &mut || {}).map_err(|e| e.to_string())?;
    if let Err(err) = fsync_staged(dst) {
        let cleanup = remove_stage_residue_report(dst);
        return Err(format!("{err}{cleanup}"));
    }
    Ok(copied)
}

fn reserve_and_copy_file(src: &Path, staged: &Path, copied: &mut CopiedSources) -> io::Result<()> {
    // Synced inside, so the data is durable before `fsync_staged` syncs the
    // directory.
    match super::publish::copy_file_preserving(src, staged) {
        Ok(read) => {
            copied.record(src, &read);
            Ok(())
        }
        // The reservation lost a race: nothing of ours to remove.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => {
            // Best effort: the caller reports the original error.
            let _ = std::fs::remove_file(staged);
            Err(e)
        }
    }
}

fn reserve_and_copy_dir(
    src: &Path,
    staged: &Path,
    entry_copied: &mut dyn FnMut(),
    copied: &mut CopiedSources,
) -> io::Result<()> {
    let md = src.symlink_metadata()?;
    super::publish::create_private_dir(staged)?;
    let result = copy_tree_into(src, staged, entry_copied, copied)
        .map_err(io::Error::other)
        .and_then(|()| super::publish::finish_copied_dir(&md, staged));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staged);
    } else {
        copied.record(src, &md);
    }
    result
}

// Same-filesystem branch for a type mismatch or non-empty directory: renames
// `src` to a sibling of `dst`, publishes it with backup rollback, and renames
// it back to `src` on publish failure.
fn stage_same_fs_replace(src: &Path, dst: &Path, seams: MoveSeams) -> Result<(), String> {
    let staged = reserve_unused_sibling(dst, "files-app-stage").map_err(|e| e.to_string())?;
    (seams.publish_rename)(src, &staged).map_err(|e| e.to_string())?;
    match publish_with_backup(&staged, dst, &seams) {
        PublishOutcome::Published => {
            // publish_with_backup only fsynced dst.parent().
            fsync_src_parent_if_distinct(src, dst, &seams)?;
            Ok(())
        }
        PublishOutcome::PublishedBackupRetained {
            backup,
            cleanup_error,
        } => {
            // Publish is durable and src is already empty; no rollback.
            fsync_src_parent_if_distinct(src, dst, &seams)?;
            Err(crate::i18n::format(
                "files-same-fs-move-to-succeeded-but-dc8a6a4e",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("arg1", (backup.display()).to_string()),
                    ("arg2", (cleanup_error)),
                ],
            ))
        }
        PublishOutcome::PublishedUncertain {
            backup,
            fsync_error,
        } => {
            // Backup is kept so OLD survives a lost dirent; src is already empty.
            Err(match backup {
                Some(b) => crate::i18n::format(
                    "files-same-fs-publish-to-unconfirmed-dst-5b9a3356",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("fsync_error", (fsync_error)),
                        ("arg1", (b.display()).to_string()),
                        ("arg2", (src.display()).to_string()),
                    ],
                ),
                None => crate::i18n::format(
                    "files-same-fs-publish-to-unconfirmed-dst-c44e2e28",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("fsync_error", (fsync_error)),
                        ("arg1", (src.display()).to_string()),
                    ],
                ),
            })
        }
        PublishOutcome::NotPublished { publish_error } => {
            // NO_REPLACE: a racing sibling at src fails EEXIST.
            match (seams.publish_rename)(&staged, src) {
                Ok(()) => Err(publish_error),
                Err(rollback_err) => {
                    let cleanup = residue_report(&staged);
                    Err(crate::i18n::format(
                        "files-catastrophic-same-fs-publish-failed-publish-940ae051",
                        &[
                            ("publish_error", (publish_error)),
                            ("arg0", (staged.display()).to_string()),
                            ("arg1", (src.display()).to_string()),
                            ("rollback_err", (rollback_err).to_string()),
                            ("arg2", (staged.display()).to_string()),
                            ("cleanup", (cleanup)),
                        ],
                    ))
                }
            }
        }
        PublishOutcome::Catastrophic {
            publish_error,
            rollback_error,
            backup,
        } => {
            // Restore src from staged; the backup holds unrelated OLD dst bytes.
            match (seams.publish_rename)(&staged, src) {
                Ok(()) => Err(crate::i18n::format(
                    "files-catastrophic-same-fs-publish-for-publish-0cc42bf5",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("publish_error", (publish_error)),
                        ("rollback_error", (rollback_error)),
                        ("arg1", (backup.display()).to_string()),
                    ],
                )),
                Err(source_rollback_err) => Err(crate::i18n::format(
                    "files-catastrophic-same-fs-publish-for-publish-78906ffe",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("publish_error", (publish_error)),
                        ("rollback_error", (rollback_error)),
                        ("source_rollback_err", (source_rollback_err).to_string()),
                        ("arg1", (staged.display()).to_string()),
                        ("arg2", (backup.display()).to_string()),
                    ],
                )),
            }
        }
    }
}

// publish_with_backup only fsyncs dst.parent(); a distinct source parent
// needs its own fsync for the removal to be durable.
fn fsync_src_parent_if_distinct(src: &Path, dst: &Path, seams: &MoveSeams) -> Result<(), String> {
    let Some(src_parent) = src.parent() else {
        return Ok(());
    };
    let dst_parent = dst.parent();
    if dst_parent == Some(src_parent) {
        return Ok(());
    }
    (seams.post_publish_fsync)(src_parent).map_err(|e| {
        crate::i18n::format(
            "files-same-fs-publish-to-completed-but-79bf598b",
            &[
                ("arg0", (dst.display()).to_string()),
                ("arg1", (src_parent.display()).to_string()),
                ("e", (e).to_string()),
            ],
        )
    })
}

// Verify the residue at `path` is absent; if it still exists, return a
// truthful location report string.
fn residue_report(path: &Path) -> String {
    match path.symlink_metadata() {
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Ok(_) => crate::i18n::format(
            "files-residue-at-was-not-cleaned-up-ea0e8ee7",
            &[("arg0", (path.display()).to_string())],
        ),
        Err(e) => crate::i18n::format(
            "files-residue-cleanup-verification-at-failed-e-cc92b046",
            &[
                ("arg0", (path.display()).to_string()),
                ("e", (e).to_string()),
            ],
        ),
    }
}

// Reserves, fills and flushes a unique staged sibling of `dst` shaped like
// `src`, returning its path; on error any residue is already reported.
//
// Shared by cross-filesystem move ([`stage_cross_fs_replace`], which then
// removes the source) and copy (`copy_replacing_with`, which never does).
pub(crate) fn stage_source_and_flush(
    src: &Path,
    dst: &Path,
    seams: &MoveSeams,
) -> Result<PathBuf, String> {
    stage_source_and_flush_with_progress(src, dst, seams, &mut || {}).map(|(staged, _)| staged)
}

// Also returns the source entries staged; an override hook records none.
pub(crate) fn stage_source_and_flush_with_progress(
    src: &Path,
    dst: &Path,
    seams: &MoveSeams,
    entry_copied: &mut dyn FnMut(),
) -> Result<(PathBuf, CopiedSources), String> {
    let src_kind = classify_source(src).map_err(|e| e.to_string())?;
    if let SourceKind::Unsupported(msg) = &src_kind {
        return Err(msg.clone());
    }

    let (staged, copied) = if let Some(hook) = seams.stage_copy_override {
        let candidate = marker_sibling(dst, "files-app-stage").map_err(|e| e.to_string())?;
        match hook(src, &candidate) {
            Ok(()) => (candidate, CopiedSources::default()),
            Err(err) => {
                let cleanup = remove_stage_residue_report(&candidate);
                return Err(format!("{err}{cleanup}"));
            }
        }
    } else {
        stage_source_exclusive(&src_kind, src, dst, entry_copied).map_err(|e| e.to_string())?
    };

    if let Err(err) = fsync_staged(&staged) {
        let cleanup = remove_stage_residue_report(&staged);
        return Err(format!("{err}{cleanup}"));
    }
    if let Err(err) = (seams.after_stage_flush)(src, &staged) {
        let cleanup = remove_stage_residue_report(&staged);
        return Err(format!("{err}{cleanup}"));
    }

    Ok((staged, copied))
}

// Cross-filesystem branch after `EXDEV`: stage, fsync, publish with backup
// rollback, then remove the copied source entries only after the final fsync
// succeeds.
//
// Any failure keeps the source and old destination; a failed tail fsync
// leaves the destination published and the source retained as a duplicate.
// Source entries created or changed during the copy are kept and reported.
fn stage_cross_fs_replace(src: &Path, dst: &Path, seams: MoveSeams) -> Result<(), String> {
    let (staged, copied) = stage_source_and_flush_with_progress(src, dst, &seams, &mut || {})?;

    match publish_with_backup(&staged, dst, &seams) {
        PublishOutcome::Published => {
            // A distinct error keeps the batch from claiming success while part
            // of the source is still visible.
            copied
                .remove_from(src)
                .map_err(|kept| source_kept(dst, &kept))
        }
        PublishOutcome::PublishedBackupRetained {
            backup,
            cleanup_error,
        } => {
            // Publish is durable, so removing the source is safe.
            copied
                .remove_from(src)
                .map_err(|kept| source_kept(dst, &kept))?;
            Err(crate::i18n::format(
                "files-copied-to-but-backup-residue-at-99d8e716",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("arg1", (backup.display()).to_string()),
                    ("arg2", (cleanup_error)),
                ],
            ))
        }
        PublishOutcome::PublishedUncertain {
            backup,
            fsync_error,
        } => {
            // Durability unconfirmed: backup and source are retained.
            Err(match backup {
                Some(b) => crate::i18n::format(
                    "files-published-bytes-at-but-durability-of-955f5af4",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("fsync_error", (fsync_error)),
                        ("arg1", (b.display()).to_string()),
                        ("arg2", (src.display()).to_string()),
                    ],
                ),
                None => crate::i18n::format(
                    "files-published-bytes-at-but-durability-of-ec63be5f",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("fsync_error", (fsync_error)),
                        ("arg1", (src.display()).to_string()),
                    ],
                ),
            })
        }
        PublishOutcome::NotPublished { publish_error } => {
            // Staged duplicates src, so removing it loses no data.
            let cleanup = remove_stage_residue_report(&staged);
            Err(format!("{publish_error}{cleanup}"))
        }
        PublishOutcome::Catastrophic {
            publish_error,
            rollback_error,
            backup,
        } => {
            // Staged duplicates the intact src; only the backup holds OLD.
            let cleanup = remove_stage_residue_report(&staged);
            Err(crate::i18n::format(
                "files-catastrophic-cross-fs-publish-for-publish-acd2146e",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("publish_error", (publish_error)),
                    ("rollback_error", (rollback_error)),
                    ("arg1", (backup.display()).to_string()),
                    ("arg2", (src.display()).to_string()),
                    ("cleanup", (cleanup)),
                ],
            ))
        }
    }
}
