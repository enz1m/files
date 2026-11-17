// Optional feature-module registry and durable enablement state.
//
// Four boolean singleton keys. Loading admits through a compact
// byte/line/token policy via [`StateAuthority::read_bounded`] and parses
// only a completely admitted UTF-8 document. Any violation refuses the
// whole document to safe defaults with a path-free notice.

use std::collections::BTreeMap;
use std::path::PathBuf;

use enzui_core::state_authority::{CustodySnapshot, PublishCustody, StateAuthority, StateIdentity};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum FeatureId {
    Thumbnails,
    Downloads,
    Network,
    Converters,
}

impl FeatureId {
    pub(crate) const ALL: [FeatureId; 4] = [
        Self::Thumbnails,
        Self::Downloads,
        Self::Network,
        Self::Converters,
    ];
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Thumbnails => "thumbnails",
            Self::Downloads => "downloads",
            Self::Network => "network",
            Self::Converters => "converters",
        }
    }
    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|id| id.id() == raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FeatureDescriptor {
    pub(crate) id: FeatureId,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) default_enabled: bool,
}

pub(crate) const REGISTRY: [FeatureDescriptor; 4] = [
    FeatureDescriptor {
        id: FeatureId::Thumbnails,
        name: "File previews",
        description: "Image, video, and PDF thumbnails in the grid and details inspector.",
        default_enabled: true,
    },
    FeatureDescriptor {
        id: FeatureId::Downloads,
        name: "Downloads",
        description: "Download manager, transfer queue, and background transfers.",
        default_enabled: true,
    },
    FeatureDescriptor {
        id: FeatureId::Network,
        name: "Network",
        description: "Discover, connect to, mount, and unmount network shares through GVfs.",
        default_enabled: true,
    },
    FeatureDescriptor {
        id: FeatureId::Converters,
        name: "Converters",
        description: "File conversion actions (not installed in this build).",
        default_enabled: false,
    },
];

// Encoded byte cap; the canonical four-key document is 61 bytes.
pub(crate) const MAX_ENCODED_BYTES: usize = 512;
pub(crate) const MAX_LINES: usize = 8;
pub(crate) const MAX_LINE_BYTES: usize = 64;
pub(crate) const MAX_KEY_BYTES: usize = 32;
pub(crate) const MAX_VALUE_BYTES: usize = 16;
pub(crate) const MAX_TOKENS: usize = 8;
pub(crate) const MAX_AGGREGATE_RETAINED_BYTES: usize = 128;

pub(crate) const FEATURES_REFUSED_TITLE: &str = "Feature settings could not be loaded";

// Path-free refusal reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum FeaturesRefusal {
    TooLarge,
    TooManyLines,
    LineTooLong,
    MalformedLine,
    InvalidKey,
    ValueTooLong,
    MalformedBoolean,
    DuplicateKey,
    TooManyTokens,
    AggregateOverflow,
    EmbeddedControl,
    InvalidUtf8,
    ReadFailed,
    ConcurrentMutation,
    AuthorityUnavailable,
    // Custody permanently lost; no further reads or writes.
    CustodyLost,
}

impl FeaturesRefusal {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::TooLarge => crate::i18n::tr("the feature settings file exceeds the size limit"),
            Self::TooManyLines => crate::i18n::tr("the feature settings file has too many lines"),
            Self::LineTooLong => crate::i18n::tr("a feature settings line is too long"),
            Self::MalformedLine => crate::i18n::tr("a feature settings line is malformed"),
            Self::InvalidKey => crate::i18n::tr("a feature settings key is invalid"),
            Self::ValueTooLong => crate::i18n::tr("a feature settings value is too long"),
            Self::MalformedBoolean => crate::i18n::tr("a feature setting is not a valid boolean"),
            Self::DuplicateKey => crate::i18n::tr("a feature settings key appears more than once"),
            Self::TooManyTokens => {
                crate::i18n::tr("the feature settings file has too many entries")
            }
            Self::AggregateOverflow => {
                crate::i18n::tr("the aggregate feature settings size exceeds the limit")
            }
            Self::EmbeddedControl => {
                crate::i18n::tr("a feature settings value contains control characters")
            }
            Self::InvalidUtf8 => crate::i18n::tr("the feature settings file is not valid UTF-8"),
            Self::ReadFailed => crate::i18n::tr("the feature settings file could not be read"),
            Self::ConcurrentMutation => {
                crate::i18n::tr("the feature settings file changed during loading")
            }
            Self::AuthorityUnavailable => {
                crate::i18n::tr("the feature settings file could not be opened")
            }
            Self::CustodyLost => crate::i18n::tr("feature settings custody has been lost"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Features {
    enabled: BTreeMap<FeatureId, bool>,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            enabled: REGISTRY.iter().map(|d| (d.id, d.default_enabled)).collect(),
        }
    }
}

impl Features {
    pub(crate) fn enabled(&self, id: FeatureId) -> bool {
        self.enabled.get(&id).copied().unwrap_or(false)
    }

    pub(crate) fn toggle(&mut self, id: FeatureId) -> bool {
        let enabled = !self.enabled(id);
        self.enabled.insert(id, enabled);
        enabled
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut body = String::new();
        for descriptor in REGISTRY {
            body.push_str(descriptor.id.id());
            body.push('=');
            body.push_str(if self.enabled(descriptor.id) {
                "true"
            } else {
                "false"
            });
            body.push('\n');
        }
        body.into_bytes()
    }
}

fn is_control_byte(b: u8) -> bool {
    b < 0x20 || b == 0x7F
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub(crate) fn parse_bounded_features(raw: &[u8]) -> Result<Features, FeaturesRefusal> {
    let text = std::str::from_utf8(raw).map_err(|_| FeaturesRefusal::InvalidUtf8)?;

    let mut features = Features::default();
    let mut line_count: usize = 0;
    let mut token_count: usize = 0;
    let mut aggregate_retained: usize = 0;
    let mut seen_keys = BTreeMap::<&str, usize>::new();

    let lines: Vec<&str> = text.split('\n').collect();
    let total_lines = lines.len();
    for (idx, line) in lines.iter().enumerate() {
        if line.is_empty() && idx == total_lines - 1 {
            continue;
        }
        line_count += 1;
        if line_count > MAX_LINES {
            return Err(FeaturesRefusal::TooManyLines);
        }
        let trimmed = line.trim_end_matches('\r');
        if trimmed.len() > MAX_LINE_BYTES {
            return Err(FeaturesRefusal::LineTooLong);
        }
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(FeaturesRefusal::MalformedLine);
        };
        if key.len() > MAX_KEY_BYTES || !is_valid_key(key) {
            return Err(FeaturesRefusal::InvalidKey);
        }
        if value.len() > MAX_VALUE_BYTES {
            return Err(FeaturesRefusal::ValueTooLong);
        }
        if value.bytes().any(is_control_byte) {
            return Err(FeaturesRefusal::EmbeddedControl);
        }
        token_count += 1;
        if token_count > MAX_TOKENS {
            return Err(FeaturesRefusal::TooManyTokens);
        }
        aggregate_retained = aggregate_retained
            .checked_add(key.len())
            .and_then(|n| n.checked_add(value.len()))
            .ok_or(FeaturesRefusal::AggregateOverflow)?;
        if aggregate_retained > MAX_AGGREGATE_RETAINED_BYTES {
            return Err(FeaturesRefusal::AggregateOverflow);
        }
        if FeatureId::parse(key).is_some() {
            if seen_keys.contains_key(key) {
                return Err(FeaturesRefusal::DuplicateKey);
            }
            seen_keys.insert(key, idx);
        }

        if let Some(id) = FeatureId::parse(key) {
            let bool_val = match value.trim() {
                "true" => true,
                "false" => false,
                _ => return Err(FeaturesRefusal::MalformedBoolean),
            };
            features.enabled.insert(id, bool_val);
        }
    }

    Ok(features)
}

// Persistence mode for feature state. Production carries a held authority or
// a boot-time refusal; tests are disarmed.
#[derive(Debug)]
pub(crate) enum FeaturesPersistence {
    Held(StateAuthority),
    Unavailable {
        path: Option<PathBuf>,
        refusal: FeaturesRefusal,
    },
    #[cfg(test)]
    Disarmed,
}

impl FeaturesPersistence {
    pub(crate) fn read_bounded(&self) -> Result<CustodySnapshot, FeaturesRefusal> {
        match self {
            Self::Held(auth) => auth.read_bounded(MAX_ENCODED_BYTES).map_err(|e| match e {
                enzui_core::state_authority::ReadError::ConcurrentMutation => {
                    FeaturesRefusal::ConcurrentMutation
                }
                _ => FeaturesRefusal::ReadFailed,
            }),
            Self::Unavailable { refusal, .. } => Err(*refusal),
            #[cfg(test)]
            Self::Disarmed => Ok(CustodySnapshot::Absent),
        }
    }

    pub(crate) fn publish(
        &mut self,
        bytes: &[u8],
    ) -> Result<StateIdentity, FeaturesPublishOutcome> {
        match self {
            Self::Held(auth) => auth.publish(bytes).map_err(|err| match err.custody() {
                PublishCustody::Unchanged => FeaturesPublishOutcome::Unchanged,
                PublishCustody::Rotated => FeaturesPublishOutcome::Rotated,
                PublishCustody::Lost => FeaturesPublishOutcome::Lost,
                _ => FeaturesPublishOutcome::Unchanged,
            }),
            Self::Unavailable { .. } => Err(FeaturesPublishOutcome::Unavailable),
            #[cfg(test)]
            Self::Disarmed => Err(FeaturesPublishOutcome::Disarmed),
        }
    }

    // Derive the retained boot path from either Held or Unavailable.
    fn boot_path(&self) -> Option<PathBuf> {
        match self {
            Self::Held(auth) => Some(auth.path().to_path_buf()),
            Self::Unavailable { path, .. } => path.clone(),
            #[cfg(test)]
            Self::Disarmed => None,
        }
    }

    // User-authorized reacquisition for refusal recovery: opens a fresh
    // authority on the boot-captured path. Debt retry must not use this; it
    // owes exact bytes through the rotated authority.
    pub(crate) fn try_reacquire_for_recovery(&mut self) -> bool {
        let Some(path) = self.boot_path() else {
            return false;
        };
        if let Some(parent) = path.parent()
            && enzui_core::secure_dir::ensure_private_dir(parent).is_err()
        {
            return false;
        }
        match StateAuthority::open_or_create_tightening(&path) {
            Ok(auth) => {
                *self = Self::Held(auth);
                true
            }
            Err(_) => false,
        }
    }

    pub(crate) fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    // Writes are blocked when the authority was never acquired, or when
    // custody has been permanently lost.
    pub(crate) fn is_write_blocked(&self) -> bool {
        match self {
            Self::Held(auth) => auth.has_lost_custody(),
            Self::Unavailable { .. } => true,
            #[cfg(test)]
            Self::Disarmed => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FeaturesPublishOutcome {
    Unchanged,
    Rotated,
    Lost,
    Unavailable,
    #[cfg(test)]
    Disarmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FeaturesLoadOutcome {
    Applied,
    Absent,
    Lost,
    Refused(FeaturesRefusal),
}

// Bounded load through persistence. Returns safe defaults and a typed
// outcome on any refusal.
pub(crate) fn load_bounded(persistence: &FeaturesPersistence) -> (Features, FeaturesLoadOutcome) {
    let snapshot = match persistence.read_bounded() {
        Ok(snap) => snap,
        Err(refusal) => {
            return (Features::default(), FeaturesLoadOutcome::Refused(refusal));
        }
    };
    match snapshot {
        CustodySnapshot::Absent => (Features::default(), FeaturesLoadOutcome::Absent),
        CustodySnapshot::Lost => (Features::default(), FeaturesLoadOutcome::Lost),
        CustodySnapshot::Current {
            bytes, overflow, ..
        } => {
            if overflow {
                return (
                    Features::default(),
                    FeaturesLoadOutcome::Refused(FeaturesRefusal::TooLarge),
                );
            }
            match parse_bounded_features(&bytes) {
                Ok(features) => (features, FeaturesLoadOutcome::Applied),
                Err(refusal) => (Features::default(), FeaturesLoadOutcome::Refused(refusal)),
            }
        }
    }
}

pub(crate) fn save_bounded(
    features: &Features,
    persistence: &mut FeaturesPersistence,
) -> Result<StateIdentity, FeaturesPublishOutcome> {
    let bytes = features.encode();
    persistence.publish(&bytes)
}

// Publish canonical defaults through the held authority.
pub(crate) fn publish_defaults(
    persistence: &mut FeaturesPersistence,
) -> Result<StateIdentity, FeaturesPublishOutcome> {
    let defaults = Features::default();
    let bytes = defaults.encode();
    persistence.publish(&bytes)
}

pub(crate) fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("files-app").join("features.conf"))
}

#[cfg(test)]
mod tests {}
