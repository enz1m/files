// Trash listing: entries, trashinfo scan and admission, deletion.

use super::{PermanentDeleteOutcome, permanent_delete_batch_descriptor_bound, permanently_delete};
use std::fs;
use std::path::{Path, PathBuf};

// One entry currently in the home Trash.
//
// Destructive operations act through the entry's own `retained_root`, so in a
// mixed listing mount-local rows stay operable while home rows refuse; `None` is
// always refused. No `PartialEq`: the retained `Arc<SecureRoot>` has no
// meaningful value equality.
#[derive(Debug, Clone)]
pub(crate) struct TrashEntry {
    // The moved payload, `<trash>/files/NAME`.
    pub(crate) trashed_path: PathBuf,
    // The `.trashinfo` twin, `<trash>/info/NAME.trashinfo`, deleted with the payload.
    pub(crate) info_path: PathBuf,
    // The decoded `Path=` origin, or the trashed path when the info file is missing
    // or malformed.
    pub(crate) original_path: PathBuf,
    // The origin's file name, so `notes.txt.1` in the trash reads as `notes.txt`.
    pub(crate) name: String,
    // The `DeletionDate=` line verbatim, or empty when unknown.
    pub(crate) deletion_date: String,
    // The authority the entry was admitted under; `None` entries are refused by the
    // reducer.
    pub(crate) retained_root: Option<crate::private_data::TrashRootAuthority>,
    // The payload's type, worked out off the UI thread (by name, then by
    // content); empty for a folder.
    pub(crate) mime: String,
}

// One top-level entry captured for a permanent delete of an ordinary selection.
//
// `name` and `is_dir` are snapshotted from the loaded listing when confirmation
// opens; deletion uses only [`path`](Self::path) and never walks the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeleteItem {
    // The selected top-level path; the sole input to deletion.
    pub(crate) path: PathBuf,
    // The lossy-decoded display name, fixed for the open dialog.
    pub(crate) name: String,
    // Whether the entry was a directory when captured; wording only.
    pub(crate) is_dir: bool,
}

// The target captured while the permanent-delete confirmation is up.
//
// Trash deletes are distinct so they also remove the `.trashinfo` twin.
#[derive(Debug, Clone)]
pub(crate) enum DeleteRequest {
    // Files selected in an ordinary directory (`Shift+Delete`).
    Files(Vec<DeleteItem>),
    // One Trash row, or every row for Empty Trash.
    Trash {
        entries: Vec<TrashEntry>,
        empty_all: bool,
    },
}

// Decodes every `%XX` in a URL-escaped `Path=` value back to the original bytes.
//
// Other file managers escape more than `%`, so decoding is not limited to
// [`super::percent_encode_path`]'s output.
fn percent_decode_path(s: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(s).collect();
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

// Maximum retained entries per Trash listing.
pub(crate) const MAX_RETAINED_TRASH_ENTRIES: usize = 4096;

// Maximum directory candidates visited per Trash listing, independent of the
// retained cap so refused candidates cannot extend traversal.
pub(crate) const MAX_CANDIDATES_SCANNED: usize = 8 * 1024;

// Byte cap for the `.trashinfo` twin scanned during listing.
pub(crate) const MAX_LISTED_TRASHINFO_BYTES: usize = 8 * 1024;

// Byte cap for each admitted `Path=` or `DeletionDate=` value.
pub(crate) const MAX_TRASHINFO_FIELD_BYTES: usize = 4096;

// Bounded, path-free outcome of a home-Trash listing scan.
//
// Not `PartialEq`, because [`TrashEntry`] is not.
#[derive(Debug, Clone, Default)]
pub(crate) struct TrashListing {
    // Retained entries, by `deletion_date` then `trashed_path`, both descending.
    // Truncation may precede sorting; see the cap flags.
    pub(crate) entries: Vec<TrashEntry>,
    // The `<trash>/files` directory itself was not readable.
    pub(crate) unavailable: bool,
    // At least one `read_dir` iterator step returned an error.
    pub(crate) iterator_failed: bool,
    // Retention stopped at [`MAX_RETAINED_TRASH_ENTRIES`].
    pub(crate) entry_cap_reached: bool,
    // Traversal stopped at [`MAX_CANDIDATES_SCANNED`].
    pub(crate) candidate_cap_reached: bool,
    // Count of refused `.trashinfo` twins (symlink, non-regular, over cap, altered,
    // malformed). The payload row stays with a fallback name; refused bytes are
    // never modified.
    pub(crate) refused_metadata: usize,
    // Each readable retained payload's kind and size, by trashed path.
    pub(crate) payloads: std::collections::HashMap<PathBuf, TrashPayload>,
}

// What a trashed payload is, for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TrashPayload {
    pub(crate) is_dir: bool,
    // A file's size in bytes; `None` for a folder.
    pub(crate) size: Option<u64>,
}

// Reads each listed payload's kind for display; unreadable payloads are omitted.
fn read_payloads(listing: &mut TrashListing) {
    for entry in &listing.entries {
        let Ok(meta) = std::fs::symlink_metadata(&entry.trashed_path) else {
            continue;
        };
        let payload = if meta.is_dir() {
            TrashPayload {
                is_dir: true,
                size: None,
            }
        } else {
            TrashPayload {
                is_dir: false,
                size: Some(meta.len()),
            }
        };
        listing.payloads.insert(entry.trashed_path.clone(), payload);
    }
}

// Parsed fields from one admitted `.trashinfo` twin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrashInfoFields {
    pub(crate) original_path: PathBuf,
    pub(crate) deletion_date: String,
}

// Why a `.trashinfo` scan refused to admit fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrashInfoScanError {
    // Path is not a regular file (symlink, FIFO, device, dir, socket).
    NotRegular,
    // Descriptor length exceeds [`MAX_LISTED_TRASHINFO_BYTES`].
    OverCap,
    // Body observed longer or shorter than the admitted descriptor length,
    // or the pathname no longer names the held descriptor.
    Altered,
    // A required field is missing or a value exceeds
    // [`MAX_TRASHINFO_FIELD_BYTES`].
    Malformed,
    // Nofollow open, `fstat`, or `read` failed.
    Unavailable,
}

// Bounded nofollow, nonblocking `.trashinfo` scan.
//
// The length is taken from the descriptor before allocating; a body of another
// length is `Altered`, and the pathname is reverified against the descriptor
// after the read so a replacement cannot publish foreign bytes.
pub(crate) fn scan_admitted_trashinfo(
    info_path: &Path,
) -> Result<TrashInfoFields, TrashInfoScanError> {
    use enzui_security::platform;
    use std::os::unix::fs::OpenOptionsExt;
    // `O_NONBLOCK` so a FIFO cannot block `open(2)` before the kind check.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | platform::libc_o_nonblock())
        .open(info_path)
        .map_err(|_| TrashInfoScanError::Unavailable)?;
    scan_open_trashinfo(&file, Some(info_path))
}

// Descriptor-relative variant: opens `info_name` under the retained info-root fd
// with the same caps and checks, without pathname re-resolution.
pub(crate) fn scan_admitted_trashinfo_at(
    info_root: &enzui_security::secure_dir::SecureRoot,
    info_name: &std::ffi::OsStr,
) -> Result<TrashInfoFields, TrashInfoScanError> {
    let file = info_root
        .open_entry_read(info_name)
        .map_err(|_| TrashInfoScanError::Unavailable)?;
    scan_open_trashinfo(&file, None)
}

// `verify_path` is `Some` for the pathname entry, which must catch a same-length
// replacement; `openat` never re-resolves, so the descriptor entry passes `None`.
fn scan_open_trashinfo(
    file: &std::fs::File,
    verify_path: Option<&Path>,
) -> Result<TrashInfoFields, TrashInfoScanError> {
    use enzui_security::tempstage as sec_tempstage;
    use std::io::Read;
    let ident =
        crate::journal::InodeIdent::from_file(file).map_err(|_| TrashInfoScanError::Unavailable)?;
    if ident.kind != crate::journal::FileKind::Regular {
        return Err(TrashInfoScanError::NotRegular);
    }
    let admitted_len = file
        .metadata()
        .map_err(|_| TrashInfoScanError::Unavailable)?
        .len();
    if admitted_len > MAX_LISTED_TRASHINFO_BYTES as u64 {
        return Err(TrashInfoScanError::OverCap);
    }
    let admitted_usize = admitted_len as usize;
    if let Some(info_path) = verify_path {
        #[cfg(test)]
        if let Some(action) = scan_fault::take_after_fstat() {
            action(info_path);
        }
        #[cfg(not(test))]
        let _ = info_path;
    }
    let mut bytes = Vec::with_capacity(admitted_usize);
    file.take(admitted_len.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| TrashInfoScanError::Unavailable)?;
    if bytes.len() != admitted_usize {
        return Err(TrashInfoScanError::Altered);
    }
    if let Some(info_path) = verify_path
        && sec_tempstage::verify_path_names_file(file, info_path).is_err()
    {
        return Err(TrashInfoScanError::Altered);
    }

    let mut path_value: Option<String> = None;
    let mut date_value: Option<String> = None;
    for raw_line in bytes.split(|b| *b == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if path_value.is_none()
            && let Some(rest) = line.strip_prefix(b"Path=")
        {
            if rest.len() > MAX_TRASHINFO_FIELD_BYTES {
                return Err(TrashInfoScanError::Malformed);
            }
            path_value = Some(String::from_utf8_lossy(rest).into_owned());
        }
        if date_value.is_none()
            && let Some(rest) = line.strip_prefix(b"DeletionDate=")
        {
            if rest.len() > MAX_TRASHINFO_FIELD_BYTES {
                return Err(TrashInfoScanError::Malformed);
            }
            date_value = Some(String::from_utf8_lossy(rest).into_owned());
        }
        if path_value.is_some() && date_value.is_some() {
            break;
        }
    }
    drop(bytes);
    let path_str = path_value.ok_or(TrashInfoScanError::Malformed)?;
    let deletion_date = date_value.unwrap_or_default();
    Ok(TrashInfoFields {
        original_path: percent_decode_path(&path_str),
        deletion_date,
    })
}

// Lists the home Trash under production caps.
//
// Entries carry no authority; the reducer uses
// [`list_trash_admitted_with_authority`] instead.
#[must_use]
pub(crate) fn list_trash_admitted(home_data_dir: &Path) -> TrashListing {
    list_trash_admitted_with_caps_and_root(
        home_data_dir,
        MAX_RETAINED_TRASH_ENTRIES,
        MAX_CANDIDATES_SCANNED,
        None,
    )
}

// Lists the home Trash with every entry carrying `retained_root`.
//
// Later operations go through `SecureRoot` at the pinned inode, closing the
// pathname substitution race after dispatch.
#[must_use]
pub(crate) fn list_trash_admitted_with_authority(
    home_data_dir: &Path,
    authority: crate::private_data::TrashRootAuthority,
) -> TrashListing {
    list_trash_admitted_with_caps_and_root(
        home_data_dir,
        MAX_RETAINED_TRASH_ENTRIES,
        MAX_CANDIDATES_SCANNED,
        Some(authority),
    )
}

// Caller-supplied caps let tests reach the ceilings without thousands of files.

fn list_trash_admitted_with_caps_and_root(
    home_data_dir: &Path,
    entry_cap: usize,
    candidate_cap: usize,
    retained_root: Option<crate::private_data::TrashRootAuthority>,
) -> TrashListing {
    let trash = home_data_dir.join("Trash");
    let files = trash.join("files");
    let info = trash.join("info");
    let mut listing = TrashListing::default();
    // With an authority, enumeration and info reads use the retained fds only.
    if let Some(authority) = retained_root.as_ref() {
        return list_trash_admitted_descriptor_relative(
            authority,
            &files,
            &info,
            entry_cap,
            candidate_cap,
        );
    }
    let Ok(dir) = fs::read_dir(&files) else {
        listing.unavailable = true;
        return listing;
    };
    let mut candidates_scanned: usize = 0;
    for entry in dir {
        if candidates_scanned >= candidate_cap {
            listing.candidate_cap_reached = true;
            break;
        }
        candidates_scanned = candidates_scanned.saturating_add(1);
        let Ok(dent) = entry else {
            listing.iterator_failed = true;
            continue;
        };
        if listing.entries.len() >= entry_cap {
            listing.entry_cap_reached = true;
            break;
        }
        let trashed_path = dent.path();
        let Some(stem) = trashed_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
        else {
            continue;
        };
        let info_path = info.join(format!("{stem}.trashinfo"));
        match scan_admitted_trashinfo(&info_path) {
            Ok(fields) => {
                let name = fields
                    .original_path
                    .file_name()
                    .map_or_else(|| stem.clone(), |s| s.to_string_lossy().into_owned());
                listing.entries.push(TrashEntry {
                    trashed_path,
                    info_path,
                    original_path: fields.original_path,
                    name,
                    deletion_date: fields.deletion_date,
                    retained_root: retained_root.clone(),
                    mime: String::new(),
                });
            }
            Err(_) => {
                listing.refused_metadata = listing.refused_metadata.saturating_add(1);
                listing.entries.push(TrashEntry {
                    trashed_path: trashed_path.clone(),
                    info_path,
                    original_path: trashed_path,
                    name: stem,
                    deletion_date: String::new(),
                    retained_root: retained_root.clone(),
                    mime: String::new(),
                });
            }
        }
    }
    for entry in &mut listing.entries {
        entry.mime = crate::services::file_mime(&entry.trashed_path, &entry.name);
    }
    // Total order: date descending, then trashed path descending.
    listing.entries.sort_by(|a, b| {
        b.deletion_date
            .cmp(&a.deletion_date)
            .then_with(|| b.trashed_path.cmp(&a.trashed_path))
    });
    read_payloads(&mut listing);
    listing
}

// Enumerates via `getdents` on the retained `files/` fd and reads each info twin
// through the retained `info/` fd, so a pathname substitution cannot redirect
// either. The produced path strings are display-only; operations use the fds.
fn list_trash_admitted_descriptor_relative(
    authority: &crate::private_data::TrashRootAuthority,
    files_path_display: &Path,
    info_path_display: &Path,
    entry_cap: usize,
    candidate_cap: usize,
) -> TrashListing {
    let mut listing = TrashListing::default();
    let files_root = authority.files_root();
    let info_root = authority.info_root();
    let Ok(names) = files_root.entry_names() else {
        listing.iterator_failed = true;
        return listing;
    };
    let mut candidates_scanned: usize = 0;
    for stat in names {
        if candidates_scanned >= candidate_cap {
            listing.candidate_cap_reached = true;
            break;
        }
        candidates_scanned = candidates_scanned.saturating_add(1);
        if listing.entries.len() >= entry_cap {
            listing.entry_cap_reached = true;
            break;
        }
        let stem_os = stat.name.clone();
        let stem = stem_os.to_string_lossy().into_owned();
        let info_name = std::ffi::OsString::from(format!("{stem}.trashinfo"));
        let trashed_path = files_path_display.join(&stem_os);
        let info_path = info_path_display.join(&info_name);
        match scan_admitted_trashinfo_at(&info_root, &info_name) {
            Ok(fields) => {
                let name = fields
                    .original_path
                    .file_name()
                    .map_or_else(|| stem.clone(), |s| s.to_string_lossy().into_owned());
                listing.entries.push(TrashEntry {
                    trashed_path,
                    info_path,
                    original_path: fields.original_path,
                    name,
                    deletion_date: fields.deletion_date,
                    retained_root: Some(authority.clone()),
                    mime: String::new(),
                });
            }
            Err(_) => {
                listing.refused_metadata = listing.refused_metadata.saturating_add(1);
                listing.entries.push(TrashEntry {
                    trashed_path: trashed_path.clone(),
                    info_path,
                    original_path: trashed_path,
                    name: stem,
                    deletion_date: String::new(),
                    retained_root: Some(authority.clone()),
                    mime: String::new(),
                });
            }
        }
    }
    for entry in &mut listing.entries {
        entry.mime = crate::services::file_mime(&entry.trashed_path, &entry.name);
    }
    listing.entries.sort_by(|a, b| {
        b.deletion_date
            .cmp(&a.deletion_date)
            .then_with(|| b.trashed_path.cmp(&a.trashed_path))
    });
    read_payloads(&mut listing);
    listing
}

// The admitted entries for delete-flow and test callers.
//
// Tries descriptor-bound admission of `<home_data_dir>/Trash`; a hostile root
// (wrong owner, group/world writable, symlinked leaf) falls back to the pathname
// scan, which reports the admission error.
#[must_use]
pub(crate) fn list_trash(home_data_dir: &Path) -> Vec<TrashEntry> {
    if let Some(authority) = try_admit_home_trash(home_data_dir) {
        return list_trash_admitted_with_authority(home_data_dir, authority).entries;
    }
    list_trash_admitted(home_data_dir).entries
}

// Best-effort; never mutates process state, and failure yields `None`.
pub(super) fn try_admit_home_trash(
    home_data_dir: &Path,
) -> Option<crate::private_data::TrashRootAuthority> {
    let trash_path = home_data_dir.join("Trash");
    let trash = enzui_security::secure_dir::SecureRoot::open(&trash_path).ok()?;
    let handles =
        crate::private_data::TrashRootHandles::from_trash(std::sync::Arc::new(trash)).ok()?;
    Some(crate::private_data::TrashRootAuthority::Home(handles))
}

// Permanently deletes captured Trash entries and their `.trashinfo` twins.
//
// Called only after the confirmation resolves. Entries with `retained_root` use
// [`permanent_delete_batch_descriptor_bound`] (`unlinkat`) so a substituted
// `<trash>` cannot redirect; entries without it use pathname unlink.
pub(crate) fn delete_trash_entries(entries: &[TrashEntry]) -> PermanentDeleteOutcome {
    let (authorized, legacy): (Vec<_>, Vec<_>) = entries
        .iter()
        .cloned()
        .partition(|entry| entry.retained_root.is_some());

    let mut out = permanent_delete_batch_descriptor_bound(&authorized);
    for entry in &legacy {
        match permanently_delete(&entry.trashed_path) {
            Ok(()) => {
                // A failed twin removal does not fail the payload delete.
                let _ = fs::remove_file(&entry.info_path);
                out.deleted.push(entry.trashed_path.clone());
            }
            Err(msg) => out.failed.push((entry.trashed_path.clone(), msg)),
        }
    }
    out
}

// Test seam firing between `fstat` and `read`; empty in product builds.
#[cfg(test)]
pub(crate) mod scan_fault {
    use std::cell::RefCell;
    use std::path::Path;

    type PathAction = Box<dyn FnOnce(&Path)>;

    thread_local! {
        static AFTER_FSTAT: RefCell<Option<PathAction>> = const { RefCell::new(None) };
    }

    pub(super) fn take_after_fstat() -> Option<PathAction> {
        AFTER_FSTAT.with(|s| s.borrow_mut().take())
    }

    pub(crate) fn clear() {
        AFTER_FSTAT.with(|s| *s.borrow_mut() = None);
    }
}
