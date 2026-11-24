// Trash stays on the source filesystem for atomic restore, per the freedesktop Trash specification.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// Which side holds bytes when a trash publication fails after the payload
// crossed into the retained `files/` fd; mirrors
// [`enzui_security::secure_dir::MoveCustody`].
//
// Only [`Self::Source`] is a clean failure: [`Self::Destination`] and
// [`Self::Both`] keep recoverable metadata under `info/`, and
// [`Self::Uncertain`] promises neither side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublishCustody {
    // The payload never left the source parent; the metadata reservation may be retired.
    Source,
    // The payload arrived at the destination; metadata must stay so a
    // visible orphan does not become an invisible one.
    Destination,
    // A cross-device copy published bytes at the destination but the source
    // remove did not succeed; both sides hold recoverable inodes.
    Both,
    // Rename succeeded but the post-move identity check refused; the
    // destination name may or may not name the retained inode.
    Uncertain,
}

// The result of one trash operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrashError {
    // The source does not exist — already gone.
    Missing,
    // The source's mount is read-only, so no `.Trash-$uid` can be created.
    // Copying to the home trash instead would break cross-mount atomicity.
    ReadOnlyMount(PathBuf),
    // The payload move partially completed. The metadata twin is kept when
    // `custody != Source`, so a recovery pass sees the same pair; not `Applied`.
    Pending {
        // Which side still holds bytes.
        custody: PublishCustody,
        // Descriptor-relative twin name preserved under `<trash>/info`
        // when `custody != Source`; `None` when the twin was retired.
        info_name: Option<String>,
        // Original I/O message from the framework, for logs (not user text).
        diagnostic: String,
    },
    // Any other `io::Error`, verbatim.
    Io(String),
}

impl TrashError {
    pub(crate) fn status_line(&self, source: &Path) -> String {
        let name = source.file_name().map_or_else(
            || source.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        match self {
            TrashError::Missing => crate::i18n::format(
                "files-could-not-trash-name-not-found-9b594bc7",
                &[("name", (name))],
            ),
            TrashError::ReadOnlyMount(_) => crate::i18n::format(
                "files-could-not-trash-name-the-volume-9ff203b8",
                &[("name", (name))],
            ),
            TrashError::Pending { custody, .. } => match custody {
                PublishCustody::Source => crate::i18n::format(
                    "files-could-not-trash-name-source-retained-8281491d",
                    &[("name", (name))],
                ),
                PublishCustody::Destination => crate::i18n::format(
                    "files-trashed-name-with-unfinished-durability-metadata-81635ec6",
                    &[("name", (name))],
                ),
                PublishCustody::Both => crate::i18n::format(
                    "files-copy-of-name-landed-but-the-7255ada0",
                    &[("name", (name))],
                ),
                PublishCustody::Uncertain => crate::i18n::format(
                    "files-trashing-name-did-not-settle-cleanly-700b340c",
                    &[("name", (name))],
                ),
            },
            TrashError::Io(e) => crate::i18n::format(
                "files-could-not-trash-name-e-fadaf072",
                &[("name", (name)), ("e", (e).clone())],
            ),
        }
    }
}

// The outcome of a whole trash batch (one entry per source).
#[derive(Debug, Clone, Default)]
pub(crate) struct TrashOutcome {
    // `(original, path-in-trash)` pairs for entries that landed.
    pub(crate) trashed: Vec<(PathBuf, PathBuf)>,
    // `(original, error)` pairs for entries that failed.
    pub(crate) failed: Vec<(PathBuf, TrashError)>,
}

impl TrashOutcome {
    pub(crate) fn summary(&self) -> String {
        let n = self.trashed.len();
        let f = self.failed.len();
        match (n, f) {
            (0, 0) => crate::i18n::tr("Nothing to trash").to_owned(),
            (1, 0) => {
                let name = self.trashed[0]
                    .0
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::i18n::format("files-moved-name-to-trash-4d5d36b2", &[("name", (name))])
            }
            (n, 0) => crate::i18n::format(
                "files-moved-n-items-to-trash-05fe5a66",
                &[("n", (n).to_string())],
            ),
            (0, f) => {
                // A single failure names the file; multiples summarise.
                if f == 1 {
                    self.failed[0].1.status_line(&self.failed[0].0)
                } else {
                    crate::i18n::format(
                        "files-trash-failed-for-f-items-5f8eca8a",
                        &[("f", (f).to_string())],
                    )
                }
            }
            (n, f) => crate::i18n::format(
                "files-moved-n-to-trash-f-failed-a8c2f50f",
                &[("n", (n).to_string()), ("f", (f).to_string())],
            ),
        }
    }
}

// The trash directory that should hold `source`, per the spec.
//
// Returns `$XDG_DATA_HOME/Trash` when `source` shares `home_data_dir`'s
// `st_dev`, otherwise `<mount>/.Trash-<uid>` on the source's mount.
#[allow(unsafe_code, reason = "libc getuid FFI")]
pub(crate) fn pick_trash_root(source: &Path, home_data_dir: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let src_meta = fs::symlink_metadata(source)?;
    // An unavailable home trash must not block a mount-local trash for a
    // source on another device.
    let home_dev = existing_ancestor(home_data_dir)
        .and_then(|home_ref| fs::metadata(&home_ref).ok())
        .map(|home_meta| home_meta.dev());
    if let Some(home_dev) = home_dev
        && src_meta.dev() == home_dev
    {
        return Ok(home_data_dir.join("Trash"));
    }
    let mount = mount_root_of(source, src_meta.dev())?;
    // SAFETY: getuid takes no pointers and has no memory preconditions.
    let uid = unsafe { libc::getuid() };
    Ok(mount.join(format!(".Trash-{uid}")))
}

// The nearest existing ancestor of `p`.
fn existing_ancestor(p: &Path) -> Option<PathBuf> {
    let mut cur = p;
    loop {
        if cur.exists() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(parent) if parent != cur => cur = parent,
            _ => return None,
        }
    }
}

// The mount point of the filesystem `source` is on — the highest ancestor
// whose `st_dev` still equals `source_dev`.
pub(crate) fn mount_root_of(source: &Path, source_dev: u64) -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut cur = if source.is_dir() {
        source.to_path_buf()
    } else {
        source
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
    };
    loop {
        let Some(parent) = cur.parent() else {
            return Ok(cur);
        };
        // `parent` may be a symlinked mount root; use the parent's device.
        match fs::metadata(parent) {
            Ok(m) if m.dev() == source_dev => cur = parent.to_path_buf(),
            Ok(_) => return Ok(cur),
            // A parent we cannot stat is treated as a mount boundary.
            Err(_) => return Ok(cur),
        }
    }
}

// Creates `<trash>/files` and `<trash>/info` with mode `0o700`.
//
// Returns `TrashError::ReadOnlyMount` on `EROFS`.
fn ensure_trash_layout(trash: &Path, source_mount: &Path) -> Result<(), TrashError> {
    use std::os::unix::fs::PermissionsExt;
    for sub in ["files", "info"] {
        let p = trash.join(sub);
        if p.is_dir() {
            continue;
        }
        if let Err(e) = fs::create_dir_all(&p) {
            return Err(match e.raw_os_error() {
                Some(libc::EROFS) => TrashError::ReadOnlyMount(source_mount.to_path_buf()),
                _ => TrashError::Io(e.to_string()),
            });
        }
    }
    // Spec: the `.Trash-$uid` should be private.
    if let Ok(md) = fs::metadata(trash) {
        let mut perms = md.permissions();
        perms.set_mode(0o700);
        let _ = fs::set_permissions(trash, perms);
    }
    Ok(())
}

// Picks a free name for `source` in `trash/files/` (`NAME`, `NAME.1`, ...).
//
// The `.trashinfo` twin is checked too: a mismatched pair would make restore
// act on the wrong origin.
pub(crate) fn disambiguate_in_trash(trash: &Path, base: &str) -> String {
    let files = trash.join("files");
    let info = trash.join("info");
    let taken =
        |name: &str| files.join(name).exists() || info.join(format!("{name}.trashinfo")).exists();
    if !taken(base) {
        return base.to_owned();
    }
    for i in 1.. {
        let name = format!("{base}.{i}");
        if !taken(&name) {
            return name;
        }
        if i > 1_000_000 {
            // Let the subsequent rename fail rather than loop forever.
            return name;
        }
    }
    unreachable!()
}

// Formats a `SystemTime` as the spec's `DeletionDate`, e.g.
// `2024-11-04T13:22:07`, in UTC so it is stable across DST.
fn format_deletion_date(t: SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (year, month, day, hour, min, sec) = civil_from_epoch(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}")
}

// Split epoch-seconds into `(year, month, day, hour, minute, second)` UTC.
fn civil_from_epoch(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let day_secs = 86_400i64;
    let mut days = secs.div_euclid(day_secs);
    let mut rem = secs.rem_euclid(day_secs);
    if rem < 0 {
        rem += day_secs;
        days -= 1;
    }
    let hour = (rem / 3600) as u32;
    let min = ((rem % 3600) / 60) as u32;
    let sec = (rem % 60) as u32;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (y + i64::from(m <= 2)) as i32;
    (year, m, d, hour, min, sec)
}

// Moves one entry to the correct trash, writing its `.trashinfo` first and
// copying then removing the payload on `EXDEV`.
//
// `home_data_dir` lets tests stage a fake home instead of the real trash.
pub(crate) fn move_to_trash_with(
    source: &Path,
    home_data_dir: &Path,
) -> Result<PathBuf, TrashError> {
    use std::os::unix::fs::MetadataExt;

    let src_meta = fs::symlink_metadata(source).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => TrashError::Missing,
        _ => TrashError::Io(e.to_string()),
    })?;

    let trash =
        pick_trash_root(source, home_data_dir).map_err(|e| TrashError::Io(e.to_string()))?;
    let source_mount = mount_root_of(source, src_meta.dev()).unwrap_or_else(|_| PathBuf::from("/"));

    ensure_trash_layout(&trash, &source_mount)?;

    let base_name = source.file_name().map_or_else(
        || "trashed".to_owned(),
        |s| s.to_string_lossy().into_owned(),
    );
    // Only the parent is resolved: canonicalizing the entry itself would
    // follow a trashed symlink and record its target as where it came from.
    let orig_abs = match (source.parent(), source.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map_or_else(|_| source.to_path_buf(), |parent| parent.join(name)),
        _ => source.to_path_buf(),
    };
    let info_body = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode_path(&orig_abs),
        format_deletion_date(SystemTime::now())
    );

    // Descriptor-bound admission reserves the info name with `O_EXCL` and
    // checks the payload through retained fds, so nothing re-resolves a path
    // between checks. Admission failure (hostile mode, symlink, non-directory)
    // falls through to pathname publication.
    let trash_parent = trash.parent().unwrap_or(&trash).to_path_buf();
    if let Some(authority) = try_admit_home_trash(&trash_parent) {
        return publish_trash_move_descriptor_bound(
            &authority,
            &trash,
            source,
            &source_mount,
            &base_name,
            &info_body,
        );
    }

    // Reached only when SecureRoot refuses the trash tree.
    ensure_trash_layout(&trash, &source_mount)?;
    let unique = disambiguate_in_trash(&trash, &base_name);
    let dest_payload = trash.join("files").join(&unique);
    let dest_info = trash.join("info").join(format!("{unique}.trashinfo"));
    if let Err(e) = fs::write(&dest_info, info_body) {
        return Err(TrashError::Io(crate::i18n::format(
            "files-trashinfo-write-failed-e-d96f1203",
            &[("e", (e).to_string())],
        )));
    }
    // This fallback has no admitted source-parent fd, so the payload rename
    // is a pathname operation.
    match fs::rename(source, &dest_payload) {
        Ok(()) => Ok(dest_payload),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let copied_source = match copy_across(source, &dest_payload, &dest_info) {
                Ok(copied) => copied,
                Err(err) => {
                    let _ = fs::remove_file(&dest_info);
                    return Err(TrashError::Io(err));
                }
            };
            // The trashed copy is whole, durable and listed; only the source
            // entries it holds are removed, and anything else is reported.
            match copied_source.remove_from(source) {
                Ok(()) => Ok(dest_payload),
                Err(kept) => Err(TrashError::Io(crate::services::source_kept(
                    &dest_payload,
                    &kept,
                ))),
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&dest_info);
            Err(match e.raw_os_error() {
                Some(libc::EROFS) => TrashError::ReadOnlyMount(source_mount),
                _ => TrashError::Io(e.to_string()),
            })
        }
    }
}

fn publish_trash_move_descriptor_bound(
    authority: &crate::private_data::TrashRootAuthority,
    trash: &Path,
    source: &Path,
    source_mount: &Path,
    base_name: &str,
    info_body: &str,
) -> Result<PathBuf, TrashError> {
    use std::io::Write;
    let files_root = authority.files_root();
    let info_root = authority.info_root();
    let (unique, mut info_file) =
        reserve_payload_and_info_names_descriptor_relative(&files_root, &info_root, base_name)
            .map_err(TrashError::Io)?;
    let info_name = format!("{unique}.trashinfo");
    let dest_payload = trash.join("files").join(&unique);
    if let Err(e) = info_file.write_all(info_body.as_bytes()) {
        drop(info_file);
        let _ = info_root.unlink_entry(std::ffi::OsStr::new(&info_name));
        return Err(TrashError::Io(crate::i18n::format(
            "files-trashinfo-write-failed-e-d96f1203",
            &[("e", (e).to_string())],
        )));
    }
    drop(info_file);

    // Kernel no-clobber `renameat` into the retained `files/` fd. On EXDEV
    // regular files are copied before source removal, with typed custody on
    // partial completion; cross-fs directories, symlinks and special files
    // return `Unsupported` with the source intact.
    match move_payload_descriptor_bound(&files_root, source, &unique, source_mount) {
        Ok(()) => Ok(dest_payload),
        Err(payload_failure) => match payload_failure {
            PayloadMoveFailure::Terminal(err) => {
                // No destination bytes, so the metadata reservation can go.
                let _ = info_root.unlink_entry(std::ffi::OsStr::new(&info_name));
                Err(err)
            }
            PayloadMoveFailure::CustodyPreserved {
                custody,
                diagnostic,
            } => {
                // Bytes may exist at the destination: keep the metadata twin so
                // they are not orphaned, and do not retry by pathname.
                Err(TrashError::Pending {
                    custody,
                    info_name: Some(info_name),
                    diagnostic,
                })
            }
        },
    }
}

// Whether destination bytes may exist after a failed payload move.
// `CustodyPreserved` means the metadata twin must be kept.
enum PayloadMoveFailure {
    Terminal(TrashError),
    CustodyPreserved {
        custody: PublishCustody,
        diagnostic: String,
    },
}

// Moves the payload into the retained `files/` fd with kernel no-clobber,
// syncing both parents on success and returning typed custody on partial
// completion.
fn move_payload_descriptor_bound(
    files_root: &std::sync::Arc<enzui_security::secure_dir::SecureRoot>,
    source: &Path,
    unique: &str,
    source_mount: &Path,
) -> Result<(), PayloadMoveFailure> {
    use enzui_security::secure_dir::{MoveCustody, RetainedDir};

    let Some(source_parent_path) = source.parent() else {
        return Err(PayloadMoveFailure::Terminal(TrashError::Io(
            crate::i18n::tr("source has no parent directory").to_owned(),
        )));
    };
    let Some(source_name_os) = source.file_name() else {
        return Err(PayloadMoveFailure::Terminal(TrashError::Io(
            crate::i18n::tr("source has no basename").to_owned(),
        )));
    };
    let source_parent = RetainedDir::open(source_parent_path).map_err(|e| {
        PayloadMoveFailure::Terminal(TrashError::Io(crate::i18n::format(
            "files-source-parent-retention-failed-e-75ba3148",
            &[("e", (e).to_string())],
        )))
    })?;
    let source_entry = source_parent.retain_entry(source_name_os).map_err(|e| {
        PayloadMoveFailure::Terminal(if e.kind() == io::ErrorKind::NotFound {
            TrashError::Missing
        } else {
            TrashError::Io(crate::i18n::format(
                "files-source-retention-failed-e-b2633d40",
                &[("e", (e).to_string())],
            ))
        })
    })?;
    let files_dest = files_root.ordinary_directory().map_err(|e| {
        PayloadMoveFailure::Terminal(TrashError::Io(crate::i18n::format(
            "files-files-retention-failed-e-8a4e8354",
            &[("e", (e).to_string())],
        )))
    })?;
    let dest_name = std::ffi::OsString::from(unique);
    match source_entry.move_noreplace(&files_dest, &dest_name) {
        Ok(_moved) => Ok(()),
        Err(failure) => {
            let raw = failure.error.raw_os_error();
            let kind = failure.error.kind();
            // No destination bytes: the caller may retire the metadata.
            if matches!(*failure.custody, MoveCustody::Source(_)) {
                return Err(PayloadMoveFailure::Terminal(match raw {
                    Some(libc::EROFS) => TrashError::ReadOnlyMount(source_mount.to_path_buf()),
                    _ if kind == io::ErrorKind::NotFound => TrashError::Missing,
                    _ => TrashError::Io(crate::i18n::format(
                        "files-descriptor-bound-payload-move-failed-custody-27302125",
                        &[("arg0", (failure.error).to_string())],
                    )),
                }));
            }
            let custody = match *failure.custody {
                MoveCustody::Source(_) => PublishCustody::Source,
                MoveCustody::Destination(_) => PublishCustody::Destination,
                MoveCustody::Both { .. } => PublishCustody::Both,
                MoveCustody::Uncertain { .. } => PublishCustody::Uncertain,
            };
            Err(PayloadMoveFailure::CustodyPreserved {
                custody,
                diagnostic: failure.error.to_string(),
            })
        }
    }
}

// The info name is reserved atomically with `O_EXCL`; the payload name is only
// prechecked, so a later race is reported by the caller after unlinking the
// info reservation.
const MAX_DISAMBIGUATION_RETRIES: usize = 1_048_576;

fn reserve_payload_and_info_names_descriptor_relative(
    files_root: &enzui_security::secure_dir::SecureRoot,
    info_root: &enzui_security::secure_dir::SecureRoot,
    base: &str,
) -> Result<(String, std::fs::File), String> {
    for i in 0..=MAX_DISAMBIGUATION_RETRIES {
        let candidate = if i == 0 {
            base.to_owned()
        } else {
            format!("{base}.{i}")
        };
        let info_name = std::ffi::OsString::from(format!("{candidate}.trashinfo"));
        // Preflight payload absence through the retained files-root fd.
        match files_root.entry_identity(std::ffi::OsStr::new(&candidate)) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => continue,
            Err(err) => {
                return Err(crate::i18n::format(
                    "files-trash-files-precheck-failed-err-d5fb5d2b",
                    &[("err", (err).to_string())],
                ));
            }
        }
        match info_root.create_entry_private(&info_name) {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(crate::i18n::format(
                    "files-trashinfo-reservation-failed-err-e3fad048",
                    &[("err", (err).to_string())],
                ));
            }
        }
    }
    Err(crate::i18n::format(
        "files-trash-disambiguation-exhausted-after-max-disambiguation-d342a6cf",
        &[(
            "MAX_DISAMBIGUATION_RETRIES",
            (MAX_DISAMBIGUATION_RETRIES).to_string(),
        )],
    ))
}

// Copies `source` to the new `payload` and makes it and the already written
// `info` durable, leaving `source` untouched. FIFOs, sockets and devices are
// refused, since opening one can block indefinitely.
fn copy_across(
    source: &Path,
    payload: &Path,
    info: &Path,
) -> Result<crate::services::CopiedSources, String> {
    let copied = crate::services::copy_new_for_move(source, payload)?;
    let synced = fs::File::open(info)
        .and_then(|file| file.sync_all())
        .and_then(|()| match info.parent() {
            Some(parent) => fs::File::open(parent)?.sync_all(),
            None => Ok(()),
        });
    if let Err(err) = synced {
        let _ = crate::services::remove_stage_residue_fallible(payload);
        return Err(err.to_string());
    }
    Ok(copied)
}

// URL-escapes the raw path bytes for `Path=`, as the trash spec asks and as
// GLib writes it: a newline cannot end the line early and a name that is not
// UTF-8 keeps its exact bytes. Separators and unreserved characters stay.
fn percent_encode_path(path: &Path) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};
    use std::os::unix::ffi::OsStrExt;
    const PATH_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'/')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::percent_encode(path.as_os_str().as_bytes(), PATH_ESCAPE).to_string()
}

// Permanently removes a file or directory (`Shift+Delete`, behind the
// confirmation dialog).
pub(crate) fn permanently_delete(path: &Path) -> Result<(), String> {
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(crate::i18n::format(
                "files-could-not-delete-e-ae989500",
                &[
                    (
                        "arg0",
                        (path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default()),
                    ),
                    ("e", (e).to_string()),
                ],
            ));
        }
    };
    let outcome = if md.file_type().is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    outcome.map_err(|e| {
        crate::i18n::format(
            "files-could-not-delete-e-ae989500",
            &[
                (
                    "arg0",
                    (path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()),
                ),
                ("e", (e).to_string()),
            ],
        )
    })
}

// The outcome of a whole permanent-delete batch.
#[derive(Debug, Clone, Default)]
pub(crate) struct PermanentDeleteOutcome {
    pub(crate) deleted: Vec<PathBuf>,
    pub(crate) failed: Vec<(PathBuf, String)>,
}

impl PermanentDeleteOutcome {
    pub(crate) fn summary(&self) -> String {
        let n = self.deleted.len();
        let f = self.failed.len();
        match (n, f) {
            (0, 0) => crate::i18n::tr("Nothing to delete").to_owned(),
            (1, 0) => {
                let name = self.deleted[0]
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                crate::i18n::format("files-deleted-name-f92c3f24", &[("name", (name))])
            }
            (n, 0) => {
                crate::i18n::format("files-deleted-n-items-50b43ad0", &[("n", (n).to_string())])
            }
            (0, 1) => self.failed[0].1.clone(),
            (0, f) => crate::i18n::format(
                "files-delete-failed-for-f-items-5664ecf3",
                &[("f", (f).to_string())],
            ),
            (n, f) => crate::i18n::format(
                "files-deleted-n-f-failed-188847e3",
                &[("n", (n).to_string()), ("f", (f).to_string())],
            ),
        }
    }
}

// Trashes a batch sequentially, one outcome entry per source.
pub(crate) fn trash_batch_with(sources: &[PathBuf], home_data_dir: &Path) -> TrashOutcome {
    let mut out = TrashOutcome::default();
    for source in sources {
        match move_to_trash_with(source, home_data_dir) {
            Ok(dest) => out.trashed.push((source.clone(), dest)),
            Err(e) => out.failed.push((source.clone(), e)),
        }
    }
    out
}

pub(crate) fn permanent_delete_batch(sources: &[PathBuf]) -> PermanentDeleteOutcome {
    let mut out = PermanentDeleteOutcome::default();
    for source in sources {
        match permanently_delete(source) {
            Ok(()) => out.deleted.push(source.clone()),
            Err(msg) => out.failed.push((source.clone(), msg)),
        }
    }
    out
}

// Permanently deletes Trash entries through the root fds retained at boot.
//
// Each unlink is `unlinkat` narrowed to the inode identity captured through
// the retained fd. An entry without a retained root, whose root fails
// revalidation, or whose identity changed is refused and left untouched, so
// a path substitution under `<trash>` cannot redirect the delete.
pub(crate) fn permanent_delete_batch_descriptor_bound(
    entries: &[TrashEntry],
) -> PermanentDeleteOutcome {
    let mut out = PermanentDeleteOutcome::default();
    for entry in entries {
        let path = entry.trashed_path.clone();
        let Some(authority) = entry.retained_root.as_ref() else {
            out.failed.push((
                path,
                crate::i18n::tr("Refused: this Trash entry has no admitted root authority")
                    .to_owned(),
            ));
            continue;
        };
        if authority.revalidate().is_err() {
            out.failed.push((
                path,
                crate::i18n::tr("Refused: the Trash root failed revalidation (identity changed)")
                    .to_owned(),
            ));
            continue;
        }
        let files_root = authority.files_root();
        let info_root = authority.info_root();

        // `*_entry` APIs refuse names with separators.
        let Some(payload_name) = entry.trashed_path.file_name() else {
            out.failed.push((
                path,
                crate::i18n::tr("Refused: malformed trashed-path").to_owned(),
            ));
            continue;
        };
        let Some(info_name) = entry.info_path.file_name() else {
            out.failed.push((
                path,
                crate::i18n::tr("Refused: malformed info-path").to_owned(),
            ));
            continue;
        };

        // The unlink is narrowed to this identity, so a later substitution
        // is refused.
        let payload_ident = match files_root.entry_identity(payload_name) {
            Ok(id) => id,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Already gone: still unlink the twin so no orphan info remains.
                let _ = info_root.unlink_entry(info_name);
                out.deleted.push(entry.trashed_path.clone());
                continue;
            }
            Err(err) => {
                out.failed.push((
                    path,
                    crate::i18n::format(
                        "files-refused-cannot-stat-payload-err-2026a6f8",
                        &[("err", (err).to_string())],
                    ),
                ));
                continue;
            }
        };
        match files_root.unlink_entry_if_identity(payload_name, payload_ident) {
            Ok(true) => {
                // Best effort: an orphan info surfaces as `refused_metadata`.
                let _ = info_root.unlink_entry(info_name);
                out.deleted.push(entry.trashed_path.clone());
            }
            Ok(false) => {
                out.failed.push((
                    path,
                    crate::i18n::tr("Refused: the payload name now denotes a different inode")
                        .to_owned(),
                ));
            }
            Err(err) => {
                out.failed.push((
                    path,
                    crate::i18n::format(
                        "files-delete-failed-err-e1bbb76b",
                        &[("err", (err).to_string())],
                    ),
                ));
            }
        }
    }
    out
}

#[path = "trash/listing.rs"]
mod listing;
#[path = "trash/restore.rs"]
mod restore;

#[cfg(test)]
pub(crate) use listing::scan_fault;
use listing::try_admit_home_trash;
pub(crate) use listing::{
    DeleteItem, DeleteRequest, MAX_CANDIDATES_SCANNED, MAX_RETAINED_TRASH_ENTRIES, TrashEntry,
    TrashListing, TrashPayload, delete_trash_entries, list_trash, list_trash_admitted,
    list_trash_admitted_with_authority,
};
pub(crate) use restore::{PublishError, RestoreError, RetireError, restore_without_replacing};
pub(crate) use restore::{
    capture_admitted_metadata_twin, publish_metadata_twin, restore_payload_without_replacing,
    retire_metadata_twin,
};
#[cfg(test)]
pub(crate) use restore::{publish_fault, retire_fault};

#[cfg(test)]
mod tests;
