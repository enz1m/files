// Files presentation and task adapters for desktop storage.

use crate::desktop_storage as shared;
use enzui_core::cancellation::CancelToken;
use std::path::{Path, PathBuf};

pub(crate) use crate::mount_service::Inventory;
pub(crate) use shared::{Kind, Location, helper_main as helper_mode, is_disk_image};

// The live storage inventory: from the enzimOS mount service when it is
// installed, otherwise from the GIO monitor helper.
pub(crate) fn subscription() -> iced::Subscription<Result<Inventory, String>> {
    iced::Subscription::run(inventory)
}

fn inventory() -> impl iced::futures::Stream<Item = Result<Inventory, String>> {
    iced::stream::channel(
        1,
        |mut output: iced::futures::channel::mpsc::Sender<Result<Inventory, String>>| async move {
            use iced::futures::{SinkExt, StreamExt};
            if crate::mount_service::available().await {
                crate::mount_service::watch(output).await;
                return;
            }
            let mut monitor = std::pin::pin!(shared::watch_stream());
            while let Some(update) = monitor.next().await {
                let update = update
                    .map(|locations| Inventory {
                        locations,
                        cloud: Vec::new(),
                    })
                    .map_err(|error| error.to_string());
                if output.send(update).await.is_err() {
                    break;
                }
            }
        },
    )
}

pub(crate) fn icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Disk => "hard-drive",
        Kind::Usb => "usb",
        Kind::Optical => "disc",
        Kind::Network => "network",
        Kind::Cloud => "cloud",
        Kind::Phone => "smartphone",
    }
}

// The icon-theme names for a location's device, most specific first; a
// theme without them keeps the glyph.
pub(crate) fn art(location: &Location) -> &'static [&'static str] {
    let device = location
        .device
        .as_deref()
        .and_then(|d| d.file_name())
        .and_then(|d| d.to_str())
        .unwrap_or_default();
    match location.kind {
        Kind::Disk => &["drive-harddisk"],
        Kind::Usb if device.starts_with("mmcblk") => &["media-flash", "drive-removable-media"],
        Kind::Usb => &["drive-removable-media"],
        Kind::Optical => &["drive-optical"],
        Kind::Network => &["folder-remote", "network-server"],
        Kind::Cloud => &["folder-cloud"],
        Kind::Phone if is_camera_name(&location.name) => &["camera-photo", "phone"],
        Kind::Phone => &["phone"],
    }
}

// Whether a portable device's name says it is a camera rather than a phone.
pub(crate) fn is_camera_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "camera", "canon", "nikon", "fujifilm", "olympus", "gopro", "dsc-", "eos ",
    ]
    .iter()
    .any(|word| name.contains(word))
}

// The address of a GVFS mount folder (`smb-share:server=nas,share=backup` is
// `smb://nas/backup`); `None` for any other path.
pub(crate) fn gvfs_address(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    if parent.file_name()? != "gvfs" {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let (backend, params) = name.split_once(':')?;
    let value = |key: &str| {
        params.split(',').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| {
                percent_encoding::percent_decode_str(v)
                    .decode_utf8_lossy()
                    .into_owned()
            })
        })
    };
    let host = value("server").or_else(|| value("host"))?;
    let user = value("user")
        .map(|user| format!("{user}@"))
        .unwrap_or_default();
    let port = value("port")
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let scheme = match backend {
        "smb-share" | "smb-server" => "smb",
        "dav" if value("ssl").as_deref() == Some("true") => "davs",
        other => other,
    };
    let path_part = value("share")
        .or_else(|| value("prefix").map(|prefix| prefix.trim_start_matches('/').to_owned()))
        .map(|rest| format!("/{rest}"))
        .unwrap_or_default();
    Some(format!("{scheme}://{user}{host}{port}{path_part}"))
}

pub(crate) fn description(location: &Location) -> String {
    if let Some(service) = &location.service {
        return service_description(location, service);
    }
    if location.empty_media {
        crate::i18n::tr("No media inserted").into()
    } else if let Some(path) = &location.mount {
        // A share mounted through GVFS reads as its address, not as the
        // `/run/user/…/gvfs/smb-share:server=…` folder that holds it.
        let shown = gvfs_address(path).unwrap_or_else(|| path.display().to_string());
        if location.read_only {
            crate::i18n::format("files-read-only-6320da54", &[("arg0", shown)])
        } else {
            shown
        }
    } else {
        crate::i18n::tr("Not mounted").into()
    }
}

// One line for a mount service location: what is happening to it, or what
// it is (the drive's own description, a network address) and whether it is
// open, joined by " · ".
fn service_description(location: &Location, service: &shared::ServiceItem) -> String {
    let network = matches!(location.kind, Kind::Network | Kind::Cloud);
    if service.formatting {
        return crate::i18n::tr("Formatting…").into();
    }
    if service.signing_in {
        return crate::i18n::tr("Waiting for the sign-in in your browser").into();
    }
    if service.busy {
        return if location.mount.is_some() {
            crate::i18n::tr("Disconnecting…")
        } else if network {
            crate::i18n::tr("Connecting…")
        } else {
            crate::i18n::tr("Mounting…")
        }
        .into();
    }
    if let Some(reason) = &service.reason {
        return reason.clone();
    }
    if location.empty_media {
        return crate::i18n::tr("No media inserted").into();
    }
    // A cloud address (`onedrive:`) means nothing to a person.
    let what = if location.kind == Kind::Network {
        service.uri.clone()
    } else if network {
        None
    } else {
        service.detail.clone()
    };
    let state = if service.locked {
        crate::i18n::tr("Locked")
    } else if location.mount.is_some() {
        if location.read_only {
            crate::i18n::tr("Read-only")
        } else if network {
            crate::i18n::tr("Connected")
        } else {
            return what.unwrap_or_else(|| {
                location
                    .mount
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            });
        }
    } else if network {
        crate::i18n::tr("Not connected")
    } else {
        crate::i18n::tr("Not mounted")
    };
    match what {
        Some(what) => crate::i18n::format(
            "files-state-detail",
            &[("state", state.to_owned()), ("detail", what)],
        ),
        None => state.to_owned(),
    }
}

pub(crate) async fn operate(id: String, unmount: bool) -> Result<Option<PathBuf>, String> {
    crate::shell_work::run(move || operate_blocking(id, unmount)).await
}

pub(crate) async fn open_authenticated(
    id: String,
    passphrase: Option<enzui_core::form::SecretText>,
) -> Result<Option<PathBuf>, crate::mount_service::ServiceError> {
    use crate::mount_service::ServiceError;
    crate::shell_work::run(move || {
        shared::open_authenticated(&id, passphrase, &CancelToken::new())
            .map(Some)
            .map_err(|error| match error {
                shared::OpenError::PassphraseRequired => ServiceError::NeedsPassphrase,
                shared::OpenError::PassphraseRejected(reason) => ServiceError::AuthFailed(reason),
                shared::OpenError::Failed(reason) => ServiceError::Failed(reason),
            })
    })
    .await
}

pub(crate) fn operate_blocking(id: String, unmount: bool) -> Result<Option<PathBuf>, String> {
    if let Some(id) = crate::mount_service::service_id(&id) {
        let operation = crate::mount_service::operate(id.to_owned(), unmount);
        return crate::mount_service::block_on(operation).map_err(|error| error.to_string());
    }
    let cancel = CancelToken::new();
    if unmount {
        shared::unmount(&id, &cancel).map(|()| None)
    } else {
        shared::open(&id, &cancel).map(Some)
    }
    .map_err(|error| error.to_string())
}

pub(crate) fn snapshot() -> Result<Vec<Location>, String> {
    if crate::mount_service::detected() {
        return crate::mount_service::snapshot_blocking();
    }
    shared::snapshot(&CancelToken::new()).map_err(|error| error.to_string())
}

pub(crate) async fn mount_image(path: PathBuf) -> Result<Option<PathBuf>, String> {
    if crate::mount_service::available().await {
        return crate::mount_service::mount_image(path)
            .await
            .map(Some)
            .map_err(|error| error.to_string());
    }
    crate::shell_work::run(move || {
        shared::mount_image(&path, &CancelToken::new())
            .map(Some)
            .map_err(|error| error.to_string())
    })
    .await
}

pub(crate) fn is_fuse_mount(root: &Path) -> bool {
    rustix::fs::statfs(root)
        .is_ok_and(|stats| stats.f_type == libc::FUSE_SUPER_MAGIC as rustix::fs::FsWord)
}

pub(crate) fn capture_fixture(home: &Path) -> Vec<Location> {
    [
        (
            crate::i18n::tr("USB work"),
            Kind::Usb,
            Some(home.join("USB work")),
            false,
            false,
        ),
        (
            crate::i18n::tr("DVD data"),
            Kind::Optical,
            None,
            true,
            false,
        ),
        (
            crate::i18n::tr("DVD drive"),
            Kind::Optical,
            None,
            false,
            true,
        ),
        (
            crate::i18n::tr("External SSD"),
            Kind::Disk,
            Some(home.join("External SSD")),
            false,
            false,
        ),
        (
            crate::i18n::tr("Tresorit Drive"),
            Kind::Cloud,
            Some(home.join("Tresorit Drive")),
            false,
            false,
        ),
    ]
    .into_iter()
    .map(|(name, kind, mount, can_mount, empty_media)| Location {
        id: format!("capture:{name}"),
        name: name.into(),
        kind,
        can_unmount: mount.is_some(),
        mount,
        device: None,
        can_mount,
        empty_media,
        can_eject: kind == Kind::Optical,
        read_only: kind == Kind::Optical,
        disk_image: false,

        service: None,
    })
    .collect()
}
