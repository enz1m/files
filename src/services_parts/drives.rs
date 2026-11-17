// Drives for the Computer view.

use super::*;

#[allow(unsafe_code, reason = "libc getuid FFI")]
fn current_uid_or_zero() -> u32 {
    // SAFETY: getuid never fails and has no preconditions.
    unsafe { libc::getuid() as u32 }
}

// Enumerate mounted filesystems from `/proc/mounts`, classified and stamped
// with capacity from `statvfs`. Pseudo/kernel mounts are filtered out.
pub(crate) fn list_drives() -> Vec<Drive> {
    let Ok(raw) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let uid = current_uid_or_zero();
    let gvfs_prefix = format!("/run/user/{uid}/gvfs");

    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut out: Vec<Drive> = Vec::new();

    for line in raw.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let source = decode_mount_field(cols[0]);
        let mount = decode_mount_field(cols[1]);
        let fstype = cols[2].to_owned();

        if !seen.insert(mount.clone()) {
            continue;
        }

        let Some(kind) = classify_mount(&source, &mount, &fstype, &gvfs_prefix) else {
            continue;
        };

        let mount_path = PathBuf::from(&mount);
        let (total, free) = if kind == DriveKind::Network || fstype.starts_with("fuse") {
            (0, 0)
        } else {
            statvfs_bytes(&mount_path).unwrap_or((0, 0))
        };
        let used = total.saturating_sub(free);

        let label = if mount == "/" {
            crate::i18n::tr("System").to_owned()
        } else {
            let name = mount_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_owned();
            if name.is_empty() {
                source.clone()
            } else {
                name
            }
        };

        out.push(Drive {
            label,
            source,
            mount: mount_path,
            filesystem: fstype,
            kind,
            total,
            used,
            free,
        });
    }

    sort_and_deduplicate(out)
}

fn sort_and_deduplicate(mut out: Vec<Drive>) -> Vec<Drive> {
    out.sort_by(|a, b| {
        let rank = |d: &Drive| match d.kind {
            DriveKind::System => 0,
            DriveKind::Local => 1,
            DriveKind::Removable => 2,
            DriveKind::Network => 3,
        };
        rank(a).cmp(&rank(b)).then_with(|| a.mount.cmp(&b.mount))
    });
    let mut filesystems = std::collections::HashSet::new();
    out.retain(|drive| {
        if matches!(drive.kind, DriveKind::System | DriveKind::Local) {
            let source = std::fs::canonicalize(&drive.source)
                .unwrap_or_else(|_| PathBuf::from(&drive.source));
            filesystems.insert((source, drive.filesystem.clone()))
        } else {
            true
        }
    });
    out
}

// Decode octal escapes that `/proc/mounts` uses (`\040` = space, etc.).
pub(crate) fn decode_mount_field(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &bytes[i + 1..i + 4];
            if digits.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                let value = u16::from(digits[0] - b'0') * 64
                    + u16::from(digits[1] - b'0') * 8
                    + u16::from(digits[2] - b'0');
                if let Ok(byte) = u8::try_from(value) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn classify_mount(source: &str, mount: &str, fstype: &str, gvfs_prefix: &str) -> Option<DriveKind> {
    if mount == "/" {
        return Some(DriveKind::System);
    }
    // GVfs's FUSE bridge is not a drive: the locations and phones inside it
    // are listed as such, and unmounting the bridge would cut them all off.
    if fstype == "fuse.gvfsd-fuse" {
        return None;
    }
    if mount.starts_with(gvfs_prefix) {
        return Some(DriveKind::Network);
    }
    if matches!(
        fstype,
        "cifs"
            | "smb3"
            | "nfs"
            | "nfs4"
            | "sshfs"
            | "fuse.sshfs"
            | "webdav"
            | "davfs"
            | "fuse.davfs2"
    ) {
        return Some(DriveKind::Network);
    }
    if source.starts_with("//") {
        return Some(DriveKind::Network);
    }
    if source.contains(":/") && !source.starts_with('/') {
        return Some(DriveKind::Network);
    }
    if mount.starts_with("/media/")
        || mount.starts_with("/run/media/")
        || mount.starts_with("/mnt/")
    {
        return Some(DriveKind::Removable);
    }
    if source.starts_with("/dev/") {
        if fstype == "squashfs" {
            return None;
        }
        return Some(DriveKind::Local);
    }
    None
}

// `statvfs(2)` → `(total_bytes, free_bytes)`, using `f_frsize` per POSIX.
#[allow(unsafe_code, reason = "libc statvfs FFI")]
pub(crate) fn statvfs_bytes(path: &Path) -> io::Result<(u64, u64)> {
    use std::ffi::CString;
    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut buf = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: the path is NUL-terminated and buf points to writable statvfs storage.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned success and initialized the output structure.
    let buf = unsafe { buf.assume_init() };
    // statvfs field widths vary by target (c_ulong is 32-bit on some), so the
    // `as u64` casts are needed off this target.
    #[allow(
        clippy::unnecessary_cast,
        reason = "statvfs field widths differ across target ABIs"
    )]
    let block = buf.f_frsize as u64;
    #[allow(
        clippy::unnecessary_cast,
        reason = "statvfs field widths differ across target ABIs"
    )]
    let total = block.saturating_mul(buf.f_blocks as u64);
    #[allow(
        clippy::unnecessary_cast,
        reason = "statvfs field widths differ across target ABIs"
    )]
    let free = block.saturating_mul(buf.f_bavail as u64);
    Ok((total, free))
}
