// GVolumeMonitor requires a GLib main loop, so storage operations use bounded helper processes.

mod helper;
mod watch;

use enzui_core::cancellation::CancelToken;
use std::path::{Path, PathBuf};

pub(crate) use watch::stream as watch_stream;

// The largest helper reply, in bytes.
pub(crate) const FRAME_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Kind {
    Disk,
    Usb,
    Optical,
    Network,
    Cloud,
    Phone,
}

// One drive, volume or mount the desktop shows.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Location {
    // A stable identity for operations: `volume:`, `mount:`, `device:` or
    // `unix:` followed by the native identity.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: Kind,
    // Where it is mounted, with the native path bytes.
    #[serde(with = "path_bytes")]
    pub(crate) mount: Option<PathBuf>,
    // Its block device, when it has one.
    #[serde(with = "path_bytes")]
    pub(crate) device: Option<PathBuf>,
    pub(crate) can_mount: bool,
    pub(crate) can_unmount: bool,
    pub(crate) can_eject: bool,
    pub(crate) empty_media: bool,
    // Mounted read-only (an image, a disc, a read-only share): its free space
    // is not writable capacity.
    pub(crate) read_only: bool,
    pub(crate) disk_image: bool,
    // Set when the enzimOS mount service reports the location; never part of
    // the helper protocol.
    #[serde(skip)]
    pub(crate) service: Option<ServiceItem>,
}

// What Files needs to know about a mount service item beyond the common fields.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ServiceItem {
    // An encrypted drive that needs a passphrase before it can be mounted.
    pub(crate) locked: bool,
    // A network or phone mount that stopped answering; only a forced
    // unmount can detach it.
    pub(crate) not_responding: bool,
    // The service is mounting or unmounting it.
    pub(crate) busy: bool,
    // The network address of a network location.
    pub(crate) uri: Option<String>,
    // Why the last attempt failed, or that the server stopped answering.
    pub(crate) reason: Option<String>,
    pub(crate) saved: bool,
    pub(crate) auto_connect: bool,
    pub(crate) mount_at_boot: bool,
    pub(crate) can_mount_at_boot: bool,
    pub(crate) can_remember_password: bool,
    // Free and total bytes, from the service's latest probe.
    pub(crate) space: Option<(u64, u64)>,
    // A writable volume of a drive that can be unplugged: the service may
    // format it.
    pub(crate) can_format: bool,
    // The volume's file system type, such as `exfat` or `iso9660`.
    pub(crate) filesystem: Option<String>,
    pub(crate) formatting: bool,
    // A cloud drive whose browser sign-in is still open.
    pub(crate) signing_in: bool,
    // The last attempt failed; `reason` says why.
    pub(crate) failed: bool,
    // What the drive is, such as "16 GB · ACME Stick".
    pub(crate) detail: Option<String>,
    // The drive can be powered off for safe removal.
    pub(crate) can_power_off: bool,
}

impl Location {
    #[must_use]
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.mount
            .as_ref()
            .is_some_and(|root| path.starts_with(root))
    }
}

// Why a storage operation failed, as a short sentence for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorageError(pub(crate) String);

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StorageError {}

// Serves a helper request when this process was started as the storage
// helper, returning its exit code; `None` for an ordinary launch.
#[must_use]
pub(crate) fn helper_main() -> Option<std::process::ExitCode> {
    helper::mode()
}

// The current inventory, from a one-shot helper.
//
// # Errors
//
// The helper could not run, or answered nonsense.
pub(crate) fn snapshot(cancel: &CancelToken) -> Result<Vec<Location>, StorageError> {
    let bytes = request(helper::SNAPSHOT, None, cancel)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| StorageError(crate::i18n::tr("The storage inventory was invalid").into()))
}

// Mounts the location if needed and returns its root.
//
// # Errors
//
// The location is gone, cannot be mounted, or the user refused.
pub(crate) fn open(id: &str, cancel: &CancelToken) -> Result<PathBuf, StorageError> {
    path_reply(request(helper::OPEN, Some(id.as_bytes().to_vec()), cancel)?)?
        .ok_or_else(|| StorageError(crate::i18n::tr("This location has no local folder").into()))
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum OpenError {
    PassphraseRequired,
    PassphraseRejected(String),
    Failed(String),
}

pub(crate) fn open_authenticated(
    id: &str,
    passphrase: Option<enzui_core::form::SecretText>,
    cancel: &CancelToken,
) -> Result<PathBuf, OpenError> {
    let input_len = 4usize
        .checked_add(id.len())
        .and_then(|length| {
            length.checked_add(passphrase.as_ref().map_or(0, |p| p.expose_secret().len()))
        })
        .filter(|length| *length <= 65_536)
        .ok_or_else(|| OpenError::Failed(crate::i18n::tr("Invalid storage request").into()))?;
    let mut input = Vec::with_capacity(input_len);
    let length = u32::try_from(id.len())
        .map_err(|_| OpenError::Failed("Invalid storage identity".into()))?;
    input.extend_from_slice(&length.to_be_bytes());
    input.extend_from_slice(id.as_bytes());
    if let Some(passphrase) = passphrase {
        input.extend_from_slice(passphrase.expose_secret().as_bytes());
    }
    let bytes = request(helper::OPEN_AUTHENTICATED, Some(input), cancel)
        .map_err(|error| OpenError::Failed(error.to_string()))?;
    let reply: Result<Vec<u8>, OpenError> = serde_json::from_slice(&bytes).map_err(|_| {
        OpenError::Failed(crate::i18n::tr("The storage service answered nonsense").into())
    })?;
    use std::os::unix::ffi::OsStringExt;
    reply.map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

// Unmounts the location.
//
// # Errors
//
// The location is gone, busy, cannot be unmounted, or the user refused.
pub(crate) fn unmount(id: &str, cancel: &CancelToken) -> Result<(), StorageError> {
    path_reply(request(
        helper::UNMOUNT,
        Some(id.as_bytes().to_vec()),
        cancel,
    )?)
    .map(|_| ())
}

// Mounts the ISO image at `path` read-only and returns its root, reusing an
// existing mount of the same image.
//
// # Errors
//
// Not an image, not a regular file, no UDisks2, refused, or unmountable.
pub(crate) fn mount_image(path: &Path, cancel: &CancelToken) -> Result<PathBuf, StorageError> {
    use std::os::unix::ffi::OsStrExt;
    path_reply(request(
        helper::MOUNT_IMAGE,
        Some(path.as_os_str().as_bytes().to_vec()),
        cancel,
    )?)?
    .ok_or_else(|| StorageError(crate::i18n::tr("The image has no local folder").into()))
}

// Whether `path` names a disk image [`mount_image`] accepts.
#[must_use]
pub(crate) fn is_disk_image(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("iso"))
}

fn path_reply(bytes: Vec<u8>) -> Result<Option<PathBuf>, StorageError> {
    use std::os::unix::ffi::OsStringExt;
    let reply: Result<Option<Vec<u8>>, String> = serde_json::from_slice(&bytes).map_err(|_| {
        StorageError(crate::i18n::tr("The storage service answered nonsense").into())
    })?;
    reply
        .map(|path| path.map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes))))
        .map_err(StorageError)
}

fn request(
    mode: &str,
    input: Option<Vec<u8>>,
    cancel: &CancelToken,
) -> Result<Vec<u8>, StorageError> {
    use enzui_services::process_transport::{self, ProcessRequest, StdinPolicy};
    let Ok(binary) = std::env::current_exe() else {
        drop(input.map(enzui_security::secret::Secret::new));
        return Err(StorageError(
            crate::i18n::tr("The storage service could not start").into(),
        ));
    };
    let deadline =
        std::time::Duration::from_secs(if mode == helper::MOUNT_IMAGE { 120 } else { 40 });
    let request = ProcessRequest::new(
        binary.into_os_string(),
        input.map_or(StdinPolicy::Null, StdinPolicy::Bytes),
        FRAME_LIMIT,
        4096,
        deadline,
    )
    .args([std::ffi::OsString::from(mode)])
    .inherit_environment();
    let output = process_transport::run(request, cancel).map_err(|_| {
        StorageError(crate::i18n::tr("The storage service did not complete the operation").into())
    })?;
    if !output.status.success() {
        return Err(StorageError(
            crate::i18n::tr("The storage service could not complete the operation").into(),
        ));
    }
    Ok(output.stdout)
}

// Merges duplicate reports of one storage: GIO metadata wins over a raw
// mount of the same root or device; a mount instance names its overmounted
// root; a cloud folder stays a cloud folder; labels never merge devices.
pub(crate) fn normalize(mut locations: Vec<Location>) -> Vec<Location> {
    let mount_instances: std::collections::HashMap<_, _> = locations
        .iter()
        .filter(|item| item.id.starts_with("unix:"))
        .filter_map(|item| Some((item.mount.clone()?, item.id.clone())))
        .collect();
    for item in &mut locations {
        if item.id.starts_with("mount:")
            && let Some(id) = item
                .mount
                .as_ref()
                .and_then(|root| mount_instances.get(root))
        {
            item.id.clone_from(id);
        }
    }
    let device_roots: std::collections::HashMap<_, _> = locations
        .iter()
        .filter_map(|item| Some((item.device.clone()?, item.mount.clone()?)))
        .collect();
    for item in &mut locations {
        if item.mount.is_none()
            && let Some(root) = item
                .device
                .as_ref()
                .and_then(|device| device_roots.get(device))
        {
            item.mount = Some(root.clone());
            item.can_mount = false;
            item.can_unmount = true;
        }
    }
    let cloud_roots: std::collections::HashSet<_> = locations
        .iter()
        .filter(|item| item.kind == Kind::Cloud)
        .filter_map(|item| item.mount.clone())
        .collect();
    for item in &mut locations {
        if matches!(item.kind, Kind::Disk | Kind::Usb)
            && item
                .mount
                .as_ref()
                .is_some_and(|root| cloud_roots.contains(root))
        {
            item.kind = Kind::Cloud;
        }
    }
    let mut ids = std::collections::HashSet::new();
    let mut mounts = std::collections::HashSet::new();
    let mut devices = std::collections::HashSet::new();
    locations.retain(|item| {
        !item.name.is_empty()
            && ids.insert(item.id.clone())
            && item
                .mount
                .as_ref()
                .is_none_or(|root| mounts.insert(root.clone()))
            && item
                .device
                .as_ref()
                .is_none_or(|device| devices.insert(device.clone()))
    });
    locations.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    locations
}

mod path_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;

    #[expect(
        clippy::ref_option,
        reason = "serde with adapters receive a reference to the field type"
    )]
    pub(crate) fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_ref()
            .map(|path| path.as_os_str().as_bytes())
            .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<Vec<u8>>::deserialize(deserializer)
            .map(|path| path.map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes))))
    }
}

#[cfg(test)]
mod tests;
