// Formatting a removable drive through the mount service. Files never
// formats through any other route.

use super::{ServiceError, call};

// A file system the service can create, in the order the dialog offers them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileSystem {
    Exfat,
    Fat32,
    Ntfs,
    Ext4,
}

impl FileSystem {
    pub(crate) const ALL: [Self; 4] = [Self::Exfat, Self::Fat32, Self::Ntfs, Self::Ext4];

    // The service's `type` option.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Exfat => "exfat",
            Self::Fat32 => "vfat",
            Self::Ntfs => "ntfs",
            Self::Ext4 => "ext4",
        }
    }

    // File system names stay untranslated.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Exfat => "exFAT",
            Self::Fat32 => "FAT32",
            Self::Ntfs => "NTFS",
            Self::Ext4 => "ext4",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Exfat => crate::i18n::tr(
                "Works with Windows, macOS, Linux, phones and TVs. The best choice for most drives.",
            ),
            Self::Fat32 => crate::i18n::tr(
                "For older cameras, car stereos and game consoles. Files up to 4\u{a0}GB.",
            ),
            Self::Ntfs => crate::i18n::tr("For drives used mostly with Windows PCs."),
            Self::Ext4 => {
                crate::i18n::tr("For drives used only with Linux. Keeps file permissions.")
            }
        }
    }

    // What a volume name may be, shown under the name field.
    pub(crate) fn name_hint(self) -> &'static str {
        match self {
            Self::Exfat => crate::i18n::tr("Up to 15 characters"),
            Self::Fat32 => {
                crate::i18n::tr("Up to 11 characters without accents; saved in capital letters")
            }
            Self::Ntfs => crate::i18n::tr("Up to 32 characters"),
            Self::Ext4 => crate::i18n::tr("Up to 16 characters, fewer with accented letters"),
        }
    }

    // Why `label` cannot name a volume of this file system, using the
    // service's limits; `None` when it can (an empty name means none).
    pub(crate) fn label_problem(self, label: &str) -> Option<&'static str> {
        if label.chars().any(char::is_control) {
            return Some(crate::i18n::tr(
                "The name cannot contain control characters",
            ));
        }
        let too_long = match self {
            Self::Exfat => label.encode_utf16().count() > 15,
            Self::Fat32 => label.len() > 11,
            Self::Ntfs => label.encode_utf16().count() > 32,
            Self::Ext4 => label.len() > 16,
        };
        if self == Self::Fat32 && (!label.is_ascii() || label.contains(FAT_FORBIDDEN)) {
            return Some(crate::i18n::tr(
                "FAT32 names cannot use accented letters or any of \" * + , . / : ; < = > ? [ \\ ] |",
            ));
        }
        too_long.then(|| match self {
            Self::Exfat => crate::i18n::tr("Too long for exFAT: use up to 15 characters"),
            Self::Fat32 => crate::i18n::tr("Too long for FAT32: use up to 11 characters"),
            Self::Ntfs => crate::i18n::tr("Too long for NTFS: use up to 32 characters"),
            Self::Ext4 => crate::i18n::tr("Too long for ext4: use a shorter name"),
        })
    }
}

// A file system type as the service names it (`vfat`, `crypto_LUKS`), as
// people know it.
pub(crate) fn filesystem_label(key: &str) -> String {
    match key {
        "exfat" => "exFAT".to_owned(),
        "vfat" | "fat" | "fat32" => "FAT32".to_owned(),
        "ntfs" => "NTFS".to_owned(),
        "iso9660" => crate::i18n::tr("Installer image (ISO 9660)").to_owned(),
        "crypto_LUKS" => crate::i18n::tr("Encrypted").to_owned(),
        other => other.to_owned(),
    }
}

// Characters FAT volume labels cannot hold.
const FAT_FORBIDDEN: [char; 16] = [
    '"', '*', '+', ',', '.', '/', ':', ';', '<', '=', '>', '?', '[', '\\', ']', '|',
];

// The Format dialog for one volume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FormatState {
    // The storage id of the volume.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) file_system: FileSystem,
    pub(crate) label: String,
    // Erase every partition of the drive and make one new volume.
    pub(crate) whole_drive: bool,
    pub(crate) phase: FormatPhase,
    // What the drive is ("16 GB · ACME Stick") and its file system now.
    pub(crate) detail: Option<String>,
    pub(crate) current: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FormatPhase {
    // Choosing options; `error` is why the last attempt failed.
    Choosing { error: Option<String> },
    Formatting,
    // Formatted; `id` is the storage id of the new, unmounted volume.
    Done { id: String },
}

impl FormatState {
    // exFAT with the volume's current name when it fits. A stick written
    // with an installer image (ISO 9660) is erased whole by default, since
    // its partitions cannot be reformatted one by one.
    pub(crate) fn new(id: String, name: String, filesystem: Option<&str>) -> Self {
        let file_system = FileSystem::Exfat;
        let label = if file_system.label_problem(&name).is_none() {
            name.clone()
        } else {
            String::new()
        };
        Self {
            id,
            name,
            file_system,
            label,
            whole_drive: filesystem == Some("iso9660"),
            phase: FormatPhase::Choosing { error: None },
            detail: None,
            current: filesystem.map(str::to_owned),
        }
    }

    // The drive's description and its file system now, for the dialog's header.
    pub(crate) fn summary(&self) -> Option<String> {
        let current = self.current.as_deref().map(filesystem_label);
        match (self.detail.clone(), current) {
            (Some(detail), Some(current)) => Some(format!("{detail} · {current}")),
            (detail, current) => detail.or(current),
        }
    }

    pub(crate) fn label_problem(&self) -> Option<&'static str> {
        self.file_system.label_problem(self.label.trim())
    }

    pub(crate) fn request(&self) -> FormatRequest {
        let label = self.label.trim();
        FormatRequest {
            file_system: self.file_system,
            label: if self.file_system == FileSystem::Fat32 {
                label.to_ascii_uppercase()
            } else {
                label.to_owned()
            },
            whole_drive: self.whole_drive,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FormatRequest {
    pub(crate) file_system: FileSystem,
    pub(crate) label: String,
    pub(crate) whole_drive: bool,
}

// Quick-formats the volume `id` (a service id) and returns the service id of
// the formatted volume, which is left unmounted.
pub(crate) async fn format(id: String, request: FormatRequest) -> Result<String, ServiceError> {
    use zbus::zvariant::Value;
    let mut options = std::collections::HashMap::new();
    options.insert("type", Value::from(request.file_system.key()));
    options.insert("label", Value::from(request.label));
    options.insert("whole_drive", Value::from(request.whole_drive));
    call::<_, String>("Format", &(id.as_str(), options)).await
}
