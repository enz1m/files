use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use enzui_core::cancellation::CancelToken;
use enzui_icons::file_type::FileTypeTable;
use enzui_services::process_transport::{
    self, OutputStream, ProcessError, ProcessErrorKind, ProcessOutput, ProcessRequest, StdinPolicy,
};

use crate::model::{Drive, DriveKind, EntryKind, FileEntry, Place, PlaceKind, PropertiesInfo};

// Recent, Trash and Computer are virtual screens; their sentinel `cwd` values
// cannot collide with real filesystem paths.

// Sample discovery output for the `FILES_APP_OPEN=attach` capture seed and
// parser tests; the live Discover path never returns it.
//
// Tab-separated `host<TAB>share[<TAB>auth]`, one per line.
pub(crate) const DISCOVERY_FIXTURE: &str = "\
# discovered SMB shares\n\
fileserver\tpublic\n\
fileserver\tprivate\tauth\n\
nas.local\tmedia\n";

pub(crate) const RECENT_VIEW_ROOT: &str = "files-app://recent";
pub(crate) const TRASH_VIEW_ROOT: &str = "files-app://trash";
pub(crate) const COMPUTER_VIEW_ROOT: &str = "files-app://computer";
// The Downloads plugin's virtual view (in-process HTTP plus optional aria2 torrents).
pub(crate) const DOWNLOADS_VIEW_ROOT: &str = "files-app://downloads";

pub(crate) const NETWORK_VIEW_ROOT: &str = "files-app://network";

pub(crate) fn path_suggestions(query: &str, home: &Path) -> Vec<PathBuf> {
    if query.is_empty() {
        return Vec::new();
    }
    let expanded = expand_user_path(query, home);
    let (parent, prefix) = if query.ends_with('/') {
        (expanded.as_path(), "".to_owned())
    } else {
        (
            expanded.parent().unwrap_or(home),
            expanded
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        )
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = std::collections::BTreeSet::new();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && (entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.path().is_dir())
        {
            matches.insert(entry.path());
            if matches.len() > 32 {
                matches.pop_last();
            }
        }
    }
    matches.into_iter().collect()
}

// Read a directory into `FileEntry` rows. Symlinks are noticed via
// `symlink_metadata` and then dereferenced to fill kind/size for the target.
pub(crate) fn read_directory(path: &Path) -> io::Result<Vec<FileEntry>> {
    let mut out = Vec::new();
    for dent in std::fs::read_dir(path)? {
        let Ok(dent) = dent else { continue };
        if let Some(entry) = entry_from_path(&dent.path()) {
            out.push(entry);
        }
    }
    Ok(out)
}

// A file's type: by name, then, for a regular file whose name says
// nothing, by its first bytes; empty for a folder. Touches the disk, so call
// it off the UI thread.
pub(crate) fn file_mime(path: &Path, name: &str) -> String {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return FileTypeTable::host().classify(name).mime;
    };
    if metadata.is_dir() {
        return String::new();
    }
    let by_name = FileTypeTable::host().classify(name).mime;
    if by_name == UNKNOWN_MIME && metadata.is_file() {
        return sniff_mime(path, metadata.len()).unwrap_or(by_name);
    }
    by_name
}

// What a name with no recognised type classifies as.
const UNKNOWN_MIME: &str = "application/octet-stream";

// How much of a file without a recognised name is read to tell text from data.
const SNIFF_BYTES: usize = 512;

// The type of a regular file whose name says nothing (`todo`, `Makefile.bak`),
// from its first bytes: empty, or UTF-8 text with no NUL. `None` keeps the
// unknown type. Runs in the listing worker; the open never blocks.
fn sniff_mime(path: &Path, size: u64) -> Option<String> {
    use std::io::Read;
    if size == 0 {
        return Some("application/x-zerosize".to_owned());
    }
    let file = open_regular_file(path).ok()?;
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    file.take(SNIFF_BYTES as u64).read_to_end(&mut head).ok()?;
    if head.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(&head) {
        Ok(_) => true,
        // Cut mid-character at the read limit: still text.
        Err(error) => error.error_len().is_none() && error.valid_up_to() + 4 > head.len(),
    };
    text.then(|| "text/plain".to_owned())
}

// `None` when the path vanished after the directory read.
fn entry_from_path(entry_path: &Path) -> Option<FileEntry> {
    let name = entry_path.file_name()?.to_string_lossy().into_owned();
    let is_hidden = name.starts_with('.');
    let sym_md = std::fs::symlink_metadata(entry_path).ok()?;
    let (kind, size, modified) = if sym_md.file_type().is_symlink() {
        let target_md = std::fs::metadata(entry_path).ok();
        let kind = if target_md.as_ref().is_some_and(std::fs::Metadata::is_dir) {
            EntryKind::Dir
        } else {
            EntryKind::Symlink
        };
        let size = target_md.as_ref().map_or(0, std::fs::Metadata::len);
        (kind, size, sym_md.modified().ok())
    } else if sym_md.is_dir() {
        (EntryKind::Dir, 0, sym_md.modified().ok())
    } else {
        (EntryKind::File, sym_md.len(), sym_md.modified().ok())
    };

    // The framework's shared-mime-info classifier applies glob precedence
    // (`CMakeLists.txt` over `*.txt`, `*.tar.gz` over `*.gz`) against the host
    // database, which `mime_guess`'s bundled extension map does not.
    let mime = if matches!(kind, EntryKind::Dir) {
        String::new()
    } else {
        let by_name = FileTypeTable::host().classify(&name).mime;
        if by_name == UNKNOWN_MIME && sym_md.is_file() {
            sniff_mime(entry_path, size).unwrap_or(by_name)
        } else {
            by_name
        }
    };

    Some(FileEntry {
        path: entry_path.to_path_buf(),
        name,
        kind,
        size,
        modified,
        is_hidden,
        mime,
    })
}

// The default places sidebar: Home, XDG directories, Trash and Computer.
//
// There is no Recent row: the app has no recent-file source, so
// `Screen::Recent` is unreachable from the sidebar.
pub(crate) fn default_places(home: &Path) -> Vec<Place> {
    let mut places = Vec::new();

    places.push(Place {
        id: "home".into(),
        kind: PlaceKind::Home,
        label: crate::i18n::tr("Home").into(),
        icon: "house".into(),
        path: home.to_path_buf(),
    });

    // (label, lucide icon, resolver)
    let xdg: [(&str, &str, Option<PathBuf>); 6] = [
        (
            crate::i18n::tr("Documents"),
            "file-text",
            dirs::document_dir(),
        ),
        (
            crate::i18n::tr("Downloads"),
            "download",
            dirs::download_dir(),
        ),
        (crate::i18n::tr("Pictures"), "image", dirs::picture_dir()),
        (crate::i18n::tr("Music"), "music", dirs::audio_dir()),
        (crate::i18n::tr("Videos"), "video", dirs::video_dir()),
        (crate::i18n::tr("Desktop"), "monitor", dirs::desktop_dir()),
    ];
    for (label, icon, path) in xdg {
        if let Some(path) = path
            && path.starts_with(home)
            && path.exists()
        {
            places.push(Place {
                id: format!("xdg:{}", label.to_lowercase()),
                kind: PlaceKind::XdgDir,
                label: label.to_owned(),
                icon: icon.to_owned(),
                path,
            });
        }
    }

    places.push(Place {
        id: "trash".into(),
        kind: PlaceKind::Trash,
        label: crate::i18n::tr("Trash").into(),
        icon: "trash-2".into(),
        path: PathBuf::from(TRASH_VIEW_ROOT),
    });
    places.push(Place {
        id: "computer".into(),
        kind: PlaceKind::Computer,
        label: crate::i18n::tr("Computer").into(),
        icon: "hard-drive".into(),
        path: PathBuf::from(COMPUTER_VIEW_ROOT),
    });
    places.push(Place {
        id: "network-panel".into(),
        kind: PlaceKind::Network,
        label: crate::i18n::tr("Network").into(),
        icon: "network".into(),
        path: PathBuf::from(NETWORK_VIEW_ROOT),
    });
    // The only route into the downloads panel. Labelled "Download Manager"
    // because `enz_sidebar_row` derives its AT id from the label and the XDG
    // folder row is already "Downloads".
    places.push(Place {
        id: "downloads-panel".into(),
        kind: PlaceKind::Downloads,
        label: crate::i18n::tr("Download Manager").into(),
        icon: "download".into(),
        path: PathBuf::from(DOWNLOADS_VIEW_ROOT),
    });
    // No root row: `/` is reachable via Computer → System, a pinned bookmark,
    // or the path bar.

    places
}

#[path = "services_parts/pins.rs"]
mod pins;
pub(crate) use pins::{
    PIN_COUNT_CAP, PIN_FILE_BYTE_CAP, PIN_LABEL_BYTE_CAP, PIN_PATH_BYTE_CAP, PinIoCategory,
    PinLoadOutcome, PinRefusal, bookmarks_path, default_pins, load_sidebar_pins_from_authority,
    pin_open_refusal, publish_bookmarks, serialize_bookmarks,
};
#[cfg(test)]
pub(crate) use pins::{load_bookmarks_from, save_bookmarks_to};

#[path = "services_parts/preview.rs"]
mod preview;
pub(crate) use preview::{is_previewable_mime, is_virtual, read_preview_content};

#[path = "services_parts/drives.rs"]
mod drives;
pub(crate) use drives::decode_mount_field;
pub(crate) use drives::list_drives;

// The "Size" column string; directories show an em dash rather than
// enumerating children during directory loading.
pub(crate) fn format_size(entry: &FileEntry) -> String {
    if matches!(entry.kind, EntryKind::Dir) {
        return "—".to_owned();
    }
    format_bytes(entry.size)
}

// Decimal-unit byte formatting (matches `df -h`): `1.50 GB`, `842 B`.
pub(crate) fn format_bytes(bytes: u64) -> String {
    enzui_widgets::file_browser::format_bytes(bytes)
}

// A short human modification string (`Jul 9, 14:03` this year; `Jul 9, 2025`
// otherwise), or blank when the host read no time.
pub(crate) fn format_modified(entry: &FileEntry) -> String {
    match entry.modified {
        Some(m) => format_time(m),
        None => String::new(),
    }
}

// A short human timestamp (`Jul 9, 14:03` this year; `Jul 9, 2025`
// otherwise), matching [`format_modified`]; pre-epoch times are rendered.
pub(crate) fn format_time(t: SystemTime) -> String {
    enzui_widgets::file_browser::format_time(t)
}

// Expand a leading `~` in a user-typed path to the home directory.
pub(crate) fn expand_user_path(input: &str, home: &Path) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

// The user's home directory, falling back to `/` so the app always opens.
pub(crate) fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

#[path = "services_parts/properties.rs"]
mod properties;
pub(crate) use properties::{
    apply_permissions, build_properties, recursive_folder_size, recursive_search,
};

// The executor consumes `RenamePlan::operations()`, the same plan the preview
// renders; there is no second derivation of the new names.

// The file names in `dir` that are not in `exclude`.
//
// Snapshotted when the bulk-rename dialog opens so
// `BulkRename::plan` can classify on-disk collisions while staying pure.
pub(crate) fn occupied_names(
    dir: &Path,
    exclude: &[PathBuf],
) -> std::collections::BTreeSet<String> {
    let excluded: std::collections::BTreeSet<&std::ffi::OsStr> =
        exclude.iter().filter_map(|p| p.file_name()).collect();
    let mut out = std::collections::BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for dent in rd.flatten() {
        let name = dent.file_name();
        if excluded.contains(name.as_os_str()) {
            continue;
        }
        out.insert(name.to_string_lossy().into_owned());
    }
    out
}

// Every entry in the folders holding `targets`, as full paths, except the
// targets themselves: what a rename of the targets could clash with.
pub(crate) fn occupied_siblings(targets: &[PathBuf]) -> std::collections::BTreeSet<PathBuf> {
    let parents: std::collections::BTreeSet<&Path> = targets
        .iter()
        .filter_map(|target| target.parent())
        .collect();
    let excluded: std::collections::BTreeSet<&PathBuf> = targets.iter().collect();
    let mut out = std::collections::BTreeSet::new();
    for parent in parents {
        let Ok(rd) = std::fs::read_dir(parent) else {
            continue;
        };
        for dent in rd.flatten() {
            let path = parent.join(dent.file_name());
            if !excluded.contains(&path) {
                out.insert(path);
            }
        }
    }
    out
}

// A name not yet present in `dir`: `New Folder`, `New Folder (2)`, ...,
// with the counter before the extension (`notes (2).txt`) so mime detection
// is unaffected.
//
// This only suggests; a race with another process can still take the name,
// and the creators then report an error instead of retrying.
pub(crate) fn suggest_unique_name(dir: &Path, base: &str, ext: Option<&str>) -> String {
    let name_of = |i: usize| -> String {
        let stem = if i == 0 {
            base.to_owned()
        } else {
            format!("{base} ({})", i + 1)
        };
        match ext {
            Some(e) if !e.is_empty() => format!("{stem}.{e}"),
            _ => stem,
        }
    };

    let occupied = occupied_names(dir, &[]);

    for i in 0..=occupied.len() {
        let candidate = name_of(i);
        if !occupied.contains(&candidate) {
            return candidate;
        }
    }
    // Unreachable: `occupied` has `len` names, so one of `0..=len` is free.
    name_of(occupied.len())
}

// Creates a uniquely named "New Folder" in `dir` and returns its path.
//
// Performs blocking I/O.
pub(crate) fn create_folder(dir: &Path) -> Result<PathBuf, String> {
    let name = suggest_unique_name(dir, "New Folder", None);
    let path = dir.join(&name);
    match std::fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(e) => Err(crate::i18n::format(
            "files-could-not-create-fb74556c",
            &[
                ("arg0", (name)),
                (
                    "arg1",
                    (e.to_string().lines().next().unwrap_or("")).to_owned(),
                ),
            ],
        )),
    }
}

// Creates a uniquely named empty file in `dir` and returns its path.
//
// `File::create_new` never overwrites, so a lost race is an error.
pub(crate) fn create_file(dir: &Path) -> Result<PathBuf, String> {
    let name = suggest_unique_name(dir, "New File", Some("txt"));
    let path = dir.join(&name);
    match std::fs::File::create_new(&path) {
        Ok(_f) => Ok(path),
        Err(e) => Err(crate::i18n::format(
            "files-could-not-create-fb74556c",
            &[
                ("arg0", (name)),
                (
                    "arg1",
                    (e.to_string().lines().next().unwrap_or("")).to_owned(),
                ),
            ],
        )),
    }
}

// Create a document in `dir` from the `template` file.
//
// The name is the template's own, made unique the way New File's is
// (`Letter (2).odt`). The template must be a regular file: a symlink,
// directory or device is refused. See [`copy_as_new`] for the publish.
pub(crate) fn create_from_template(dir: &Path, template: &Path) -> Result<PathBuf, String> {
    let display = display_name(template);
    let fail = |detail: String| {
        crate::i18n::format(
            "files-could-not-create-from-display-detail-e9195556",
            &[("display", (display).clone()), ("detail", (detail))],
        )
    };
    let meta = std::fs::symlink_metadata(template).map_err(|e| fail(first_line(&e)))?;
    if !meta.file_type().is_file() {
        return Err(fail(
            crate::i18n::tr("the template is not a regular file").into(),
        ));
    }
    let name = template
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail(crate::i18n::tr("the template name is not valid text").into()))?;
    let (stem, ext) = split_extension(name, false);
    copy_as_new(template, dir, stem, ext).map_err(fail)
}

// Duplicate `source` beside itself as `Name (copy).ext`, then
// `Name (copy) (2).ext`. A folder is copied whole and a symlink as a
// symlink; a device, socket or pipe is refused.
pub(crate) fn duplicate_entry(source: &Path) -> Result<PathBuf, String> {
    let display = display_name(source);
    let fail = |detail: String| {
        crate::i18n::format(
            "files-could-not-duplicate-display-detail-64f6ed35",
            &[("display", (display).clone()), ("detail", (detail))],
        )
    };
    let dir = source
        .parent()
        .ok_or_else(|| fail(crate::i18n::tr("it has no containing folder").into()))?;
    let meta = std::fs::symlink_metadata(source).map_err(|e| fail(first_line(&e)))?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail(crate::i18n::tr("the name is not valid text").into()))?;
    let (stem, ext) = split_extension(name, meta.is_dir());
    copy_as_new(source, dir, &format!("{stem} (copy)"), ext).map_err(fail)
}

// Copy `source` into `dir` under the first free name built from `stem` and
// `ext`, never exposing a partial copy and never replacing anything.
//
// The bytes go into an exclusively created hidden sibling (`O_EXCL`,
// `mkdir`, or `symlink`, which all refuse an existing name) and are flushed,
// then published with a no-replace rename. A name that appeared meanwhile
// is reported rather than overwritten or silently retried, like the other
// creators.
fn copy_as_new(
    source: &Path,
    dir: &Path,
    stem: &str,
    ext: Option<&str>,
) -> Result<PathBuf, String> {
    let target = dir.join(suggest_unique_name(dir, stem, ext));
    let staged = stage::stage_source_and_flush(source, &target, &stage::MoveSeams::PRODUCTION)?;
    if let Err(error) = stage::rename_paths_no_replace(&staged, &target) {
        let cleanup = publish::remove_stage_residue_report(&staged);
        let detail = if error.kind() == std::io::ErrorKind::AlreadyExists {
            crate::i18n::format(
                "files-appeared-while-it-was-being-created-8b72eb76",
                &[("arg0", (display_name(&target)))],
            )
        } else {
            first_line(&error)
        };
        return Err(format!("{detail}{cleanup}"));
    }
    sync_dir(dir);
    Ok(target)
}

// Create a symbolic link to `target` in `dir`, named
// `Link to <name>` (made unique). The link stores `target`'s absolute path.
// `symlink(2)` refuses an existing name, so nothing is ever replaced; a
// filesystem without links (FAT, many network shares) says so.
pub(crate) fn create_link(dir: &Path, target: &Path) -> Result<PathBuf, String> {
    let display = display_name(target);
    let fail = |detail: String| {
        crate::i18n::format(
            "files-could-not-link-display-detail-b6bdb061",
            &[("display", (display).clone()), ("detail", (detail))],
        )
    };
    if !target.is_absolute() {
        return Err(fail(
            crate::i18n::tr("the target is not an absolute path").into(),
        ));
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| fail(crate::i18n::tr("the name is not valid text").into()))?;
    let link = dir.join(suggest_unique_name(dir, &format!("Link to {name}"), None));
    match std::os::unix::fs::symlink(target, &link) {
        Ok(()) => {
            sync_dir(dir);
            Ok(link)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(fail(crate::i18n::format(
                "files-appeared-while-it-was-being-created-8b72eb76",
                &[("arg0", (display_name(&link)))],
            )))
        }
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EPERM | libc::EOPNOTSUPP | libc::ENOSYS)
            ) =>
        {
            Err(fail(
                crate::i18n::tr("this location does not support links").into(),
            ))
        }
        Err(error) => Err(fail(first_line(&error))),
    }
}

// `name` split for unique naming: a folder or dot-file keeps its whole name.
fn split_extension(name: &str, is_dir: bool) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !is_dir && !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn sync_dir(dir: &Path) {
    let _ = std::fs::File::open(dir).and_then(|dir| dir.sync_all());
}

fn first_line(error: &std::io::Error) -> String {
    error.to_string().lines().next().unwrap_or("").to_owned()
}

// The outcome of one applied bulk rename.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BulkRenameOutcome {
    // `(from, to)` pairs that landed, in the order performed.
    pub(crate) renamed: Vec<(PathBuf, PathBuf)>,
    // `(from, error)` for pairs that failed.
    pub(crate) failed: Vec<(PathBuf, String)>,
}

impl BulkRenameOutcome {
    pub(crate) fn summary(&self) -> String {
        let n = self.renamed.len();
        let f = self.failed.len();
        let summary = match (n, f) {
            (0, 0) => crate::i18n::tr("Nothing to rename").to_owned(),
            (1, 0) => crate::i18n::tr("Renamed 1 file").to_owned(),
            (n, 0) => {
                crate::i18n::format("files-renamed-n-files-af03a8aa", &[("n", (n).to_string())])
            }
            (0, f) => crate::i18n::format(
                "files-rename-failed-for-f-file-s-60a32542",
                &[("f", (f).to_string())],
            ),
            (n, f) => crate::i18n::format(
                "files-renamed-n-file-s-f-failed-f5c76163",
                &[("n", (n).to_string()), ("f", (f).to_string())],
            ),
        };
        if self.failed.is_empty() {
            summary
        } else {
            crate::i18n::format(
                "files-bulk-rename-failure-details",
                &[
                    ("summary", summary),
                    (
                        "details",
                        self.failed
                            .iter()
                            .map(|(path, error)| format!("{}: {error}", path.display()))
                            .collect::<Vec<_>>()
                            .join("; "),
                    ),
                ],
            )
        }
    }
}

pub(crate) fn apply_bulk_rename(operations: &[(PathBuf, PathBuf)]) -> BulkRenameOutcome {
    bulk_rename::apply(operations)
}

#[path = "services_parts/bulk_rename.rs"]
mod bulk_rename;
#[path = "services_parts/publish.rs"]
mod publish;
#[path = "services_parts/stage.rs"]
mod stage;
pub(crate) use publish::{perform_conflict_choice, plan_paste, plan_trash_restore};
pub(crate) use stage::rename_paths_no_replace;
pub(crate) use stage::{
    is_cross_filesystem_error, move_merging, move_replacing, rename_reversing,
    same_fs_no_replace_rename,
};

// Explicit resource policy for the short unmount helpers.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnmountLimits {
    pub(crate) stdout_cap: usize,
    pub(crate) stderr_cap: usize,
    pub(crate) deadline: Duration,
}

const UNMOUNT_LIMITS: UnmountLimits = UnmountLimits {
    stdout_cap: 4 * 1024,
    stderr_cap: 4 * 1024,
    deadline: Duration::from_secs(5),
};

// Owns resolution and bounded execution of `udisksctl` / `umount`.
//
// The caller owns the supplied cancellation token for the full reducer task.
// Every argument remains an [`OsString`]; no display string is parsed back
// into argv, so mount points and device names containing spaces stay exact.
#[derive(Clone, Debug)]
pub(crate) struct UnmountService {
    search: crate::exe_resolver::SearchPath,
    limits: UnmountLimits,
    #[cfg(test)]
    source_override: Option<Option<OsString>>,
}

impl UnmountService {
    pub(crate) fn production() -> Self {
        Self {
            search: crate::exe_resolver::SearchPath::from_env(),
            limits: UNMOUNT_LIMITS,
            #[cfg(test)]
            source_override: None,
        }
    }

    // Helpers for removable-media operations, with a longer deadline because
    // mount and power-off may wait on an authorization prompt.
    pub(crate) fn for_devices() -> Self {
        Self {
            search: crate::exe_resolver::SearchPath::from_env(),
            limits: UnmountLimits {
                deadline: Duration::from_secs(120),
                ..UNMOUNT_LIMITS
            },
            #[cfg(test)]
            source_override: None,
        }
    }

    fn source_for(&self, mount: &Path) -> Option<OsString> {
        #[cfg(test)]
        if let Some(source) = &self.source_override {
            return source.clone();
        }
        source_for_mount(mount).map(OsString::from)
    }

    // Unmounts `mount` under the caller's cancellation owner.
    //
    // Block devices use `udisksctl`, user FUSE mounts use `fusermount3` or
    // `fusermount`, and other filesystems use `umount`. A complete UDisks
    // refusal can fall back; transport failures never start another helper.
    pub(crate) fn unmount(&self, mount: &Path, cancel: &CancelToken) -> Result<PathBuf, String> {
        if let Some(source) = self
            .source_for(mount)
            .filter(|source| Path::new(source).starts_with("/dev"))
        {
            let args = [OsString::from("unmount"), OsString::from("-b"), source];
            match self.run_helper("udisksctl", args, cancel) {
                Ok(_) => return Ok(mount.to_path_buf()),
                Err(HelperFailure::NotFound { .. } | HelperFailure::NonZero { .. }) => {
                    // Usually a mount UDisks does not manage.
                }
                Err(error @ HelperFailure::Transport { .. }) => {
                    return Err(error.into_line());
                }
            }
        }

        if crate::storage::is_fuse_mount(mount) {
            let args = [
                OsString::from("-u"),
                OsString::from("--"),
                mount.as_os_str().to_owned(),
            ];
            return match self.run_helper("fusermount3", args.clone(), cancel) {
                Ok(_) => Ok(mount.to_owned()),
                Err(HelperFailure::NotFound { .. }) => self
                    .run_helper("fusermount", args, cancel)
                    .map(|_| mount.to_owned())
                    .map_err(HelperFailure::into_line),
                Err(error) => Err(error.into_line()),
            };
        }
        self.run_helper("umount", [mount.as_os_str().to_os_string()], cancel)
            .map(|_| mount.to_path_buf())
            .map_err(HelperFailure::into_line)
    }

    // Runs one resolved helper under the bounded transport, returning its
    // output on success.
    pub(crate) fn run_helper(
        &self,
        program: &'static str,
        args: impl IntoIterator<Item = OsString>,
        cancel: &CancelToken,
    ) -> Result<ProcessOutput, HelperFailure> {
        let Some(executable) = self.search.resolve(program) else {
            return Err(HelperFailure::NotFound { program });
        };
        let request = ProcessRequest::new(
            executable.into_os_string(),
            StdinPolicy::Null,
            self.limits.stdout_cap,
            self.limits.stderr_cap,
            self.limits.deadline,
        )
        .args(args);
        match process_transport::run(request, cancel) {
            Ok(output) if output.status.success() => Ok(output),
            Ok(output) => Err(HelperFailure::NonZero { program, output }),
            Err(error) => Err(HelperFailure::Transport { program, error }),
        }
    }
}

#[derive(Debug)]
pub(crate) enum HelperFailure {
    NotFound {
        program: &'static str,
    },
    // A complete response after cleanup finished; the only class that may
    // fall back to another helper.
    NonZero {
        program: &'static str,
        output: ProcessOutput,
    },
    // An incomplete lifecycle outcome; never eligible for fallback.
    Transport {
        program: &'static str,
        error: ProcessError,
    },
}

impl HelperFailure {
    pub(crate) fn into_line(self) -> String {
        match self {
            Self::NotFound { program } => crate::i18n::format(
                "files-program-not-found-5e683303",
                &[("program", (program).to_owned())],
            ),
            Self::NonZero { program, output } => {
                let fallback = match output.status.code() {
                    Some(code) => crate::i18n::format(
                        "files-program-exited-with-status-code-900e7c14",
                        &[
                            ("program", (program).to_owned()),
                            ("code", (code).to_string()),
                        ],
                    ),
                    None => crate::i18n::format(
                        "files-program-was-terminated-a6921ce3",
                        &[("program", (program).to_owned())],
                    ),
                };
                bounded_helper_line(&output.stderr).unwrap_or(fallback)
            }
            Self::Transport { program, error } => match error.kind() {
                ProcessErrorKind::Deadline => crate::i18n::format(
                    "files-program-timed-out-32d1396c",
                    &[("program", (program).to_owned())],
                ),
                ProcessErrorKind::Cancelled(_) => crate::i18n::format(
                    "files-program-cancelled-d13b0259",
                    &[("program", (program).to_owned())],
                ),
                ProcessErrorKind::OutputLimitExceeded(stream) => {
                    // After overflow the stderr capture may be a partial prefix
                    // or tail; only a `Complete` capture is shown.
                    let detail = complete_helper_line(&error).unwrap_or_else(|| match stream {
                        OutputStream::Stdout => crate::i18n::format(
                            "files-program-produced-too-much-output-be56bb3f",
                            &[("program", (program).to_owned())],
                        ),
                        OutputStream::Stderr => crate::i18n::format(
                            "files-program-produced-too-much-error-output-2b625ce0",
                            &[("program", (program).to_owned())],
                        ),
                    });
                    crate::i18n::format(
                        "files-detail-output-truncated-91d4c99c",
                        &[("detail", (detail))],
                    )
                }
                ProcessErrorKind::ExecutableAuthority => crate::i18n::format(
                    "files-program-executable-resolution-refused-f1680c3e",
                    &[("program", (program).to_owned())],
                ),
                ProcessErrorKind::Spawn => crate::i18n::format(
                    "files-program-spawn-failed-171fc45f",
                    &[
                        ("program", (program).to_owned()),
                        (
                            "arg0",
                            (error
                                .diagnostic()
                                .unwrap_or_else(|| crate::i18n::tr("unknown spawn error")))
                            .to_owned(),
                        ),
                    ],
                ),
                ProcessErrorKind::InvalidLimit(_)
                | ProcessErrorKind::Setup
                | ProcessErrorKind::StdinWrite
                | ProcessErrorKind::Read(_)
                | ProcessErrorKind::ReaderPanicked(_)
                | ProcessErrorKind::WorkerPanicked
                | ProcessErrorKind::Wait
                | ProcessErrorKind::GroupSignal
                | ProcessErrorKind::ResidualCleanup
                | ProcessErrorKind::DirectChildCleanup => format!("{program} {error}"),
            },
        }
    }
}

// The helper's first stderr line, only when the stderr capture is complete.
//
// Partial captures (overflow, deadline, cancellation) yield `None` so the
// caller never shows truncated bytes as a diagnostic.
fn complete_helper_line(error: &ProcessError) -> Option<String> {
    bounded_helper_line(error.complete_capture(OutputStream::Stderr)?)
}

fn bounded_helper_line(bytes: &[u8]) -> Option<String> {
    const DISPLAY_CAP: usize = 512;
    let text = String::from_utf8_lossy(bytes);
    let line = text.trim().lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let mut chars = line.chars();
    let mut bounded: String = chars.by_ref().take(DISPLAY_CAP).collect();
    if chars.next().is_some() {
        bounded.push('…');
    }
    Some(bounded)
}

// The device backing `mount`, from `/proc/mounts`. `None` when the mount point
// is not listed (already unmounted, or a synthetic path in a test).
fn source_for_mount(mount: &Path) -> Option<String> {
    let raw = std::fs::read_to_string("/proc/mounts").ok()?;
    let want = mount.to_string_lossy();
    for line in raw.lines() {
        let mut cols = line.split_whitespace();
        let source = cols.next()?;
        let target = cols.next()?;
        if decode_mount_field(target) == want {
            return Some(decode_mount_field(source));
        }
    }
    None
}

// Opens `path` for reading only if it is a regular file. The open itself is
// non-blocking, so a named pipe called `notes.txt` is refused at once
// instead of holding a worker until something writes to it.
pub(crate) fn open_regular_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            crate::i18n::tr("Not a regular file"),
        ));
    }
    Ok(file)
}
