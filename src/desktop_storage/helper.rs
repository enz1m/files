// The GLib side of [`desktop_storage`](super): the helper modes the app's
// own executable serves, on its main thread with the default context.

mod image;
mod unix;

use super::{Kind, Location};
use gio::prelude::*;
use std::cell::Cell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;

pub(super) const MONITOR: &str = "--enzui-storage-monitor";
pub(super) const SNAPSHOT: &str = "--enzui-storage-snapshot";
pub(super) const OPEN_AUTHENTICATED: &str = "--enzui-storage-open-authenticated";
pub(super) const OPEN: &str = "--enzui-storage-open";
pub(super) const UNMOUNT: &str = "--enzui-storage-unmount";
pub(super) const MOUNT_IMAGE: &str = "--enzui-storage-mount-image";

#[cfg(test)]
pub(super) use unix::parse_entry;

// The operation one helper performs on a location.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Open,
    Unmount,
}

pub(super) fn mode() -> Option<std::process::ExitCode> {
    let mode = std::env::args_os().nth(1)?;
    let ok = |success: bool| {
        Some(if success {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        })
    };
    match mode.to_str()? {
        MONITOR => {
            monitor();
            ok(true)
        }
        SNAPSHOT => ok(emit(&collect(&gio::VolumeMonitor::get()))),
        MOUNT_IMAGE => {
            use std::os::unix::ffi::{OsStrExt, OsStringExt};
            let result = read_input().and_then(|bytes| {
                image::mount(&PathBuf::from(std::ffi::OsString::from_vec(bytes)))
                    .map(|path| path.map(|path| path.as_os_str().as_bytes().to_vec()))
            });
            ok(emit(&result))
        }
        OPEN_AUTHENTICATED => ok(emit(&open_authenticated())),
        OPEN | UNMOUNT => {
            use std::os::unix::ffi::OsStrExt;
            let operation = match mode.to_str()? {
                OPEN => Operation::Open,
                _ => Operation::Unmount,
            };
            let result = read_input()
                .and_then(|bytes| {
                    String::from_utf8(bytes)
                        .map_err(|_| crate::i18n::tr("Invalid storage identity").to_owned())
                })
                .and_then(|id| operate(&id, operation, None))
                .map(|path| path.map(|path| path.as_os_str().as_bytes().to_vec()));
            ok(emit(&result))
        }
        _ => None,
    }
}

struct MountAuthentication {
    passphrase: Option<enzui_core::form::SecretText>,
    requested: Cell<bool>,
    answered: Cell<bool>,
}

impl MountAuthentication {
    fn operation(self: &Rc<Self>) -> gio::MountOperation {
        let operation = gio::MountOperation::new();
        operation.set_password_save(gio::PasswordSave::Never);
        let authentication = Rc::clone(self);
        operation.connect_ask_password(move |operation, _, _, _, flags| {
            authentication.requested.set(true);
            if flags.contains(gio::AskPasswordFlags::NEED_PASSWORD)
                && !authentication.answered.replace(true)
                && let Some(passphrase) = &authentication.passphrase
            {
                operation.set_password(Some(passphrase.expose_secret()));
                operation.reply(gio::MountOperationResult::Handled);
            } else {
                operation.reply(gio::MountOperationResult::Aborted);
            }
        });
        operation
    }
}

fn open_authenticated() -> Result<Vec<u8>, super::OpenError> {
    use super::OpenError;
    use std::os::unix::ffi::OsStrExt;
    let bytes = read_secret_input().map_err(OpenError::Failed)?;
    let bytes = bytes.as_bytes();
    let length = bytes
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| OpenError::Failed("Invalid storage request".into()))?
        as usize;
    let id_end = 4usize
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| OpenError::Failed("Invalid storage identity".into()))?;
    let id = std::str::from_utf8(&bytes[4..id_end])
        .map_err(|_| OpenError::Failed("Invalid storage identity".into()))?;
    let passphrase = std::str::from_utf8(&bytes[id_end..])
        .map_err(|_| OpenError::Failed("Invalid passphrase encoding".into()))?;
    if passphrase.contains('\0') {
        return Err(OpenError::PassphraseRejected(
            crate::i18n::tr("Invalid passphrase encoding").into(),
        ));
    }
    let authentication = Rc::new(MountAuthentication {
        passphrase: (!passphrase.is_empty())
            .then(|| enzui_core::form::SecretText::new(passphrase.to_owned())),
        requested: Cell::new(false),
        answered: Cell::new(false),
    });
    let result = operate(id, Operation::Open, Some(Rc::clone(&authentication)));
    if authentication.requested.get() && authentication.passphrase.is_none() {
        return Err(OpenError::PassphraseRequired);
    }
    result
        .map_err(|error| {
            if authentication.passphrase.is_some() {
                OpenError::PassphraseRejected(error.replace(passphrase, "[redacted]"))
            } else {
                OpenError::Failed(error)
            }
        })?
        .map(|path| path.as_os_str().as_bytes().to_vec())
        .ok_or_else(|| {
            OpenError::Failed(crate::i18n::tr("This location has no local folder").into())
        })
}

fn read_secret_input() -> Result<enzui_security::secret::Secret, String> {
    let mut bytes = vec![0u8; 65_537];
    let mut input = std::io::stdin().lock().take(65_537);
    let mut length = 0;
    let result = loop {
        match input.read(&mut bytes[length..]) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                length += n;
                if length == bytes.len() {
                    break Err(());
                }
            }
            Err(_) => break Err(()),
        }
    };
    if result.is_err() {
        enzui_security::secret::wipe_bytes(&mut bytes);
        return Err(crate::i18n::tr("Invalid storage request").into());
    }
    bytes.truncate(length);
    Ok(enzui_security::secret::Secret::new(bytes))
}

// The request's argument, at most 64 KiB, from standard input.
fn read_input() -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let read = std::io::stdin().lock().take(65_537).read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 65_536 {
        return Err(crate::i18n::tr("Invalid storage request").into());
    }
    Ok(bytes)
}

fn emit(value: &impl serde::Serialize) -> bool {
    let Ok(mut bytes) = serde_json::to_vec(value) else {
        return false;
    };
    if bytes.len() > super::FRAME_LIMIT {
        return false;
    }
    bytes.push(b'\n');
    std::io::stdout().lock().write_all(&bytes).is_ok()
}

// Emits a coalesced snapshot whenever the desktop's storage changes, until
// the parent closes standard input or stops reading.
fn monitor() {
    // EOF on the parent's pipe ends the helper, also after a parent crash.
    let _ = std::thread::Builder::new()
        .name("storage-parent".into())
        .spawn(|| {
            let _ = std::io::stdin().read(&mut [0]);
            std::process::exit(0);
        });
    // GVolumeMonitor requires the default context on the main thread.
    let volumes = gio::VolumeMonitor::get();
    let mounts = gio_unix::MountMonitor::get();
    let pending = Rc::new(Cell::new(false));
    let notify = {
        let volumes = volumes.clone();
        move || {
            if pending.replace(true) {
                return;
            }
            let volumes = volumes.clone();
            let pending = std::rc::Rc::clone(&pending);
            glib::idle_add_local_once(move || {
                pending.set(false);
                if !emit(&collect(&volumes)) {
                    std::process::exit(0);
                }
            });
        }
    };
    for signal in [
        "volume-added",
        "volume-removed",
        "volume-changed",
        "mount-added",
        "mount-removed",
        "mount-changed",
        "drive-connected",
        "drive-disconnected",
        "drive-changed",
    ] {
        let notify = notify.clone();
        let _ = volumes.connect_local(signal, false, move |_| {
            notify();
            None
        });
    }
    for signal in ["mounts-changed", "mountpoints-changed"] {
        let notify = notify.clone();
        let _ = mounts.connect_local(signal, false, move |_| {
            notify();
            None
        });
    }
    // Block devices appear before GIO reports their volumes; a udev block
    // event is also a reason to look again.
    let udev = udev::MonitorBuilder::new()
        .and_then(|builder| builder.match_subsystem("block"))
        .and_then(udev::MonitorBuilder::listen);
    if let Ok(udev) = udev {
        use std::os::fd::AsRawFd;
        let notify = notify.clone();
        let _ =
            glib_unix::unix_fd_add_local(udev.as_raw_fd(), glib::IOCondition::IN, move |_, _| {
                if udev.iter().next().is_some() {
                    notify();
                }
                glib::ControlFlow::Continue
            });
    }
    notify();
    glib::MainLoop::new(None, false).run();
}

fn volume_id(volume: &gio::Volume) -> String {
    if let Some(uuid) = volume.uuid() {
        volume.identifier("unix-device").map_or_else(
            || format!("volume:uuid:{uuid}"),
            |device| format!("volume:uuid:{uuid}:device:{device}"),
        )
    } else if let Some(device) = volume.identifier("unix-device") {
        format!("volume:device:{device}")
    } else if let Some(root) = volume.activation_root() {
        format!("volume:uri:{}", root.uri())
    } else {
        format!("volume:name:{}", volume.name())
    }
}

fn drive_id(drive: &gio::Drive) -> String {
    format!(
        "device:{}",
        drive
            .identifier("unix-device")
            .map_or_else(|| drive.name().to_string(), |id| id.to_string())
    )
}

fn icon_kind(icon: &gio::Icon, remote: bool) -> Kind {
    let names = icon
        .downcast_ref::<gio::ThemedIcon>()
        .map(|icon| icon.names().join(" "))
        .unwrap_or_default();
    if names.contains("optical") || names.contains("cdrom") {
        Kind::Optical
    } else if names.contains("phone")
        || names.contains("camera")
        || names.contains("multimedia-player")
    {
        Kind::Phone
    } else if names.contains("usb") || names.contains("removable") {
        Kind::Usb
    } else if names.contains("cloud") || names.contains("google") {
        Kind::Cloud
    } else if remote || names.contains("remote") || names.contains("network") {
        Kind::Network
    } else {
        Kind::Disk
    }
}

fn is_loop(device: Option<&PathBuf>) -> bool {
    device.is_some_and(|device| {
        device
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("loop"))
    })
}

fn collect(monitor: &gio::VolumeMonitor) -> Vec<Location> {
    let read_only = unix::read_only_roots();
    let mut result = Vec::new();
    for volume in monitor.volumes() {
        let mounted = volume.get_mount();
        if mounted
            .as_ref()
            .is_some_and(gio::prelude::MountExt::is_shadowed)
        {
            continue;
        }
        let activation = volume.activation_root();
        let remote = activation.as_ref().is_some_and(|root| !root.is_native());
        let phone = activation.as_ref().is_some_and(|root| {
            let uri = root.uri();
            uri.starts_with("mtp:") || uri.starts_with("gphoto2:")
        });
        let mount = mounted
            .as_ref()
            .and_then(|mount| mount.default_location().path());
        let device = volume
            .identifier("unix-device")
            .map(|device| PathBuf::from(device.as_str()));
        let disk_image = is_loop(device.as_ref());
        result.push(Location {
            id: volume_id(&volume),
            name: volume.name().to_string(),
            kind: if phone {
                Kind::Phone
            } else if disk_image {
                Kind::Optical
            } else {
                icon_kind(&volume.icon(), remote)
            },
            read_only: mount.as_ref().is_some_and(|root| read_only.contains(root)),
            mount,
            disk_image,

            service: None,
            device,
            can_mount: volume.can_mount(),
            can_unmount: mounted
                .as_ref()
                .is_some_and(gio::prelude::MountExt::can_unmount),
            can_eject: volume.can_eject(),
            empty_media: false,
        });
    }
    for mount in monitor.mounts() {
        if mount.is_shadowed() || mount.volume().is_some() {
            continue;
        }
        let root = mount.default_location();
        let path = root.path();
        result.push(Location {
            id: format!("mount:{}", mount.root().uri()),
            name: mount.name().to_string(),
            kind: icon_kind(&mount.icon(), !root.is_native()),
            read_only: path.as_ref().is_some_and(|root| read_only.contains(root)),
            mount: path,
            device: None,
            can_mount: false,
            can_unmount: mount.can_unmount(),
            can_eject: mount.can_eject(),
            empty_media: false,
            disk_image: false,

            service: None,
        });
    }
    for drive in monitor.connected_drives() {
        if !drive.volumes().is_empty() || !drive.is_media_removable() {
            continue;
        }
        result.push(Location {
            id: drive_id(&drive),
            name: drive.name().to_string(),
            kind: icon_kind(&drive.icon(), false),
            mount: None,
            device: drive
                .identifier("unix-device")
                .map(|device| PathBuf::from(device.as_str())),
            can_mount: false,
            can_unmount: false,
            can_eject: drive.can_eject(),
            empty_media: !drive.has_media(),
            read_only: false,
            disk_image: false,

            service: None,
        });
    }
    result.extend(unix::locations());
    super::normalize(result)
}

fn operate(
    id: &str,
    operation: Operation,
    authentication: Option<Rc<MountAuthentication>>,
) -> Result<Option<PathBuf>, String> {
    let monitor = gio::VolumeMonitor::get();
    let locations = collect(&monitor);
    let location = locations
        .iter()
        .find(|location| location.id == id)
        .ok_or_else(|| crate::i18n::tr("This device or mount is no longer available").to_owned())?;
    if operation == Operation::Open
        && let Some(path) = &location.mount
    {
        return Ok(Some(path.clone()));
    }
    if location.empty_media {
        return Err(crate::i18n::tr("No media is inserted in this drive").into());
    }
    if id.starts_with("unix:") {
        let root = location
            .mount
            .as_ref()
            .ok_or_else(|| crate::i18n::tr("The mount is no longer available"))?;
        return match operation {
            Operation::Unmount => unix::unmount(root).map(|()| None),
            Operation::Open => Ok(Some(root.clone())),
        };
    }
    let matching: Vec<_> = monitor
        .volumes()
        .into_iter()
        .filter(|volume| volume_id(volume) == id)
        .collect();
    if matching.len() > 1 {
        return Err(crate::i18n::tr("The desktop reported an ambiguous volume identity").into());
    }
    let volume = matching.into_iter().next();
    let mount = volume
        .as_ref()
        .and_then(gio::prelude::VolumeExt::get_mount)
        .or_else(|| {
            monitor
                .mounts()
                .into_iter()
                .find(|mount| format!("mount:{}", mount.root().uri()) == id)
        });
    if operation == Operation::Open
        && let Some(mount) = &mount
    {
        return mount.default_location().path().map(Some).ok_or_else(|| {
            crate::i18n::tr(
                "This location has no local file path; its GVfs file bridge may be unavailable",
            )
            .into()
        });
    }
    let context = glib::MainContext::default();
    let flags = gio::MountUnmountFlags::NONE;
    let outcome = match operation {
        Operation::Unmount => {
            let mount = mount.ok_or_else(|| crate::i18n::tr("This location is not mounted"))?;
            if !mount.can_unmount() {
                return Err(crate::i18n::tr("This location does not support unmounting").into());
            }
            context
                .block_on(mount.unmount_with_operation_future(flags, None::<&gio::MountOperation>))
        }
        Operation::Open => {
            let volume = volume
                .as_ref()
                .ok_or_else(|| crate::i18n::tr("This location cannot be mounted"))?;
            if !volume.can_mount() {
                return Err(crate::i18n::tr("This location cannot be mounted").into());
            }
            let mount_operation = authentication.as_ref().map(MountAuthentication::operation);
            context
                .block_on(volume.mount_future(gio::MountMountFlags::NONE, mount_operation.as_ref()))
        }
    };
    outcome.map_err(|error| {
        error
            .message()
            .chars()
            .filter(|c| !c.is_control())
            .take(240)
            .collect::<String>()
    })?;
    match operation {
        Operation::Open => volume
            .and_then(|volume| volume.get_mount())
            .and_then(|mount| mount.default_location().path())
            .map(Some)
            .ok_or_else(|| {
                crate::i18n::tr(
                    "This location has no local file path; its GVfs file bridge may be unavailable",
                )
                .into()
            }),
        Operation::Unmount => Ok(None),
    }
}

#[cfg(test)]
mod authentication_tests {}
