// Read-only disk-image mounting through UDisks2, given an open descriptor.

use gio::prelude::*;
use glib::variant::ToVariant;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn proxy(path: &str, interface: &str) -> Result<gio::DBusProxy, String> {
    gio::DBusProxy::for_bus_sync(
        gio::BusType::System,
        gio::DBusProxyFlags::NONE,
        None,
        "org.freedesktop.UDisks2",
        path,
        interface,
        gio::Cancellable::NONE,
    )
    .map_err(|error| error.to_string())
}

// Mounts the image at `path` read-only, or returns where it already is.
pub(super) fn mount(path: &Path) -> Result<Option<PathBuf>, String> {
    mount_native(path).map_err(|error| mount_error_text(&error))
}

fn mount_native(path: &Path) -> Result<Option<PathBuf>, String> {
    if !crate::desktop_storage::is_disk_image(path) {
        return Err(crate::i18n::tr("Not an ISO disk image").into());
    }
    let file = crate::services::open_regular_file(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(crate::i18n::tr("The disk image must be a regular file").into());
    }
    if let Some(root) = mounted_image(path)? {
        return Ok(Some(root));
    }
    let manager = proxy(
        "/org/freedesktop/UDisks2/Manager",
        "org.freedesktop.UDisks2.Manager",
    )?;
    let fds = gio::UnixFDList::new();
    let index = {
        use std::os::fd::AsFd as _;
        fds.append(file.as_fd())
            .map_err(|error| error.to_string())?
    };
    let options = HashMap::from([("read-only", true.to_variant())]);
    let (response, _) = manager
        .call_with_unix_fd_list_sync(
            "LoopSetup",
            Some(&(glib::variant::Handle(index), options).to_variant()),
            gio::DBusCallFlags::NONE,
            30_000,
            Some(&fds),
            gio::Cancellable::NONE,
        )
        .map_err(|error| error.to_string())?;
    let object = response
        .child_value(0)
        .str()
        .ok_or_else(|| crate::i18n::tr("Invalid disk-image response"))?
        .to_owned();
    let result = (|| {
        let loop_device = proxy(&object, "org.freedesktop.UDisks2.Loop")?;
        let empty = HashMap::<String, glib::Variant>::new();
        let filesystem = proxy(&object, "org.freedesktop.UDisks2.Filesystem")?;
        let mounted = filesystem
            .call_sync(
                crate::i18n::tr("Mount"),
                Some(
                    &(HashMap::from([(
                        "options",
                        "ro,nodev,nosuid,noexec".to_variant(),
                    )]),)
                        .to_variant(),
                ),
                gio::DBusCallFlags::NONE,
                30_000,
                gio::Cancellable::NONE,
            )
            .map_err(|error| error.to_string())?;
        let root = mounted
            .child_value(0)
            .str()
            .ok_or_else(|| crate::i18n::tr("Invalid image mount point"))?
            .to_owned();
        // Autoclear only once mounted: the loop then goes away with the mount.
        let _ = loop_device
            .call_sync(
                "SetAutoclear",
                Some(&(true, empty).to_variant()),
                gio::DBusCallFlags::NONE,
                5_000,
                gio::Cancellable::NONE,
            )
            .map_err(|error| error.to_string())?;
        Ok(Some(PathBuf::from(root)))
    })();
    if result.is_err()
        && let Ok(loop_device) = proxy(&object, "org.freedesktop.UDisks2.Loop")
    {
        let empty = HashMap::<String, glib::Variant>::new();
        if let Ok(filesystem) = proxy(&object, "org.freedesktop.UDisks2.Filesystem") {
            let _ = filesystem.call_sync(
                crate::i18n::tr("Unmount"),
                Some(&(empty.clone(),).to_variant()),
                gio::DBusCallFlags::NONE,
                5_000,
                gio::Cancellable::NONE,
            );
        }
        let _ = loop_device.call_sync(
            crate::i18n::tr("Delete"),
            Some(&(empty,).to_variant()),
            gio::DBusCallFlags::NONE,
            5_000,
            gio::Cancellable::NONE,
        );
    }
    result
}

// Where an existing loop device backed by `path` is mounted, if anywhere.
fn mounted_image(path: &Path) -> Result<Option<PathBuf>, String> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    type Properties = HashMap<String, glib::Variant>;
    type Objects = HashMap<glib::variant::ObjectPath, HashMap<String, Properties>>;
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    let manager = proxy(
        "/org/freedesktop/UDisks2",
        "org.freedesktop.DBus.ObjectManager",
    )?;
    let reply = manager
        .call_sync(
            "GetManagedObjects",
            None,
            gio::DBusCallFlags::NONE,
            5_000,
            gio::Cancellable::NONE,
        )
        .map_err(|error| error.to_string())?;
    let (objects,) = reply
        .get::<(Objects,)>()
        .ok_or_else(|| crate::i18n::tr("Invalid storage inventory"))?;
    for (_, interfaces) in objects
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>()
    {
        let backing = interfaces
            .get("org.freedesktop.UDisks2.Loop")
            .and_then(|props| props.get("BackingFile"))
            .and_then(glib::Variant::get::<Vec<u8>>);
        if backing.as_ref().is_some_and(|bytes| {
            bytes.strip_suffix(&[0]).unwrap_or(bytes) == canonical.as_os_str().as_bytes()
        }) {
            let roots = interfaces
                .get("org.freedesktop.UDisks2.Filesystem")
                .and_then(|props| props.get("MountPoints"))
                .and_then(glib::Variant::get::<Vec<Vec<u8>>>);
            if let Some(root) = roots.and_then(|roots| roots.into_iter().next()) {
                let root = root.strip_suffix(&[0]).unwrap_or(&root).to_vec();
                return Ok(Some(PathBuf::from(std::ffi::OsString::from_vec(root))));
            }
        }
    }
    Ok(None)
}

// A user-facing sentence for a native failure, without D-Bus details.
pub(in crate::desktop_storage) fn mount_error_text(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("notauthorized")
        || lower.contains("permission denied")
        || lower.contains("not authorized")
    {
        crate::i18n::tr("permission to mount the image was denied").into()
    } else if lower.contains("serviceunknown") || lower.contains("udisks2 is unavailable") {
        crate::i18n::tr("the desktop disk service is unavailable; install or start UDisks2").into()
    } else if lower.contains("cancelled") || lower.contains("canceled") {
        crate::i18n::tr("mounting was cancelled").into()
    } else if lower.contains("no such interface") || lower.contains("no filesystem") {
        crate::i18n::tr("the image does not contain a mountable filesystem").into()
    } else if lower.contains("no such file") {
        crate::i18n::tr("the disk image was moved or deleted").into()
    } else if lower.contains("not an iso") || lower.contains("regular file") {
        error.to_owned()
    } else {
        crate::i18n::tr(
            "the image could not be mounted; it may be damaged or use an unsupported filesystem",
        )
        .into()
    }
}
