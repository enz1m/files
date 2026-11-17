// Properties dialog and recursive search.

use super::*;

// Reads the Properties snapshot for `path`.
//
// A directory's `size_text` is an em dash; the reducer runs
// [`recursive_folder_size`] on a background thread.
pub(crate) fn build_properties(path: &Path) -> Result<PropertiesInfo, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let md = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let is_symlink = md.file_type().is_symlink();
    let target_md = if is_symlink {
        std::fs::metadata(path).ok()
    } else {
        None
    };
    let is_dir = md.is_dir() || target_md.as_ref().is_some_and(std::fs::Metadata::is_dir);

    let (kind_label, icon, size_bytes, mime_type) = if is_dir {
        (
            crate::i18n::tr("Folder").to_owned(),
            "folder".to_owned(),
            0,
            "inode/directory".to_owned(),
        )
    } else if is_symlink {
        (
            crate::i18n::tr("Link").to_owned(),
            "link".to_owned(),
            md.len(),
            "inode/symlink".to_owned(),
        )
    } else {
        // The same type the listing shows: by name, then by content.
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mime = super::file_mime(path, &name);
        (
            EntryKind::File.label(&mime),
            "file".to_owned(),
            md.len(),
            mime,
        )
    };

    let link_target = if is_symlink {
        std::fs::read_link(path)
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let size_text = if is_dir {
        "\u{2014}".to_owned()
    } else {
        format_bytes(size_bytes)
    };

    let modified_text = md.modified().ok().map(format_time).unwrap_or_default();
    let accessed_text = md.accessed().ok().map(format_time).unwrap_or_default();

    let owner = lookup_user(md.uid());
    let group = lookup_group(md.gid());

    let content_summary = if is_dir {
        summarize_directory(path)
    } else {
        String::new()
    };

    let name = path.file_name().and_then(|n| n.to_str()).map_or_else(
        || path.display().to_string(),
        std::borrow::ToOwned::to_owned,
    );

    let mut info = PropertiesInfo {
        path: path.to_path_buf(),
        name,
        kind_label,
        icon,
        size_bytes,
        size_text,
        modified_text,
        accessed_text,
        owner,
        group,
        is_symlink,
        link_target,
        content_summary,
        mime_type,
        recursive_size_text: String::new(),
        owner_read: false,
        owner_write: false,
        owner_exec: false,
        group_read: false,
        group_write: false,
        group_exec: false,
        other_read: false,
        other_write: false,
        other_exec: false,
        // Placeholder; `set_mode_bits` sets it below.
        baseline_mode: 0,
    };
    // Also stamps `baseline_mode`, so `Model::permissions_dirty` starts clean.
    info.set_mode_bits(md.permissions().mode());
    Ok(info)
}

// Applies the nine permission bits in `info` to `info.path` with `chmod(2)`,
// preserving setuid, setgid and sticky.
pub(crate) fn apply_permissions(info: &PropertiesInfo) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = info.mode_bits();
    let cur = std::fs::symlink_metadata(&info.path)
        .map_err(|e| e.to_string())?
        .permissions()
        .mode();
    let full = (cur & 0o7000) | mode;
    std::fs::set_permissions(&info.path, std::fs::Permissions::from_mode(full))
        .map_err(|e| e.to_string())
}

// Direct-children summary, "X files, Y folders", from one `read_dir`.
fn summarize_directory(path: &Path) -> String {
    let mut files = 0usize;
    let mut dirs = 0usize;
    if let Ok(rd) = std::fs::read_dir(path) {
        for dent in rd.flatten() {
            if dent.file_type().is_ok_and(|ft| ft.is_dir()) {
                dirs += 1;
            } else {
                files += 1;
            }
        }
    }
    crate::i18n::format(
        "files-folder-contents",
        &[
            (
                "files",
                crate::i18n::count("files-file-count", files as u64),
            ),
            (
                "folders",
                crate::i18n::count("files-folder-count", dirs as u64),
            ),
        ],
    )
}

// Recursively sums the size and count of everything under `path`, excluding the
// root, as `(total_bytes, item_count)`.
//
// Uses [`crate::folder_size::walk`], which does not follow symlinks. Slow on deep
// trees; run it off the reducer.
pub(crate) fn recursive_folder_size(path: &Path) -> (u64, usize) {
    let never = std::sync::atomic::AtomicBool::new(false);
    let tally = crate::folder_size::walk(path, &never, |_| {}).unwrap_or_default();
    (
        tally.bytes,
        usize::try_from(tally.items).unwrap_or(usize::MAX),
    )
}

// Collects every entry under `root` whose name contains `needle`,
// case-insensitively; the `SearchScope::Everywhere` walk.
//
// Does not follow symlinks and stays on `root`'s filesystem: a mount below it
// (`/proc` under `/`, a network share under `$HOME`) is matched by name but
// not entered. Must run off the reducer; returns early once `cancelled` turns
// true. An empty `needle` returns nothing, and `MAX_HITS` bounds the result.
pub(crate) fn recursive_search(
    root: &Path,
    needle: &str,
    cancelled: &dyn Fn() -> bool,
) -> Vec<FileEntry> {
    use std::os::unix::fs::MetadataExt;
    const MAX_HITS: usize = 5000;
    let needle = needle.to_lowercase();
    let mut hits: Vec<FileEntry> = Vec::new();
    if needle.is_empty() {
        return hits;
    }
    let root_device = root.metadata().map(|metadata| metadata.dev()).ok();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if hits.len() >= MAX_HITS || cancelled() {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for dent in rd.flatten() {
            let path = dent.path();
            // Never follow links, so cycles cannot loop.
            let Ok(md) = path.symlink_metadata() else {
                continue;
            };
            if md.file_type().is_dir() && root_device.is_none_or(|device| md.dev() == device) {
                stack.push(path.clone());
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.to_lowercase().contains(&needle)
                && let Some(entry) = entry_from_path(&path)
            {
                hits.push(entry);
                if hits.len() >= MAX_HITS {
                    break;
                }
            }
        }
    }
    hits
}

#[allow(unsafe_code, reason = "libc getpwuid_r FFI")]
fn lookup_user(uid: u32) -> String {
    use std::ffi::CStr;
    let mut buf = [0i8; 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: owned buffers of the correct sizes; `result` is checked before
    // `pwd` is read, and `getpwuid_r` is thread-safe.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return uid.to_string();
    }
    // SAFETY: successful getpwuid_r initialized pwd and its output pointers.
    let pwd = unsafe { pwd.assume_init() };
    // SAFETY: pw_name is NUL-terminated within the still-live lookup buffer.
    let name = unsafe { CStr::from_ptr(pwd.pw_name) };
    name.to_string_lossy().into_owned()
}

#[allow(unsafe_code, reason = "libc getgrgid_r FFI")]
fn lookup_group(gid: u32) -> String {
    use std::ffi::CStr;
    let mut buf = [0i8; 1024];
    let mut grp = std::mem::MaybeUninit::<libc::group>::zeroed();
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: as `lookup_user`.
    let rc = unsafe {
        libc::getgrgid_r(
            gid,
            grp.as_mut_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return gid.to_string();
    }
    // SAFETY: successful getgrgid_r initialized grp and its output pointers.
    let grp = unsafe { grp.assume_init() };
    // SAFETY: gr_name is NUL-terminated within the still-live lookup buffer.
    let name = unsafe { CStr::from_ptr(grp.gr_name) };
    name.to_string_lossy().into_owned()
}
