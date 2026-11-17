// Mounts GIO does not report: user-owned FUSE and cloud folders, network
// shares at customary roots and `x-gvfs-show` entries, read from
// `/proc/self/mountinfo` with native path bytes.

use super::{Kind, Location, icon_kind};
use gio::prelude::*;
use std::collections::{HashMap, HashSet};
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

pub(super) fn locations() -> Vec<Location> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let fstab: HashMap<_, _> = gio_unix::MountPoint::mount_points()
        .0
        .into_iter()
        .map(|point| {
            (
                point.mount_path(),
                point
                    .options()
                    .map(|options| options.to_string())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let uid = rustix::process::getuid().as_raw();
    entries()
        .into_iter()
        .filter_map(|entry| {
            let root = entry.mount;
            let fs = entry.fs;
            let options = format!(
                "{},{}",
                entry.options,
                fstab.get(&root).map_or("", String::as_str)
            );
            let customary = root.starts_with("/media")
                || root.starts_with("/run/media")
                || root.starts_with("/mnt")
                || (!home.as_os_str().is_empty() && root.starts_with(&home));
            let interesting = customary
                && ["nfs", "nfs4", "cifs", "smb3", "sshfs", "fuse.sshfs"].contains(&fs.as_str());
            let explicit = options.split(',').any(|option| option == "x-gvfs-show");
            if (!explicit && entry.root != Path::new("/"))
                || !visible_unix(&root, &home, &fs, &options, interesting, false, uid)
            {
                return None;
            }
            let name = option_value(&options, "x-gvfs-name=").unwrap_or_else(|| {
                root.file_name().map_or_else(
                    || entry.source.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )
            });
            let icon =
                option_value(&options, "x-gvfs-icon=").map(|name| gio::ThemedIcon::new(&name));
            let kind = if cloud_filesystem(&fs, &entry.source) {
                Kind::Cloud
            } else if fs.contains("nfs") || fs.contains("cifs") || fs.contains("sshfs") {
                Kind::Network
            } else if matches!(fs.as_str(), "iso9660" | "udf") {
                Kind::Optical
            } else if let Some(icon) = icon {
                icon_kind(icon.upcast_ref(), false)
            } else {
                Kind::Disk
            };
            let device = entry
                .source
                .starts_with("/dev/")
                .then(|| PathBuf::from(&entry.source));
            Some(Location {
                id: format!("unix:{}:{}", entry.id, gio::File::for_path(&root).uri()),
                name,
                kind,
                read_only: has_option(&entry.options, "ro"),
                disk_image: super::is_loop(device.as_ref()),

                service: None,
                mount: Some(root),
                device,
                can_mount: false,
                can_unmount: true,
                can_eject: false,
                empty_media: false,
            })
        })
        .collect()
}

// The roots mounted read-only right now.
pub(super) fn read_only_roots() -> HashSet<PathBuf> {
    entries()
        .into_iter()
        .filter(|entry| has_option(&entry.options, "ro"))
        .map(|entry| entry.mount)
        .collect()
}

fn has_option(options: &str, option: &str) -> bool {
    options.split(',').any(|part| part == option)
}

// Unmounts a mount GIO does not own: `fusermount3 -u` (or `fusermount`) for
// FUSE, `umount` for a user mount, bounded and argument-safe.
pub(super) fn unmount(root: &Path) -> Result<(), String> {
    use enzui_services::process_transport::{self, ProcessRequest, StdinPolicy};
    let fuse = entries()
        .iter()
        .rev()
        .find(|entry| entry.mount == root)
        .is_some_and(|entry| entry.fs.starts_with("fuse"));
    let candidates: &[(&str, &[&str])] = if fuse {
        &[
            ("fusermount3", &["-u", "--"]),
            ("fusermount", &["-u", "--"]),
        ]
    } else {
        &[("umount", &["--"])]
    };
    let Some((program, args)) = candidates
        .iter()
        .find_map(|(name, args)| find_program(name).map(|path| (path, *args)))
    else {
        return Err(crate::i18n::tr("The unmount tool is not installed").into());
    };
    let request = ProcessRequest::new(
        program.into_os_string(),
        StdinPolicy::Null,
        4096,
        4096,
        std::time::Duration::from_secs(30),
    )
    .args(
        args.iter()
            .map(std::ffi::OsString::from)
            .chain([root.as_os_str().to_owned()]),
    )
    .inherit_environment();
    let output = process_transport::run(request, &enzui_core::cancellation::CancelToken::new())
        .map_err(|_| crate::i18n::tr("The location could not be unmounted").to_owned())?;
    if output.status.success() {
        Ok(())
    } else if String::from_utf8_lossy(&output.stderr).contains("busy") {
        Err(crate::i18n::tr("The location is busy; close the files it holds open").into())
    } else {
        Err(crate::i18n::tr("The location could not be unmounted").into())
    }
}

fn find_program(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

pub(in crate::desktop_storage) struct Entry {
    pub(crate) id: u64,
    pub(crate) mount: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) source: String,
    pub(crate) fs: String,
    pub(crate) options: String,
}

// Undoes mountinfo's octal escapes (`\040` for a space), keeping raw bytes.
fn decode_field(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                let value = u16::from(digits[0] - b'0') * 64
                    + u16::from(digits[1] - b'0') * 8
                    + u16::from(digits[2] - b'0');
                if let Ok(byte) = u8::try_from(value) {
                    output.push(byte);
                    index += 4;
                    continue;
                }
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    output
}

// One `/proc/self/mountinfo` line, with its optional fields skipped.
pub(in crate::desktop_storage) fn parse_entry(line: &[u8]) -> Option<Entry> {
    let columns: Vec<_> = line
        .split(u8::is_ascii_whitespace)
        .filter(|part| !part.is_empty())
        .collect();
    let separator = columns.iter().position(|column| *column == b"-")?;
    if separator < 6 || columns.len() <= separator + 3 {
        return None;
    }
    Some(Entry {
        id: std::str::from_utf8(columns[0]).ok()?.parse().ok()?,
        mount: PathBuf::from(std::ffi::OsString::from_vec(decode_field(columns[4]))),
        root: PathBuf::from(std::ffi::OsString::from_vec(decode_field(columns[3]))),
        source: String::from_utf8_lossy(&decode_field(columns[separator + 2])).into_owned(),
        fs: String::from_utf8_lossy(columns[separator + 1]).into_owned(),
        options: format!(
            "{},{}",
            String::from_utf8_lossy(columns[5]),
            String::from_utf8_lossy(columns[separator + 3])
        ),
    })
}

// The current instance of every mounted path: an overmount hides the mount
// beneath it.
pub(in crate::desktop_storage) fn active_entries(bytes: &[u8]) -> Vec<Entry> {
    let mut entries: Vec<_> = bytes
        .split(|byte| *byte == b'\n')
        .filter_map(parse_entry)
        .collect();
    let mut roots = HashSet::new();
    entries.reverse();
    entries.retain(|entry| roots.insert(entry.mount.clone()));
    entries
}

fn entries() -> Vec<Entry> {
    active_entries(&std::fs::read("/proc/self/mountinfo").unwrap_or_default())
}

// The GVfs visibility rules for a mount GIO does not report.
pub(in crate::desktop_storage) fn visible_unix(
    root: &Path,
    home: &Path,
    fs: &str,
    options: &str,
    desktop_visible: bool,
    internal: bool,
    uid: u32,
) -> bool {
    if matches!(fs, "fuse.gvfsd-fuse" | "fuse.portal")
        || root == Path::new("/")
        || options.split(',').any(|part| part == "x-gvfs-hide")
    {
        return false;
    }
    if options.split(',').any(|part| part == "x-gvfs-show") {
        return true;
    }
    if internal {
        return false;
    }
    let hidden = root.components().any(
        |part| matches!(part, Component::Normal(name) if name.as_encoded_bytes().starts_with(b".")),
    );
    if hidden {
        return false;
    }
    let owned = options.split(',').any(|option| {
        option
            .strip_prefix("user_id=")
            .and_then(|value| value.parse::<u32>().ok())
            == Some(uid)
    });
    let user_fuse = (fs == "fuse" || fs.starts_with("fuse."))
        && (owned
            || (!home.as_os_str().is_empty() && root.starts_with(home))
            || root.starts_with("/media")
            || root.starts_with("/run/media")
            || root.starts_with("/mnt"));
    desktop_visible || user_fuse
}

fn option_value(options: &str, prefix: &str) -> Option<String> {
    options
        .split(',')
        .find_map(|option| option.strip_prefix(prefix))
        .map(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8_lossy()
                .into_owned()
        })
}

fn cloud_filesystem(fs: &str, source: &str) -> bool {
    let provider = format!("{fs} {source}").to_lowercase();
    [
        "tresorit",
        "rclone",
        "pcloud",
        "onedrive",
        "google-drive",
        "gdrive",
        "filen",
        "proton",
        "koofr",
        "mega",
    ]
    .iter()
    .any(|name| provider.contains(name))
}
