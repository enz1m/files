// Sidebar pins and bookmark persistence.

use super::*;

// Format: one `label<TAB>path` per line; tabs and newlines in labels become
// spaces on write. Exceeding any `PIN_*_CAP` refuses the whole document and
// leaves the file intact and write-protected until the user retires it. A line
// with no tab or an empty path is skipped; other lines still load.

// Encoded-byte cap for the pins file: 128 KiB + 1 sentinel byte.
// A file of exactly this size is admitted; one byte larger is refused.
pub(crate) const PIN_FILE_BYTE_CAP: u64 = 128 * 1024 + 1;

// Physical line cap (including the v2 header).
pub(crate) const PIN_LINE_CAP: usize = 512;

// Maximum admitted pin rows after parsing.
pub(crate) const PIN_COUNT_CAP: usize = 256;

// Maximum encoded bytes in a single pin label.
pub(crate) const PIN_LABEL_BYTE_CAP: usize = 256;

// Maximum encoded bytes in a single pin path.
pub(crate) const PIN_PATH_BYTE_CAP: usize = 4096;

// The bookmarks file, `$XDG_CONFIG_HOME/files-app/bookmarks` (default
// `~/.config`), or `None` when no config directory exists.
#[must_use]
pub(crate) fn bookmarks_path() -> Option<PathBuf> {
    dirs::config_dir().map(|c| bookmarks_path_in(&c))
}

// The bookmarks file under an explicit config root, for sandboxed tests.
#[must_use]
pub(crate) fn bookmarks_path_in(config_root: &std::path::Path) -> PathBuf {
    config_root.join("files-app").join("bookmarks")
}

// The typed refusal for a bookmarks file whose authority would not open; a
// final symlink reports as `InvalidInput`, so the name is inspected.
pub(crate) fn pin_open_refusal(path: &std::path::Path, error: &std::io::Error) -> PinRefusal {
    let symlink = error.raw_os_error() == Some(libc::ELOOP)
        || std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if symlink {
        PinRefusal::SymlinkRefused
    } else if error.kind() == std::io::ErrorKind::NotFound {
        PinRefusal::Missing
    } else if error.kind() == std::io::ErrorKind::PermissionDenied {
        PinRefusal::Io(PinIoCategory::PermissionDenied)
    } else {
        PinRefusal::Io(PinIoCategory::Other)
    }
}

// Parse one `label<TAB>path` line into a [`Place`], or `None` when the line is
// malformed (no tab, or an empty path), or `Some(Err)` when a field exceeds
// its per-pin byte cap.
fn parse_bookmark_line(line: &str) -> Option<Result<Place, PinRefusal>> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let (label, path) = line.split_once('\t')?;
    let path = path.trim_end_matches('\n');
    if path.is_empty() {
        return None;
    }
    if label.len() > PIN_LABEL_BYTE_CAP {
        return Some(Err(PinRefusal::LabelTooLong {
            len: label.len(),
            cap: PIN_LABEL_BYTE_CAP,
        }));
    }
    if path.len() > PIN_PATH_BYTE_CAP {
        return Some(Err(PinRefusal::PathTooLong {
            len: path.len(),
            cap: PIN_PATH_BYTE_CAP,
        }));
    }
    let path = PathBuf::from(path);
    Some(Ok(Place {
        id: format!("bookmark:{}", path.to_string_lossy()),
        kind: PlaceKind::Bookmark,
        label: label.to_owned(),
        icon: "folder".into(),
        path,
    }))
}

// Header marking a complete, explicit pinned-Places ordering. Legacy files
// did not carry this marker and contained only user-added bookmarks.
pub(crate) const SIDEBAR_PINS_V2_HEADER: &str = "# files-app sidebar pins v2";

// Why a pins file or individual pin was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PinRefusal {
    // The file does not exist (first run or deleted).
    Missing,
    // The file path is a symbolic link (opened with `O_NOFOLLOW`).
    SymlinkRefused,
    // The file exceeds the encoded-byte cap.
    FileTooLarge { bytes: u64, cap: u64 },
    // The file contains more physical lines than the cap.
    TooManyLines { lines: usize, cap: usize },
    // The parsed document yields more admitted pins than the cap.
    TooManyPins { pins: usize, cap: usize },
    // One pin's label field exceeds its per-pin byte cap.
    LabelTooLong { len: usize, cap: usize },
    // One pin's path field exceeds its per-pin byte cap.
    PathTooLong { len: usize, cap: usize },
    // A path contains non-UTF-8 bytes; the v2 format cannot represent it.
    PathNotUtf8,
    // The file was replaced or mutated between open and read completion.
    ReplacedDuringRead,
    // An I/O or system error, represented as a bounded category.
    Io(PinIoCategory),
    // Bookmark persistence is not available on this configuration.
    Unavailable,
    // Custody was lost; the destination inode is no longer ours.
    CustodyLost,
}

// Bounded I/O failure category for [`PinRefusal::Io`].
//
// Raw paths and OS error strings are kept in logs, not in model/UI state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PinIoCategory {
    PermissionDenied,
    DirProvisionFailed,
    ReadFailed,
    Other,
}

impl std::fmt::Display for PinIoCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::DirProvisionFailed => write!(f, "configuration directory unavailable"),
            Self::ReadFailed => write!(f, "could not read file"),
            Self::Other => write!(f, "unexpected error"),
        }
    }
}

impl std::fmt::Display for PinRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SymlinkRefused => write!(f, "Bookmarks file is a symbolic link"),
            Self::FileTooLarge { bytes, cap } => {
                write!(f, "Bookmarks file is {bytes} bytes (limit {cap})")
            }
            Self::TooManyLines { lines, cap } => {
                write!(f, "Bookmarks file has {lines} lines (limit {cap})")
            }
            Self::TooManyPins { pins, cap } => {
                write!(f, "Bookmarks file has {pins} pins (limit {cap})")
            }
            Self::LabelTooLong { len, cap } => {
                write!(f, "A pin label is {len} bytes (limit {cap})")
            }
            Self::PathTooLong { len, cap } => {
                write!(f, "A pin path is {len} bytes (limit {cap})")
            }
            Self::PathNotUtf8 => {
                write!(
                    f,
                    "A pin path contains bytes the bookmark format cannot represent"
                )
            }
            Self::Missing => write!(f, "Bookmarks file does not exist"),
            Self::ReplacedDuringRead => {
                write!(f, "Bookmarks file was replaced during read")
            }
            Self::Io(cat) => write!(f, "Could not read bookmarks: {cat}"),
            Self::Unavailable => write!(f, "Bookmark persistence is not available"),
            Self::CustodyLost => write!(f, "Bookmarks file was modified externally"),
        }
    }
}

// The outcome of loading sidebar pins: either a successfully admitted set of
// places, or a refusal that preserves the original file.
#[derive(Clone, Debug)]
pub(crate) enum PinLoadOutcome {
    // The document was admitted. `retained_bytes` is the byte count of all
    // admitted label+path content (for counters).
    Admitted {
        pins: Vec<Place>,
        retained_rows: usize,
        retained_bytes: usize,
    },
    // The document was refused. The original file stays intact.
    Refused(PinRefusal),
}

impl PinLoadOutcome {}

// Serialises bookmarks with the v2 header, which makes an empty list an
// explicit "all defaults unpinned" state.
//
// Returns `Err` above `PIN_FILE_BYTE_CAP` or `PIN_COUNT_CAP`, so the UI cannot
// write a state the loader refuses.
pub(crate) fn serialize_bookmarks(bookmarks: &[Place]) -> Result<String, PinRefusal> {
    use std::os::unix::ffi::OsStrExt;

    if bookmarks.len() > PIN_COUNT_CAP {
        return Err(PinRefusal::TooManyPins {
            pins: bookmarks.len(),
            cap: PIN_COUNT_CAP,
        });
    }
    let mut s = format!("{SIDEBAR_PINS_V2_HEADER}\n");
    for b in bookmarks {
        let label = b.label.replace(['\t', '\n'], " ");
        if label.len() > PIN_LABEL_BYTE_CAP {
            return Err(PinRefusal::LabelTooLong {
                len: label.len(),
                cap: PIN_LABEL_BYTE_CAP,
            });
        }
        // Non-UTF-8 paths cannot round-trip through the v2 text format.
        let native = b.path.as_os_str().as_bytes();
        let path_str = std::str::from_utf8(native).map_err(|_| PinRefusal::PathNotUtf8)?;
        if native.len() > PIN_PATH_BYTE_CAP {
            return Err(PinRefusal::PathTooLong {
                len: native.len(),
                cap: PIN_PATH_BYTE_CAP,
            });
        }
        s.push_str(&label);
        s.push('\t');
        s.push_str(path_str);
        s.push('\n');
    }
    if s.len() as u64 > PIN_FILE_BYTE_CAP {
        return Err(PinRefusal::FileTooLarge {
            bytes: s.len() as u64,
            cap: PIN_FILE_BYTE_CAP,
        });
    }
    Ok(s)
}

#[must_use]
#[cfg(test)]
pub(crate) fn load_bookmarks_from(path: &Path) -> Vec<Place> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(parse_bookmark_line)
        .filter_map(std::result::Result::ok)
        .collect()
}

// Map a `StateAuthority::read_bounded` result to an optional UTF-8 body
// or a typed refusal. `None` means the authority retained an absent file.
fn read_authority_snapshot(
    authority: &enzui_core::state_authority::StateAuthority,
) -> Result<Option<String>, PinRefusal> {
    use enzui_core::state_authority::{CustodySnapshot, ReadError};

    let snapshot = authority
        .read_bounded(PIN_FILE_BYTE_CAP as usize)
        .map_err(|e| match e {
            ReadError::ConcurrentMutation => PinRefusal::ReplacedDuringRead,
            ReadError::CapOverflow => PinRefusal::FileTooLarge {
                bytes: u64::MAX,
                cap: PIN_FILE_BYTE_CAP,
            },
            ReadError::Io(io) => {
                if io.kind() == std::io::ErrorKind::PermissionDenied {
                    PinRefusal::Io(PinIoCategory::PermissionDenied)
                } else {
                    PinRefusal::Io(PinIoCategory::ReadFailed)
                }
            }
        })?;

    match snapshot {
        CustodySnapshot::Absent => Ok(None),
        CustodySnapshot::Lost => Err(PinRefusal::ReplacedDuringRead),
        CustodySnapshot::Current {
            bytes, overflow, ..
        } => {
            if overflow {
                return Err(PinRefusal::FileTooLarge {
                    bytes: bytes.len() as u64 + 1,
                    cap: PIN_FILE_BYTE_CAP,
                });
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| PinRefusal::Io(PinIoCategory::ReadFailed))
        }
    }
}

// Parse an admitted pins-file body into places, enforcing all per-document
// and per-pin bounds. Duplicate paths use first-occurrence precedence.
fn parse_pins_bounded(
    contents: &str,
    defaults: &[Place],
) -> Result<(Vec<Place>, bool), PinRefusal> {
    let line_count = contents.lines().count();
    if line_count > PIN_LINE_CAP {
        return Err(PinRefusal::TooManyLines {
            lines: line_count,
            cap: PIN_LINE_CAP,
        });
    }

    let is_v2 = contents.lines().next() == Some(SIDEBAR_PINS_V2_HEADER);

    let mut loaded: Vec<Place> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();
    for line in contents.lines() {
        if let Some(result) = parse_bookmark_line(line) {
            let place = result?;
            if !seen_paths.insert(place.path.clone()) {
                continue;
            }
            // Recover built-in row semantics from defaults by path.
            let resolved = defaults
                .iter()
                .find(|d| d.path == place.path)
                .cloned()
                .unwrap_or(place);
            loaded.push(resolved);
        }
    }

    if loaded.len() > PIN_COUNT_CAP {
        return Err(PinRefusal::TooManyPins {
            pins: loaded.len(),
            cap: PIN_COUNT_CAP,
        });
    }

    Ok((loaded, is_v2))
}

// Aggregate retained bytes for a pin set, using native path bytes.
fn aggregate_retained_bytes(pins: &[Place]) -> usize {
    use std::os::unix::ffi::OsStrExt;
    pins.iter()
        .map(|p| p.label.len() + p.path.as_os_str().as_bytes().len())
        .sum()
}

// Deduplicates by path (first wins), then enforces count and byte caps; every
// load path admits through here.
fn admit_pins(pins: Vec<Place>) -> PinLoadOutcome {
    let mut deduped: Vec<Place> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in pins {
        if seen.insert(p.path.clone()) {
            deduped.push(p);
        }
    }
    if deduped.len() > PIN_COUNT_CAP {
        return PinLoadOutcome::Refused(PinRefusal::TooManyPins {
            pins: deduped.len(),
            cap: PIN_COUNT_CAP,
        });
    }
    let retained_bytes = aggregate_retained_bytes(&deduped);
    if retained_bytes as u64 > PIN_FILE_BYTE_CAP {
        return PinLoadOutcome::Refused(PinRefusal::FileTooLarge {
            bytes: retained_bytes as u64,
            cap: PIN_FILE_BYTE_CAP,
        });
    }
    PinLoadOutcome::Admitted {
        retained_rows: deduped.len(),
        retained_bytes,
        pins: deduped,
    }
}

// Load sidebar pins from a held `StateAuthority`, migrating the legacy format.
//
// Absent authority yields deduplicated defaults. Lost custody is a refusal.
// A v2 file is authoritative; a legacy file prepends defaults.
pub(crate) fn load_sidebar_pins_from_authority(
    authority: &enzui_core::state_authority::StateAuthority,
    defaults: &[Place],
) -> PinLoadOutcome {
    let contents = match read_authority_snapshot(authority) {
        Ok(Some(c)) => c,
        Ok(None) => return admit_pins(defaults.to_vec()),
        Err(refusal) => return PinLoadOutcome::Refused(refusal),
    };

    let (loaded, is_v2) = match parse_pins_bounded(&contents, defaults) {
        Ok(pair) => pair,
        Err(refusal) => return PinLoadOutcome::Refused(refusal),
    };

    let merged = if is_v2 {
        loaded
    } else {
        let mut combined: Vec<Place> = defaults.to_vec();
        combined.extend(loaded);
        combined
    };

    admit_pins(merged)
}

// Publish serialized bookmarks through a held authority. On success the
// authority rotates custody to the newly published inode.
//
// Returns `Err` when serialization exceeds admission bounds or when the
// authority refuses publication.
pub(crate) fn publish_bookmarks(
    authority: &mut enzui_core::state_authority::StateAuthority,
    bookmarks: &[Place],
) -> Result<(), PinPublishError> {
    let body = serialize_bookmarks(bookmarks).map_err(PinPublishError::Serialization)?;
    authority
        .publish(body.as_bytes())
        .map(|_| ())
        .map_err(PinPublishError::Publication)
}

// Why a bookmark publication failed.
#[derive(Debug)]
pub(crate) enum PinPublishError {
    // Serialization refused; no bytes were staged or published.
    Serialization(PinRefusal),
    // Publication failed with a typed custody outcome.
    Publication(enzui_core::state_authority::PublishError),
}

impl std::fmt::Display for PinPublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization(r) => write!(f, "serialization refused: {r}"),
            Self::Publication(e) => write!(f, "publication failed: {e}"),
        }
    }
}

impl PinPublishError {
    // The custody outcome. `None` for a pre-publication serialization
    // failure (no bytes were staged).
    pub(crate) fn custody(&self) -> Option<enzui_core::state_authority::PublishCustody> {
        match self {
            Self::Serialization(_) => None,
            Self::Publication(e) => Some(e.custody()),
        }
    }
}

// Compute the canonical default pins (Home + available XDG dirs),
// deduplicated and bounded through [`admit_pins`].
pub(crate) fn default_pins(home: &std::path::Path) -> PinLoadOutcome {
    let places = default_places(home);
    let defaults: Vec<Place> = places
        .into_iter()
        .filter(|p| matches!(p.kind, PlaceKind::Home | PlaceKind::XdgDir))
        .collect();
    admit_pins(defaults)
}

#[cfg(test)]
pub(crate) fn save_bookmarks_to(path: &Path, bookmarks: &[Place]) -> io::Result<()> {
    let body = serialize_bookmarks(bookmarks)
        .map_err(|refusal| io::Error::new(io::ErrorKind::InvalidData, refusal.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::atomic_publish::publish_raw(path, body.as_bytes())
}

#[cfg(test)]
impl PinLoadOutcome {}
