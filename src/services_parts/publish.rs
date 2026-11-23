// Publish with backup, copy, paste plans, and conflict choices.

use super::stage::{
    MoveSeams, PublishOutcome, STAGE_RESERVE_RETRIES, describe_special_type,
    stage_source_and_flush_with_progress,
};
use super::*;

// Publication never overwrites racing siblings; uncertain durability preserves source and recovery custody.
pub(crate) fn publish_with_backup(staged: &Path, dst: &Path, seams: &MoveSeams) -> PublishOutcome {
    let dst_present = dst.symlink_metadata().is_ok();
    let dst_parent = dst.parent();
    if dst_present && !seams.replace {
        return PublishOutcome::NotPublished {
            publish_error: super::stage::destination_exists(dst),
        };
    }

    if !dst_present {
        if let Err(e) = (seams.publish_rename)(staged, dst) {
            return PublishOutcome::NotPublished {
                publish_error: e.to_string(),
            };
        }
        if let Some(parent) = dst_parent
            && let Err(e) = (seams.post_publish_fsync)(parent)
        {
            return PublishOutcome::PublishedUncertain {
                backup: None,
                fsync_error: e.to_string(),
            };
        }
        return PublishOutcome::Published;
    }

    let backup = match swap_dst_to_backup(dst, seams) {
        Ok(b) => b,
        Err(e) => return PublishOutcome::NotPublished { publish_error: e },
    };
    // No-replace: dst is now missing.
    if let Err(publish_err) = (seams.publish_rename)(staged, dst) {
        return match (seams.publish_rename)(&backup, dst) {
            Ok(()) => PublishOutcome::NotPublished {
                publish_error: crate::i18n::format(
                    "files-publish-of-failed-rolled-the-old-3c400133",
                    &[
                        ("arg0", (dst.display()).to_string()),
                        ("arg1", (publish_err).to_string()),
                        ("arg2", (backup.display()).to_string()),
                    ],
                ),
            },
            Err(rollback_err) => PublishOutcome::Catastrophic {
                publish_error: publish_err.to_string(),
                rollback_error: rollback_err.to_string(),
                backup,
            },
        };
    }
    // Fsync the parent before removing the backup so a fsync failure keeps
    // rollback authority; removing the backup first would lose the old bytes.
    if let Some(parent) = dst_parent
        && let Err(e) = (seams.post_publish_fsync)(parent)
    {
        return PublishOutcome::PublishedUncertain {
            backup: Some(backup),
            fsync_error: e.to_string(),
        };
    }
    // dst holds the new bytes and the backup residue holds the old.
    if let Err(e) = (seams.backup_cleanup)(&backup) {
        return PublishOutcome::PublishedBackupRetained {
            backup,
            cleanup_error: e.to_string(),
        };
    }
    // A failed fsync here leaves the removal non-durable; the backup may reappear.
    if let Some(parent) = dst_parent
        && let Err(e) = (seams.post_publish_fsync)(parent)
    {
        return PublishOutcome::PublishedBackupRetained {
            backup,
            cleanup_error: crate::i18n::format(
                "files-backup-was-removed-but-the-removal-ff915778",
                &[("e", (e).to_string())],
            ),
        };
    }
    PublishOutcome::Published
}

// Renames `dst` to a fresh backup sibling without replacing, reserving and
// renaming in one loop so a sibling that appears in between triggers a retry.
fn swap_dst_to_backup(dst: &Path, seams: &MoveSeams) -> Result<PathBuf, String> {
    for _ in 0..STAGE_RESERVE_RETRIES {
        let candidate = marker_sibling(dst, "files-app-backup").map_err(|e| e.to_string())?;
        match (seams.publish_rename)(dst, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(crate::i18n::format(
        "files-could-not-swap-to-a-fresh-21a0bba3",
        &[
            ("arg0", (dst.display()).to_string()),
            ("arg1", (STAGE_RESERVE_RETRIES).to_string()),
        ],
    ))
}

// Fsyncs a staged entry and its parent so a crash after `Ok` recovers the staged
// bytes at the staged name.
//
// Directories are synced recursively; a symlink's dirent is covered by the
// parent fsync.
pub(crate) fn fsync_staged(staged: &Path) -> Result<(), String> {
    let md = staged.symlink_metadata().map_err(|e| e.to_string())?;
    let ft = md.file_type();
    if ft.is_symlink() {
    } else if ft.is_dir() {
        fsync_tree(staged)?;
    } else {
        fsync_file(staged).map_err(|e| e.to_string())?;
    }
    if let Some(parent) = staged.parent() {
        fsync_dir(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// Fsyncs every regular file under `root`, then the directories bottom-up, without
// following symlinks. Any enumeration error fails the whole call.
fn fsync_tree(root: &Path) -> Result<(), String> {
    let rd = std::fs::read_dir(root).map_err(|e| e.to_string())?;
    for dent in rd {
        let dent = dent.map_err(|e| {
            crate::i18n::format(
                "files-aborting-fsync-of-could-not-read-1737dd35",
                &[
                    ("arg0", (root.display()).to_string()),
                    ("e", (e).to_string()),
                ],
            )
        })?;
        let path = dent.path();
        let md = path.symlink_metadata().map_err(|e| e.to_string())?;
        let ft = md.file_type();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            fsync_tree(&path)?;
        } else {
            fsync_file(&path).map_err(|e| e.to_string())?;
        }
    }
    fsync_dir(root).map_err(|e| e.to_string())
}

fn fsync_file(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

// Fsyncs a directory, persisting its dirent list so a renamed name survives power
// loss.
pub(crate) fn fsync_dir(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

// Removes `src` after a completed cross-filesystem move, unlinking a symlink
// rather than following it.
pub(crate) fn remove_source_entry(src: &Path) -> io::Result<()> {
    let md = src.symlink_metadata()?;
    let ft = md.file_type();
    if ft.is_symlink() || !ft.is_dir() {
        std::fs::remove_file(src)
    } else {
        std::fs::remove_dir_all(src)
    }
}

// Removes a staging or backup residue and reports what remains.
//
// Returns an empty string when the path is gone, otherwise a `" [...]"`-prefixed
// sentence naming the surviving path for the caller's error message.
pub(crate) fn remove_stage_residue_report(path: &Path) -> String {
    match remove_stage_residue_fallible(path) {
        Ok(()) => String::new(),
        Err(e) => crate::i18n::format(
            "files-cleanup-incomplete-residue-at-could-not-abac9d1d",
            &[
                ("arg0", (path.display()).to_string()),
                ("e", (e).to_string()),
            ],
        ),
    }
}

// Removes `path` without following symlinks; a missing path is `Ok`.
pub(crate) fn remove_stage_residue_fallible(path: &Path) -> io::Result<()> {
    let md = match path.symlink_metadata() {
        Ok(md) => md,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let ft = md.file_type();
    if ft.is_symlink() || !ft.is_dir() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    }
}

// Copies the contents of `src` into `dst`, which the caller created with
// `create_dir`.
//
// Owns every nested create, so a symlink race after `dst` exists still lands each
// child under the caller's name. FIFOs, sockets, devices and entry read errors
// abort before `stage_cross_fs_replace` replaces the destination.
pub(crate) fn copy_tree_into(
    src: &Path,
    dst: &Path,
    entry_copied: &mut dyn FnMut(),
) -> Result<(), String> {
    let rd = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for dent in rd {
        let dent = dent.map_err(|e| {
            crate::i18n::format(
                "files-aborting-staging-of-could-not-read-fdc1d8c4",
                &[
                    ("arg0", (src.display()).to_string()),
                    ("e", (e).to_string()),
                ],
            )
        })?;
        let child_src = dent.path();
        let child_dst = dst.join(dent.file_name());
        let md = child_src.symlink_metadata().map_err(|e| e.to_string())?;
        let ft = md.file_type();
        if ft.is_symlink() {
            let target = std::fs::read_link(&child_src).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(&target, &child_dst).map_err(|e| e.to_string())?;
        } else if ft.is_dir() {
            create_private_dir(&child_dst).map_err(|e| e.to_string())?;
            copy_tree_into(&child_src, &child_dst, entry_copied)?;
            finish_copied_dir(&md, &child_dst).map_err(|e| e.to_string())?;
        } else if ft.is_file() {
            copy_file_preserving(&child_src, &child_dst).map_err(|e| e.to_string())?;
        } else {
            return Err(crate::i18n::format(
                "files-cannot-stage-unsupported-nested-file-type-0219b50d",
                &[
                    ("arg0", (child_src.display()).to_string()),
                    ("arg1", (describe_special_type(ft)).to_owned()),
                ],
            ));
        }
        entry_copied();
    }
    Ok(())
}

// The permission bits a copy keeps: everything but setuid and setgid, which a
// copy must not hand to whoever runs it next.
fn copied_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o1777
}

fn copied_times(metadata: &std::fs::Metadata) -> std::fs::FileTimes {
    let mut times = std::fs::FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    times
}

// Copies a regular file to a new `dst`, keeping its permission bits and
// access/modification times. The file is created with the source's mode, so
// a private file is never readable by others mid-copy, and the data is
// synced before returning.
pub(crate) fn copy_file_preserving(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut src_file = std::fs::File::open(src)?;
    let metadata = src_file.metadata()?;
    let mode = copied_mode(&metadata);
    let mut dst_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode & 0o777)
        .open(dst)?;
    std::io::copy(&mut src_file, &mut dst_file)?;
    // The umask narrowed the creation mode; set the exact bits.
    dst_file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    dst_file.set_times(copied_times(&metadata))?;
    dst_file.sync_all()
}

// A folder being filled by a copy: private to its owner until
// [`finish_copied_dir`] gives it the source's bits.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

// Gives a copied folder its source's times and permission bits, after its
// contents are in place (adding entries would move the times, and a
// read-only source mode would refuse them).
pub(crate) fn finish_copied_dir(source: &std::fs::Metadata, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::File::open(dst)?.set_times(copied_times(source))?;
    std::fs::set_permissions(dst, std::fs::Permissions::from_mode(copied_mode(source)))
}

// A per-process counter in `staging_name` and `backup_name`, so concurrent moves
// to one destination pick distinct siblings.
fn next_stage_counter() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// Reserves an unused sibling name for `dst` with the given marker, retrying
// [`STAGE_RESERVE_RETRIES`] times.
//
// The check is racy; callers consume the name immediately with no-replace
// `rename(2)`, and cross-fs staging uses `stage_source_exclusive` instead.
pub(crate) fn reserve_unused_sibling(dst: &Path, marker: &str) -> io::Result<PathBuf> {
    for _ in 0..STAGE_RESERVE_RETRIES {
        let candidate = marker_sibling(dst, marker)?;
        if candidate.symlink_metadata().is_err() {
            return Ok(candidate);
        }
    }
    Err(io::Error::other(crate::i18n::format(
        "files-could-not-reserve-a-unique-marker-7a0accf4",
        &[
            ("marker", (marker).to_owned()),
            ("arg0", (dst.display()).to_string()),
            ("arg1", (STAGE_RESERVE_RETRIES).to_string()),
        ],
    )))
}

pub(crate) fn marker_sibling(dst: &Path, marker: &str) -> io::Result<PathBuf> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::format(
                "files-cannot-stage-has-no-parent-a4d7719b",
                &[("arg0", (dst.display()).to_string())],
            ),
        )
    })?;
    let name = dst.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::format(
                "files-cannot-stage-has-no-filename-d7b99f94",
                &[("arg0", (dst.display()).to_string())],
            ),
        )
    })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut sib = std::ffi::OsString::from(".");
    sib.push(name);
    sib.push(".");
    sib.push(marker);
    sib.push(".");
    sib.push(std::process::id().to_string());
    sib.push(".");
    sib.push(nanos.to_string());
    sib.push(".");
    sib.push(next_stage_counter().to_string());
    Ok(parent.join(sib))
}

// Splits a paste of `srcs` into `dest_dir` into the part that needs no decision
// and the part that does.
//
// Returns the whole plan (see [`crate::conflict::PastePlan`]). A Cut into its own
// folder is a no-op; a Copy into it plans the first free numbered sibling, such as
// `report (2).txt`.
pub(crate) fn plan_paste(
    srcs: &[PathBuf],
    dest_dir: &Path,
    mode: crate::conflict::ClipMode,
) -> crate::conflict::PastePlan {
    let mut plan = crate::conflict::PastePlan::default();
    let mut reserved = std::collections::BTreeSet::new();
    for src in srcs {
        let Some(name) = src.file_name() else {
            continue;
        };
        let mut dst = dest_dir.join(name);
        let is_dir = crate::conflict::is_real_dir(src);
        if is_dir && is_within(dest_dir, src) {
            // Copying would re-enter its own output until the path is too long;
            // moving cannot work at all.
            plan.refused.push((
                src.clone(),
                dst,
                crate::i18n::tr("Cannot paste a folder into itself").to_owned(),
            ));
            continue;
        }
        if *src == dst {
            // Names are reserved across the batch so two planned copies cannot pick
            // the same not-yet-created destination.
            if mode == crate::conflict::ClipMode::Cut {
                continue;
            }
            dst = unique_copy_destination(src, dest_dir, &mut reserved);
        }
        if reserved.contains(&dst) {
            // Another item in this batch already takes the name (two
            // `report.txt` from different folders): keep both.
            dst = unique_copy_destination(src, dest_dir, &mut reserved);
        }
        if dst.symlink_metadata().is_ok() {
            plan.conflicts.push(crate::conflict::Conflict {
                kind: crate::conflict::classify(src, &dst),
                dst_detail: describe_for_conflict(&dst),
                src_detail: describe_for_conflict(src),
                src: src.clone(),
                dst,
            });
        } else {
            reserved.insert(dst.clone());
            if is_dir && mode == crate::conflict::ClipMode::Copy {
                plan.copy_folders.insert(dst.clone());
            }
            plan.clear.push((src.clone(), dst));
        }
    }
    plan
}

// Chooses the first free sibling name for a same-folder copy.
//
// `report.txt` becomes `report (2).txt`; a directory named `report.txt` becomes
// `report.txt (2)`. `reserved` holds names already chosen in this pass.
// Whether `path` is `folder` or inside it, by canonical identity when both
// resolve and lexically otherwise.
fn is_within(path: &Path, folder: &Path) -> bool {
    match (std::fs::canonicalize(path), std::fs::canonicalize(folder)) {
        (Ok(path), Ok(folder)) => path.starts_with(folder),
        _ => path.starts_with(folder),
    }
}

fn unique_copy_destination(
    src: &Path,
    dest_dir: &Path,
    reserved: &mut std::collections::BTreeSet<PathBuf>,
) -> PathBuf {
    let name = src.file_name().map_or_else(
        || "Copy".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let is_dir = src
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir());
    let stem = if is_dir {
        name.as_str().into()
    } else {
        src.file_stem()
            .map_or_else(|| name.as_str().into(), |stem| stem.to_string_lossy())
    };
    let extension = (!is_dir)
        .then(|| src.extension().map(|extension| extension.to_string_lossy()))
        .flatten();

    (2..)
        .map(|index| {
            let candidate = match extension.as_deref() {
                Some(extension) if !extension.is_empty() => {
                    format!("{stem} ({index}).{extension}")
                }
                _ => format!("{stem} ({index})"),
            };
            dest_dir.join(candidate)
        })
        .find(|candidate| candidate.symlink_metadata().is_err() && !reserved.contains(candidate))
        .expect("an unbounded numeric suffix must eventually be free")
}

// Plans restoring one Trash entry to the full original path in `.trashinfo`.
//
// A missing original parent is refused, not recreated. An occupied destination
// becomes the same typed conflict paste uses.
pub(crate) fn plan_trash_restore(
    entry: &crate::trash::TrashEntry,
) -> Result<crate::conflict::PastePlan, String> {
    if entry.original_path == entry.trashed_path {
        return Err(crate::i18n::format(
            "files-could-not-restore-the-original-location-76a94d1f",
            &[("arg0", (entry.name).clone())],
        ));
    }
    let Some(parent) = entry.original_path.parent() else {
        return Err(crate::i18n::format(
            "files-could-not-restore-the-original-folder-76538c0c",
            &[("arg0", (entry.name).clone())],
        ));
    };
    if !parent.is_dir() {
        return Err(crate::i18n::format(
            "files-could-not-restore-the-original-folder-76538c0c",
            &[("arg0", (entry.name).clone())],
        ));
    }
    if !entry.trashed_path.exists() {
        return Err(crate::i18n::format(
            "files-could-not-restore-the-trash-entry-1368df06",
            &[("arg0", (entry.name).clone())],
        ));
    }

    let mut plan = crate::conflict::PastePlan::default();
    if entry.original_path.exists() {
        plan.conflicts.push(crate::conflict::Conflict {
            kind: crate::conflict::classify(&entry.trashed_path, &entry.original_path),
            dst_detail: describe_for_conflict(&entry.original_path),
            src_detail: describe_for_conflict(&entry.trashed_path),
            src: entry.trashed_path.clone(),
            dst: entry.original_path.clone(),
        });
    } else {
        plan.clear
            .push((entry.trashed_path.clone(), entry.original_path.clone()));
    }
    Ok(plan)
}

// A short `size · modified` line for one side of a conflict comparison.
fn describe_for_conflict(path: &Path) -> String {
    let Ok(md) = path.symlink_metadata() else {
        return crate::i18n::tr("unknown").to_owned();
    };
    let size = if md.is_dir() {
        crate::i18n::tr("Folder").to_owned()
    } else {
        format_bytes(md.len())
    };
    match md.modified() {
        Ok(t) => crate::i18n::format(
            "files-size-modified",
            &[("size", size), ("modified", format_time(t))],
        ),
        Err(_) => size,
    }
}

// Copies `src` over `dst`, leaving `src` intact (`ConflictChoice::Replace`).
//
// The old destination stays intact until the replacement is staged, fsynced and
// published, so a failure never leaves a truncated final path. The source is
// never removed, whatever the [`PublishOutcome`].
pub(crate) fn copy_replacing(src: &Path, dst: &Path) -> Result<(), String> {
    copy_replacing_with(src, dst, MoveSeams::PRODUCTION)
}

// Copy preserves source bytes for every publication outcome, including retained-backup and durability failures.
pub(crate) fn copy_replacing_with(src: &Path, dst: &Path, seams: MoveSeams) -> Result<(), String> {
    copy_replacing_with_progress(src, dst, seams, &mut || {})
}

fn copy_replacing_with_progress(
    src: &Path,
    dst: &Path,
    seams: MoveSeams,
    entry_copied: &mut dyn FnMut(),
) -> Result<(), String> {
    let staged = stage_source_and_flush_with_progress(src, dst, &seams, entry_copied)?;
    match publish_with_backup(&staged, dst, &seams) {
        PublishOutcome::Published => Ok(()),
        PublishOutcome::PublishedBackupRetained {
            backup,
            cleanup_error,
        } => Err(crate::i18n::format(
            "files-copied-to-but-backup-residue-at-878ad30f",
            &[
                ("arg0", (dst.display()).to_string()),
                ("arg1", (backup.display()).to_string()),
                ("cleanup_error", (cleanup_error)),
                ("arg2", (src.display()).to_string()),
            ],
        )),
        PublishOutcome::PublishedUncertain {
            backup,
            fsync_error,
        } => Err(match backup {
            Some(b) => crate::i18n::format(
                "files-copied-bytes-to-but-durability-of-446f54af",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("fsync_error", (fsync_error)),
                    ("arg1", (b.display()).to_string()),
                    ("arg2", (src.display()).to_string()),
                ],
            ),
            None => crate::i18n::format(
                "files-copied-bytes-to-but-durability-of-4bccf563",
                &[
                    ("arg0", (dst.display()).to_string()),
                    ("fsync_error", (fsync_error)),
                    ("arg1", (src.display()).to_string()),
                ],
            ),
        }),
        PublishOutcome::NotPublished { publish_error } => {
            // Staged duplicates the intact source, so removing it loses no data.
            let cleanup = remove_stage_residue_report(&staged);
            Err(format!("{publish_error}{cleanup}"))
        }
        PublishOutcome::Catastrophic {
            publish_error,
            rollback_error,
            backup,
        } => {
            let cleanup = remove_stage_residue_report(&staged);
            Err(crate::i18n::format(
                "files-catastrophic-copy-publish-for-publish-failed-f66070c2",
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

// Copies the contents of `src` into `dst`, keeping non-colliding destination
// entries (`ConflictChoice::Merge`); `src` is left intact.
//
// Each colliding leaf goes through [`copy_replacing`], so a failure keeps that
// leaf's old destination. An entry read error aborts before any child publishes.
pub(crate) fn copy_merging(src: &Path, dst: &Path) -> Result<(), String> {
    copy_merging_with(src, dst, MoveSeams::PRODUCTION)
}

// [`copy_merging`] with injectable [`MoveSeams`].
pub(crate) fn copy_merging_with(src: &Path, dst: &Path, seams: MoveSeams) -> Result<(), String> {
    use crate::conflict::is_real_dir;
    if !is_real_dir(src) || !is_real_dir(dst) {
        return copy_replacing_with(src, dst, seams);
    }
    let rd = std::fs::read_dir(src).map_err(|e| e.to_string())?;
    for dent in rd {
        let dent = dent.map_err(|e| {
            crate::i18n::format(
                "files-aborting-copy-merge-of-into-could-da1743d7",
                &[
                    ("arg0", (src.display()).to_string()),
                    ("arg1", (dst.display()).to_string()),
                    ("e", (e).to_string()),
                ],
            )
        })?;
        let child_src = dent.path();
        let child_dst = dst.join(dent.file_name());
        if is_real_dir(&child_src) && is_real_dir(&child_dst) {
            copy_merging_with(&child_src, &child_dst, seams)?;
        } else {
            copy_replacing_with(&child_src, &child_dst, seams)?;
        }
    }
    Ok(())
}

// Runs one conflict decision against the filesystem.
//
// A `Cut` moves and a `Copy` writes byte copies; `Skip` never arrives here.
//
// [`ClipMode`]: crate::conflict::ClipMode
#[cfg(test)]
pub(crate) fn perform_conflict_choice(
    src: &Path,
    dst: &Path,
    choice: crate::conflict::ConflictChoice,
    mode: crate::conflict::ClipMode,
) -> crate::conflict::ItemOutcome {
    perform_conflict_choice_with_progress(src, dst, choice, mode, &mut || {})
}

pub(crate) fn perform_conflict_choice_with_progress(
    src: &Path,
    dst: &Path,
    choice: crate::conflict::ConflictChoice,
    mode: crate::conflict::ClipMode,
    entry_copied: &mut dyn FnMut(),
) -> crate::conflict::ItemOutcome {
    use crate::conflict::{ClipMode, ConflictChoice, ItemOutcome};
    match (mode, choice) {
        (_, ConflictChoice::Skip) => ItemOutcome::Skipped,
        (ClipMode::Cut, ConflictChoice::Merge) => match move_merging(src, dst) {
            Ok(()) => ItemOutcome::Merged,
            Err(e) => ItemOutcome::Failed(e),
        },
        (ClipMode::Cut, ConflictChoice::Replace) => match move_replacing(src, dst) {
            Ok(()) => ItemOutcome::Replaced,
            Err(e) => ItemOutcome::Failed(e),
        },
        (ClipMode::Copy, ConflictChoice::Merge) => match copy_merging(src, dst) {
            Ok(()) => ItemOutcome::Merged,
            Err(e) => ItemOutcome::Failed(e),
        },
        (ClipMode::Copy, ConflictChoice::Replace) => match copy_replacing(src, dst) {
            Ok(()) => ItemOutcome::Replaced,
            Err(e) => ItemOutcome::Failed(e),
        },
        (ClipMode::Cut, ConflictChoice::Place) => {
            match super::stage::move_replacing_with(src, dst, MoveSeams::PLACE) {
                Ok(()) => ItemOutcome::Moved,
                Err(e) => ItemOutcome::Failed(e),
            }
        }
        (ClipMode::Copy, ConflictChoice::Place) => {
            match copy_replacing_with_progress(src, dst, MoveSeams::PLACE, entry_copied) {
                Ok(()) => ItemOutcome::Copied,
                Err(e) => ItemOutcome::Failed(e),
            }
        }
    }
}
