// MTP enumeration and mounting use the storage owner; file operations use the per-user GVfs bridge.

use std::path::PathBuf;
// One phone or camera advertised by the desktop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Phone {
    // The desktop storage identity used to open this device.
    pub(crate) id: String,
    // The name the device reports.
    pub(crate) name: String,
    // Where its files are reachable as paths, once it is open.
    pub(crate) mount: Option<PathBuf>,
}

// Why a phone could not be opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PhoneError {
    // The phone is locked or not sharing files over USB.
    Locked,
    // The phone is no longer connected.
    Gone,
    // It opened, but GVfs's FUSE bridge is not running, so its files have no
    // path Files can browse.
    NoBridge,
    // It did not answer before the storage operation deadline.
    Timeout,
    // Any other failure, with one bounded line.
    Failed(String),
}

impl std::fmt::Display for PhoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => f.write_str(crate::i18n::tr(
                "unlock the phone and choose File transfer in its USB settings, then open it again",
            )),
            Self::Gone => f.write_str(crate::i18n::tr("the phone is no longer connected")),
            Self::NoBridge => f.write_str(crate::i18n::tr(
                "its files have no folder path here because the GVfs file bridge is not running",
            )),
            Self::Timeout => f.write_str(crate::i18n::tr("the phone did not respond in time")),
            Self::Failed(line) => f.write_str(line),
        }
    }
}

// Two phones, one closed and one open, for the `FILES_APP_OPEN=computer-phones`
// capture seed only.
#[must_use]
pub(crate) fn capture_fixture() -> Vec<Phone> {
    vec![
        Phone {
            id: "mtp://Samsung_Galaxy_S23_R5CW/".into(),
            name: crate::i18n::tr("Galaxy S23").into(),
            mount: Some(PathBuf::from(
                "/run/user/1000/gvfs/mtp:host=Samsung_Galaxy_S23_R5CW",
            )),
        },
        Phone {
            id: "mtp://Google_Pixel_7_1A2B/".into(),
            name: crate::i18n::tr("Pixel 7").into(),
            mount: None,
        },
    ]
}

// Whether `uri` names an MTP device.

// Classify a GIO mount failure from its message.
//
// GVfs's MTP backend reports a locked phone, or one sharing nothing over
// USB, as a device it cannot open or that has no storage; the person's fix
// is the same for both.
#[must_use]
pub(crate) fn classify_open_error(message: &str) -> PhoneError {
    let lower = message.to_lowercase();
    if [
        "locked",
        crate::i18n::tr("unable to open mtp device"),
        crate::i18n::tr("no storage"),
        crate::i18n::tr("no mtp device"),
        crate::i18n::tr("device is busy"),
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return PhoneError::Locked;
    }
    if lower.contains("not found")
        || lower.contains("disconnected")
        || lower.contains("no longer available")
    {
        return PhoneError::Gone;
    }
    let line: String = message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(160)
        .collect();
    PhoneError::Failed(line)
}

// Phones and cameras known to the desktop, open or not.
#[must_use]
pub(crate) fn list() -> Vec<Phone> {
    crate::storage::snapshot()
        .unwrap_or_default()
        .into_iter()
        .filter(|location| location.kind == crate::storage::Kind::Phone)
        .map(|location| Phone {
            id: location.id,
            name: location.name,
            mount: location.mount,
        })
        .collect()
}

// Opens a phone through the desktop storage owner.
//
// # Errors
// Reports disconnected, locked, timed-out devices or an unavailable file bridge.
pub(crate) fn open(id: &str) -> Result<PathBuf, PhoneError> {
    crate::storage::operate_blocking(id.to_owned(), false)
        .map_err(|error| {
            if error.contains("no local file path") {
                PhoneError::NoBridge
            } else if error.to_lowercase().contains("deadline")
                || error.to_lowercase().contains("timed out")
            {
                PhoneError::Timeout
            } else {
                classify_open_error(&error)
            }
        })?
        .ok_or(PhoneError::NoBridge)
}
