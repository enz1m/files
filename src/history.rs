// Previous versions and backups through the enzimOS history service
// (`org.enzimos.History1` on the system bus): earlier versions of a file,
// a folder as it was, excluding folders from backups, removing files from
// backups, and starting a backup. Without the service none of it is offered.
//
// Old versions arrive as read-only descriptors; Files copies them as the
// person, so restored files are theirs and nothing is written as root.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const NAME: &str = "org.enzimos.History1";
const PATH: &str = "/org/enzimos/History1";
// Removing files from backups waits for the password prompt.
const TIMEOUT: Duration = Duration::from_secs(180);
// A folder restore copies at most this many entries.
const RESTORE_LIMIT: usize = 20_000;
// The most paths one Forget request may name.
pub(crate) const FORGET_LIMIT: usize = 256;
// Text previews read this much.
const PREVIEW_BYTES: usize = 16 * 1024;
// Image previews load files up to this size.
const IMAGE_PREVIEW_BYTES: u64 = 24 * 1024 * 1024;

static AVAILABLE: OnceLock<bool> = OnceLock::new();

// Detect the service once (running or D-Bus activatable). Never in tests.
pub(crate) async fn detect() -> bool {
    if cfg!(test) {
        return false;
    }
    if let Some(found) = AVAILABLE.get() {
        return *found;
    }
    let found = async {
        let connection = bus().await.ok()?;
        let bus = zbus::fdo::DBusProxy::new(&connection).await.ok()?;
        let name = zbus::names::BusName::try_from(NAME).ok()?;
        if bus.name_has_owner(name).await.unwrap_or(false) {
            return Some(true);
        }
        Some(
            bus.list_activatable_names()
                .await
                .ok()?
                .iter()
                .any(|n| n.as_str() == NAME),
        )
    }
    .await
    .unwrap_or(false);
    *AVAILABLE.get_or_init(|| found)
}

// Whether `path` is inside the person's home (history covers homes only).
pub(crate) fn covers(home: &Path, path: &Path) -> bool {
    path.starts_with(home) && path != home && !crate::services::is_virtual(path)
}

// Whether backups leave `path` out, given the person's excluded paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Exclusion {
    Included,
    Excluded,
    // An enclosing folder is excluded.
    Inside(PathBuf),
}

pub(crate) fn exclusion(excluded: &[PathBuf], path: &Path) -> Exclusion {
    if excluded.iter().any(|p| p == path) {
        Exclusion::Excluded
    } else if let Some(folder) = excluded.iter().find(|p| path.starts_with(p)) {
        Exclusion::Inside(folder.clone())
    } else {
        Exclusion::Included
    }
}

fn friendly(error: zbus::Error) -> String {
    let text = match &error {
        zbus::Error::MethodError(_, Some(detail), _) => detail.clone(),
        other => other.to_string(),
    };
    let message = if text.contains("not-authorized") {
        crate::i18n::tr("Not allowed. Only the person signed in at this computer can do this.")
    } else if text.contains("not-found") {
        crate::i18n::tr("That version is no longer kept.")
    } else if text.contains("no-history") {
        crate::i18n::tr("Earlier versions are not kept on this computer.")
    } else if text.contains("no-backup-drive") {
        crate::i18n::tr("No backup drive is set up. Set one up in Settings › Backups.")
    } else if text.contains("busy") {
        crate::i18n::tr("Backups are busy. Try again in a moment.")
    } else {
        return text.chars().filter(|c| !c.is_control()).take(300).collect();
    };
    message.to_owned()
}

// The system bus; the demo stand-in serves on the session bus.
async fn bus() -> zbus::Result<zbus::Connection> {
    if crate::service_demo::enabled() {
        zbus::Connection::session().await
    } else {
        zbus::Connection::system().await
    }
}

async fn proxy() -> Result<zbus::Proxy<'static>, String> {
    let connection = bus().await.map_err(friendly)?;
    zbus::Proxy::new_owned(connection, NAME, PATH, NAME)
        .await
        .map_err(friendly)
}

async fn call<B, R>(method: &str, body: &B) -> Result<R, String>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
{
    let proxy = proxy().await?;
    tokio::time::timeout(TIMEOUT, proxy.call(method, body))
        .await
        .map_err(|_| crate::i18n::tr("The backup service did not answer.").to_owned())?
        .map_err(friendly)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// One earlier version of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    pub(crate) number: u32,
    // When the snapshot was taken (seconds since the epoch).
    pub(crate) taken: i64,
    pub(crate) size: u64,
    pub(crate) modified: i64,
    // Same content as the file now.
    pub(crate) current: bool,
}

// One entry of a folder as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) size: u64,
    pub(crate) modified: i64,
    // Still there now.
    pub(crate) exists: bool,
}

pub(crate) async fn versions(path: PathBuf) -> Result<Vec<Version>, String> {
    let rows: Vec<(u32, i64, u64, i64, bool)> = call("Versions", &(text(&path),)).await?;
    Ok(rows
        .into_iter()
        .map(|(number, taken, size, modified, current)| Version {
            number,
            taken,
            size,
            modified,
            current,
        })
        .collect())
}

pub(crate) async fn snapshots() -> Result<Vec<(u32, i64)>, String> {
    let mut rows: Vec<(u32, i64)> = call("Snapshots", &()).await?;
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    Ok(rows)
}

pub(crate) async fn folder(path: PathBuf, number: u32) -> Result<Vec<Item>, String> {
    let rows: Vec<(String, String, u64, i64, bool)> =
        call("Folder", &(text(&path), number)).await?;
    Ok(rows
        .into_iter()
        .filter(|(_, kind, ..)| kind == "f" || kind == "d")
        .map(|(name, kind, size, modified, exists)| Item {
            name,
            dir: kind == "d",
            size,
            modified,
            exists,
        })
        .collect())
}

async fn open_version(path: &Path, number: u32) -> Result<std::fs::File, String> {
    let fd: zbus::zvariant::OwnedFd = call("Open", &(text(path), number)).await?;
    Ok(std::fs::File::from(std::os::fd::OwnedFd::from(fd)))
}

// The person's excluded folders (absolute paths).
pub(crate) async fn excluded() -> Result<Vec<PathBuf>, String> {
    let (_, paths): (bool, Vec<String>) = call("Rules", &()).await?;
    Ok(paths.into_iter().map(PathBuf::from).collect())
}

pub(crate) async fn set_excluded(path: PathBuf, excluded: bool) -> Result<bool, String> {
    call::<_, ()>("SetExcluded", &(text(&path), excluded))
        .await
        .map(|()| excluded)
}

// Remove files from earlier versions now and from the backup drive at its
// next backup. Returns (entries removed, snapshots changed, queued for the drive).
pub(crate) async fn forget(paths: Vec<PathBuf>) -> Result<(u64, u32, bool), String> {
    call(
        "Forget",
        &(paths.iter().map(|p| text(p)).collect::<Vec<_>>(),),
    )
    .await
}

// Detect the service and read the person's exclusions; `None` = no service.
pub(crate) async fn refresh() -> Option<Vec<PathBuf>> {
    if !detect().await {
        return None;
    }
    Some(excluded().await.unwrap_or_default())
}

pub(crate) async fn back_up_now() -> Result<(), String> {
    call("BackUpNow", &()).await
}

// What a preview shows.
#[derive(Clone, Debug)]
pub(crate) enum Preview {
    Text(String),
    Image(iced::widget::image::Handle),
    None,
}

pub(crate) fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"]
            .contains(&e.to_ascii_lowercase().as_str())
    })
}

pub(crate) async fn preview(path: PathBuf, number: u32) -> Result<Preview, String> {
    let mut file = open_version(&path, number).await?;
    crate::shell_work::run(move || {
        if is_image(&path) {
            let size = file.metadata().map_err(|e| e.to_string())?.len();
            if size > IMAGE_PREVIEW_BYTES {
                return Ok(Preview::None);
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return Ok(Preview::Image(iced::widget::image::Handle::from_bytes(
                bytes,
            )));
        }
        let mut bytes = vec![0u8; PREVIEW_BYTES];
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        bytes.truncate(n);
        Ok(match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => Preview::Text(text),
            _ => Preview::None,
        })
    })
    .await
}

fn system_time(seconds: i64) -> std::time::SystemTime {
    u64::try_from(seconds).map_or(std::time::UNIX_EPOCH, |s| {
        std::time::UNIX_EPOCH + Duration::from_secs(s)
    })
}

// `report (Nov 4, 14.00).txt` beside the original, never an existing name.
pub(crate) fn copy_name(path: &Path, taken: i64) -> PathBuf {
    let when = crate::services::format_time(system_time(taken)).replace([':', '/'], ".");
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    let mut candidate = parent.join(format!("{stem} ({when}){ext}"));
    let mut n = 2;
    while candidate.symlink_metadata().is_ok() {
        candidate = parent.join(format!("{stem} ({when}) {n}{ext}"));
        n += 1;
    }
    candidate
}

// Write `source` to a new file `target` (never replacing one), then give it the
// version's modification time.
fn write_new(mut source: std::fs::File, target: &Path, modified: i64) -> Result<(), String> {
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    std::io::copy(&mut source, &mut out).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    if let Some(when) = u64::try_from(modified)
        .ok()
        .map(|s| std::time::UNIX_EPOCH + Duration::from_secs(s))
    {
        let _ = out.set_modified(when);
    }
    out.sync_all().map_err(|e| e.to_string())
}

// Save an earlier version beside the file as a new copy; returns its path.
pub(crate) async fn save_copy(
    path: PathBuf,
    number: u32,
    taken: i64,
    modified: i64,
) -> Result<PathBuf, String> {
    let source = open_version(&path, number).await?;
    crate::shell_work::run(move || {
        let target = copy_name(&path, taken);
        write_new(source, &target, modified)?;
        Ok(target)
    })
    .await
}

// Copy an earlier version to a private read-only file to open it.
pub(crate) async fn open_copy(
    path: PathBuf,
    number: u32,
    modified: i64,
) -> Result<PathBuf, String> {
    let source = open_version(&path, number).await?;
    crate::shell_work::run(move || {
        let base =
            std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let dir = base
            .join("files-previous-versions")
            .join(format!("{number}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let target = dir.join(path.file_name().ok_or("no file name")?);
        let _ = std::fs::remove_file(&target);
        write_new(source, &target, modified)?;
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o400));
        Ok(target)
    })
    .await
}

// Put an earlier version back: the file there now goes to the Trash first, so
// nothing is lost. Folders are copied entry by entry (bounded).
pub(crate) async fn restore(
    path: PathBuf,
    number: u32,
    home_data_dir: PathBuf,
) -> Result<usize, String> {
    // The tree to copy, listed through the service.
    let mut plan: Vec<(PathBuf, Option<i64>)> = Vec::new();
    let mut folders = Vec::new();
    match folder(path.clone(), number).await {
        Ok(_) => folders.push(path.clone()),
        Err(_) => plan.push((path.clone(), None)),
    }
    let mut dirs = Vec::new();
    while let Some(dir) = folders.pop() {
        dirs.push(dir.clone());
        for item in folder(dir.clone(), number).await? {
            if plan.len() + dirs.len() > RESTORE_LIMIT {
                return Err(crate::i18n::tr(
                    "That folder is too large to restore here. Restore it from Settings › Backups.",
                )
                .to_owned());
            }
            let child = dir.join(&item.name);
            if item.dir {
                folders.push(child);
            } else {
                plan.push((child, Some(item.modified)));
            }
        }
    }
    // Copies are fetched as descriptors first, then written off the UI thread.
    let mut sources = Vec::with_capacity(plan.len());
    for (file, modified) in plan {
        let handle = open_version(&file, number).await?;
        sources.push((file, handle, modified));
    }
    let count = sources.len();
    crate::shell_work::run(move || {
        if path.symlink_metadata().is_ok() {
            crate::trash::move_to_trash_with(&path, &home_data_dir)
                .map_err(|e| e.status_line(&path))?;
        }
        for dir in &dirs {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        for (file, handle, modified) in sources {
            let modified = modified.unwrap_or(0);
            write_new(handle, &file, modified)?;
        }
        Ok(count)
    })
    .await
}

fn date_part(text: &str) -> &str {
    text.split(',').next().unwrap_or(text)
}

// When a version was taken: "Today, 14:00", "Yesterday, 09:00" or the
// framework's short date and time.
pub(crate) fn when(taken: i64, now: i64) -> String {
    let full = crate::services::format_time(system_time(taken));
    let day = date_part(&full).to_owned();
    let clock = crate::services::format_time(system_time(taken));
    let time = clock.rsplit(", ").next().unwrap_or_default().to_owned();
    if day == date_part(&crate::services::format_time(system_time(now))) && time.contains(':') {
        format!("{}, {time}", crate::i18n::tr("Today"))
    } else if day == date_part(&crate::services::format_time(system_time(now - 86_400)))
        && time.contains(':')
    {
        format!("{}, {time}", crate::i18n::tr("Yesterday"))
    } else {
        full
    }
}

// Seconds since the epoch now.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

// The previous-versions dialog.
#[derive(Debug)]
pub(crate) struct VersionsDialog {
    pub(crate) path: PathBuf,
    pub(crate) versions: Option<Result<Vec<Version>, String>>,
    pub(crate) selected: Option<usize>,
    pub(crate) preview: Option<Preview>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

// Browsing a folder as it was.
#[derive(Debug)]
pub(crate) struct BrowseDialog {
    pub(crate) folder: PathBuf,
    pub(crate) snapshots: Option<Result<Vec<(u32, i64)>, String>>,
    // Index into `snapshots` (0 = newest).
    pub(crate) at: usize,
    pub(crate) items: Option<Result<Vec<Item>, String>>,
    pub(crate) chosen: Vec<String>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug)]
pub(crate) enum Dialog {
    Versions(VersionsDialog),
    Browse(BrowseDialog),
}

#[derive(Clone, Debug)]
pub(crate) enum HistoryMsg {
    ShowVersions(PathBuf),
    Browse(PathBuf),
    Versions(Result<Vec<Version>, String>),
    Select(usize),
    Previewed(u32, Result<Preview, String>),
    OpenSelected,
    SaveSelected,
    RestoreSelected,
    Snapshots(Result<Vec<(u32, i64)>, String>),
    Step(bool),
    Items(u32, Result<Vec<Item>, String>),
    Choose(String, bool),
    RestoreChosen,
    Done(Result<String, String>),
    Opened(Result<PathBuf, String>),
    Exclude(PathBuf, bool),
    Excluded(PathBuf, Result<bool, String>),
    BackUpNow,
    BackupStarted(Result<(), String>),
    Forgotten(Result<(u64, u32, bool), String>),
    Refreshed(Option<Vec<PathBuf>>),
}

#[cfg(test)]
mod tests {}
