// Documents from the user's Templates folder.
//
// The folder is `XDG_TEMPLATES_DIR`; pointing it at `$HOME` disables it, per the
// spec. A scan lists only regular, non-hidden files up to [`MAX_TEMPLATE_BYTES`],
// at most [`MAX_TEMPLATES`], sorted by name, without nested folders. It runs off
// the UI thread each time the background menu opens.

use std::path::{Path, PathBuf};

pub(crate) const MAX_TEMPLATES: usize = 32;
// The largest template copied; a bigger file is left out of the list.
pub(crate) const MAX_TEMPLATE_BYTES: u64 = 64 * 1024 * 1024;

// One offered template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Template {
    // The menu label: the file name without its extension.
    pub(crate) label: String,
    pub(crate) path: PathBuf,
}

// What the last scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Templates {
    // Not scanned yet.
    #[default]
    Unscanned,
    // No Templates folder is configured, or it does not exist.
    Absent,
    // The folder exists but could not be read.
    Unreadable(String),
    // The usable templates (possibly none).
    Found(Vec<Template>),
}

pub(crate) fn templates_dir(home: &Path) -> Option<PathBuf> {
    dirs::template_dir().filter(|dir| dir != home)
}

// Scan `dir` for usable templates.
pub(crate) fn scan(dir: Option<&Path>) -> Templates {
    let Some(dir) = dir else {
        return Templates::Absent;
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Templates::Absent,
        Err(error) => {
            return Templates::Unreadable(error.to_string().lines().next().unwrap_or("").into());
        }
    };
    let mut found: Vec<Template> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') || name.ends_with('~') {
                return None;
            }
            // `DirEntry::metadata` does not follow a symlink.
            let meta = entry.metadata().ok()?;
            if !meta.file_type().is_file() || meta.len() > MAX_TEMPLATE_BYTES {
                return None;
            }
            // An empty text template makes the same file as New File.
            if is_blank_text(&entry.path(), &name, meta.len()) {
                return None;
            }
            let label = match name.rsplit_once('.') {
                Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
                _ => name.clone(),
            };
            Some(Template {
                label,
                path: entry.path(),
            })
        })
        .collect();
    found.sort_by_cached_key(|template| {
        (
            enzui_core::palette::normalize(&template.label),
            template.path.clone(),
        )
    });
    found.truncate(MAX_TEMPLATES);
    Templates::Found(found)
}

// The largest text template still read to see whether it is blank.
const BLANK_TEXT_PROBE_BYTES: u64 = 64;

// Whether `path` is a plain-text template (`.txt` or no extension) holding
// nothing but whitespace, which duplicates the built-in New File.
fn is_blank_text(path: &Path, name: &str, len: u64) -> bool {
    let plain = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension.eq_ignore_ascii_case("txt"),
        _ => true,
    };
    if !plain || len > BLANK_TEXT_PROBE_BYTES {
        return false;
    }
    std::fs::read(path).is_ok_and(|bytes| bytes.iter().all(u8::is_ascii_whitespace))
}

// Scan off the UI thread.
pub(crate) fn scan_task(home: PathBuf) -> impl std::future::Future<Output = Templates> + Send {
    crate::shell_work::run(move || scan(templates_dir(&home).as_deref()))
}
