// Settings validation, persistence, and download settings.

use super::*;

pub(crate) const ARIA2C_INSTALL_HINT: &str = "Install it with `pacman -S aria2` or `apt install aria2` to enable magnet links and .torrent files. HTTP/HTTPS downloads remain available.";

// The backend name carried by [`TransferError::BackendUnavailable`] when
// `aria2c` is missing.
//
// Only an error naming this backend sets [`Aria2Status`] to `Absent` and
// shows the install banner; a magnet link's error names something else.
pub(crate) const ARIA2C_BACKEND: &str = "aria2c";

pub(super) fn io_failure_reason(category: IoFailureCategory) -> &'static str {
    match category {
        IoFailureCategory::Permission => crate::i18n::tr("permission denied"),
        IoFailureCategory::NotFound | IoFailureCategory::NotADirectory => {
            crate::i18n::tr("the destination was not found")
        }
        IoFailureCategory::StorageExhausted => crate::i18n::tr("no storage space remaining"),
        IoFailureCategory::Unknown => crate::i18n::tr("a filesystem error occurred"),
    }
}

pub(crate) const ARIA2C_MISSING_TITLE: &str = "Torrent downloads unavailable";

// Banner title for a transfer that stopped with an error; the body is
// [`TransferError::message`].
pub(crate) const TRANSFER_FAILED_TITLE: &str = "Download failed";

// The empty state's description, under the "No downloads yet" title.
pub(crate) const NO_DOWNLOADS: &str =
    "Use Add download to fetch a file from an HTTP or HTTPS address.";

pub(crate) const ADD_DOWNLOAD_LABEL: &str = "Add download";

pub(crate) const ENQUEUE_LABEL: &str = "Choose destination…";

pub(crate) const ADD_DOWNLOAD_HINT: &str =
    "Paste an HTTP or HTTPS address of a file, or a magnet link.";

// Why a media-site address (YouTube, Vimeo, SoundCloud) cannot be added.
pub(crate) const MEDIA_SITE_UNSUPPORTED: &str =
    "Files downloads files, not videos from media sites such as YouTube.";

pub(crate) const ADD_DOWNLOAD_PLACEHOLDER: &str = "https://… or magnet:?xt=…";
pub(super) const MAX_RATE_KIB: u32 = 1024 * 1024;
pub(super) const MAX_CONCURRENCY: usize = 16;

// Maximum encoded bytes of the settings file. A file exceeding this limit
// is refused as a whole without allocating its content.
pub(crate) const SETTINGS_MAX_ENCODED_BYTES: usize = 8192;

// Maximum physical lines (LF-terminated) admitted in the settings file.
pub(crate) const SETTINGS_MAX_LINES: usize = 32;

// Maximum bytes of a single `key=value` line before the line terminator.
pub(crate) const SETTINGS_MAX_LINE_BYTES: usize = 4096;

pub(crate) const SETTINGS_MAX_KEY_BYTES: usize = 64;

// Maximum encoded bytes of a single value (the part after `=`).
pub(crate) const SETTINGS_MAX_VALUE_BYTES: usize = 4096;

// Maximum percent-decoded bytes of `destination_uri`.
pub(crate) const SETTINGS_MAX_DECODED_URI_BYTES: usize = 4096;

// Maximum aggregate retained bytes across all decoded settings values.
pub(crate) const SETTINGS_MAX_AGGREGATE_RETAINED_BYTES: usize = 8192;

// Banner title when the settings file was refused.
pub(crate) const SETTINGS_REFUSED_TITLE: &str = "Download settings could not be loaded";

// Known singleton keys in `downloads.conf`.
const KNOWN_SETTINGS_KEYS: &[&str] = &[
    "location",
    "destination_uri",
    "max_download_kib",
    "max_upload_kib",
    "concurrency",
    "torrent_policy",
];

// Why a settings source was refused. Carries no path or file content so a
// disclosure cannot leak the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum SettingsRefusal {
    // The file exceeds the encoded-byte ceiling.
    TooLarge,
    // The file contains more physical lines than admitted.
    TooManyLines,
    // A single key=value line exceeds the line-byte ceiling.
    LineTooLong,
    // A nonempty line has no `=` separator.
    MalformedLine,
    // A key exceeds the key-byte ceiling or contains invalid characters.
    InvalidKey,
    // A value exceeds the value-byte ceiling.
    ValueTooLong,
    // A percent-decoded value exceeds the decoded-byte ceiling.
    DecodedValueTooLong,
    // The aggregate retained bytes across all values exceed the ceiling.
    AggregateOverflow,
    // A known singleton key appeared more than once.
    DuplicateKey,
    // A percent escape was malformed (`%` not followed by two hex digits).
    MalformedPercent,
    // A decoded value contains control characters, NUL, or DEL.
    EmbeddedControl,
    // A decoded value is not valid UTF-8.
    InvalidUtf8,
    // `destination_uri` is not a valid absolute local `file://` URI, or
    // contains raw query/fragment/userinfo.
    InvalidDestinationUri,
    // Reading the retained inode failed.
    ReadFailed,
    // In-place mutation detected during the bounded read.
    ConcurrentMutation,
    // An unknown torrent policy would silently default to a more permissive
    // mode.
    UnknownTorrentPolicy,
    // A numeric setting value (rate, concurrency) is not a valid number.
    InvalidNumeric,
    // The production settings authority could not be acquired.
    AuthorityUnavailable,
}

impl SettingsRefusal {
    // A fixed, path-free explanation suitable for a UI banner.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::TooLarge => crate::i18n::tr("the settings file exceeds the size limit"),
            Self::TooManyLines => crate::i18n::tr("the settings file has too many lines"),
            Self::LineTooLong => crate::i18n::tr("a settings line is too long"),
            Self::MalformedLine => crate::i18n::tr("a settings line is malformed"),
            Self::InvalidKey => crate::i18n::tr("a settings key is invalid"),
            Self::ValueTooLong => crate::i18n::tr("a settings value is too long"),
            Self::DecodedValueTooLong => crate::i18n::tr("a decoded settings value is too long"),
            Self::AggregateOverflow => {
                crate::i18n::tr("the aggregate settings size exceeds the limit")
            }
            Self::DuplicateKey => crate::i18n::tr("a settings key appears more than once"),
            Self::MalformedPercent => crate::i18n::tr("a percent escape is malformed"),
            Self::EmbeddedControl => {
                crate::i18n::tr("a settings value contains control characters")
            }
            Self::InvalidUtf8 => crate::i18n::tr("a settings value is not valid UTF-8"),
            Self::InvalidDestinationUri => {
                crate::i18n::tr("the destination URI is not a valid local file path")
            }
            Self::ReadFailed => crate::i18n::tr("the settings file could not be read"),
            Self::ConcurrentMutation => crate::i18n::tr("the settings file changed during loading"),
            Self::UnknownTorrentPolicy => crate::i18n::tr("the torrent policy is not recognized"),
            Self::InvalidNumeric => crate::i18n::tr("a numeric setting is not valid"),
            Self::AuthorityUnavailable => crate::i18n::tr("the settings file could not be opened"),
        }
    }
}

// Whether a byte is a control character, NUL, or DEL in the settings
// admission sense. No control byte is admitted, including tab.
fn is_control_byte(b: u8) -> bool {
    b < 0x20 || b == 0x7F
}

fn has_unicode_control(s: &str) -> bool {
    s.chars().any(char::is_control)
}

// Whether `key` is a valid settings key token: ASCII alphanumeric plus `_`,
// non-empty.
fn is_valid_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// Validates a single percent-encoded value, decoding it and checking all
// admission bounds. Returns the decoded string on success.
fn validate_percent_value(encoded: &str) -> Result<String, SettingsRefusal> {
    if encoded.len() > SETTINGS_MAX_VALUE_BYTES {
        return Err(SettingsRefusal::ValueTooLong);
    }
    let bytes = enzui_core::portal::uri::decode_percent(encoded)
        .map_err(|_| SettingsRefusal::MalformedPercent)?;
    if bytes.len() > SETTINGS_MAX_DECODED_URI_BYTES {
        return Err(SettingsRefusal::DecodedValueTooLong);
    }
    if bytes.iter().any(|&b| is_control_byte(b)) {
        return Err(SettingsRefusal::EmbeddedControl);
    }
    let decoded = String::from_utf8(bytes).map_err(|_| SettingsRefusal::InvalidUtf8)?;
    if has_unicode_control(&decoded) {
        return Err(SettingsRefusal::EmbeddedControl);
    }
    Ok(decoded)
}

// Reject a plain value (not percent-encoded) that contains control bytes.
fn reject_control_value(value: &str) -> Result<(), SettingsRefusal> {
    if value.bytes().any(is_control_byte) || has_unicode_control(value) {
        return Err(SettingsRefusal::EmbeddedControl);
    }
    Ok(())
}

// Check the raw percent-encoded `destination_uri` value for disallowed raw
// query (`?`), fragment (`#`), and userinfo (`@` before the path). These are
// checked on the encoded form so that percent-encoded path bytes `%3F`/`%23`
// pass through the parser.
fn reject_raw_uri_structure(encoded: &str) -> Result<(), SettingsRefusal> {
    if encoded.is_empty() {
        return Ok(());
    }
    let bytes = encoded.as_bytes();
    let mut i = 0;
    let mut past_authority = false;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            i += 3;
            continue;
        }
        match bytes[i] {
            b'?' | b'#' => return Err(SettingsRefusal::InvalidDestinationUri),
            b'/' if !past_authority => past_authority = true,
            b'@' if !past_authority => return Err(SettingsRefusal::InvalidDestinationUri),
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

// Validates a decoded `destination_uri`: must be an absolute local `file://`
// URI. Empty is the only absent value; whitespace-only is invalid.
pub(super) fn validate_destination_uri(decoded: &str) -> Result<(), SettingsRefusal> {
    if decoded.is_empty() {
        return Ok(());
    }
    if decoded.trim().is_empty() {
        return Err(SettingsRefusal::InvalidDestinationUri);
    }
    let path = enzui_core::portal::uri::file_uri_to_path(decoded)
        .map_err(|_| SettingsRefusal::InvalidDestinationUri)?;
    if !path.is_absolute() {
        return Err(SettingsRefusal::InvalidDestinationUri);
    }
    Ok(())
}

// Validate a rate string into a typed value, refusing the document on error.
fn validate_rate(raw: &str) -> Result<Option<NonZeroU32>, SettingsRefusal> {
    let value: u32 = raw
        .trim()
        .parse()
        .map_err(|_| SettingsRefusal::InvalidNumeric)?;
    if value > MAX_RATE_KIB {
        return Err(SettingsRefusal::InvalidNumeric);
    }
    Ok(NonZeroU32::new(value))
}

// Track aggregate retained bytes and refuse on overflow.
fn add_aggregate(current: usize, added: usize) -> Result<usize, SettingsRefusal> {
    let next = current
        .checked_add(added)
        .ok_or(SettingsRefusal::AggregateOverflow)?;
    if next > SETTINGS_MAX_AGGREGATE_RETAINED_BYTES {
        return Err(SettingsRefusal::AggregateOverflow);
    }
    Ok(next)
}

// Parse and fully validate a settings document from raw bytes without
// touching live state; the caller installs the candidate atomically or
// refuses the whole document.
pub(super) fn parse_bounded_settings(raw: &[u8]) -> Result<SettingsCandidate, SettingsRefusal> {
    let text = std::str::from_utf8(raw).map_err(|_| SettingsRefusal::InvalidUtf8)?;
    let mut line_count = 0usize;
    let mut seen_keys: HashMap<&str, usize> = HashMap::new();
    let mut aggregate_retained: usize = 0;

    let mut destination_uri: Option<String> = None;
    let mut max_download_kib_raw: Option<&str> = None;
    let mut max_upload_kib_raw: Option<&str> = None;
    let mut concurrency_raw: Option<&str> = None;
    let mut torrent_policy: Option<TorrentPolicy> = None;

    let lines: Vec<&str> = text.split('\n').collect();
    let total_lines = lines.len();
    for (idx, line) in lines.iter().enumerate() {
        // Only the final empty string from a terminal LF is excluded.
        let is_terminal_artifact = line.is_empty() && idx == total_lines - 1;
        if is_terminal_artifact {
            continue;
        }
        line_count += 1;
        if line_count > SETTINGS_MAX_LINES {
            return Err(SettingsRefusal::TooManyLines);
        }
        let trimmed = line.trim_end_matches('\r');
        if trimmed.len() > SETTINGS_MAX_LINE_BYTES {
            return Err(SettingsRefusal::LineTooLong);
        }
        // Empty lines (blank physical lines) count toward the line limit.
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(SettingsRefusal::MalformedLine);
        };
        if key.len() > SETTINGS_MAX_KEY_BYTES || !is_valid_key(key) {
            return Err(SettingsRefusal::InvalidKey);
        }
        if value.len() > SETTINGS_MAX_VALUE_BYTES {
            return Err(SettingsRefusal::ValueTooLong);
        }
        reject_control_value(value)?;
        // A repeated known singleton key refuses the whole document.
        let is_known = KNOWN_SETTINGS_KEYS.contains(&key);
        if is_known {
            if seen_keys.contains_key(key) {
                return Err(SettingsRefusal::DuplicateKey);
            }
            seen_keys.insert(key, line_count);
        }
        match key {
            // Deprecated key: bounded and ignored.
            "location" => {
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
            }
            "destination_uri" => {
                reject_raw_uri_structure(value)?;
                let decoded = validate_percent_value(value)?;
                validate_destination_uri(&decoded)?;
                aggregate_retained = add_aggregate(aggregate_retained, decoded.len())?;
                destination_uri = Some(decoded);
            }
            "max_download_kib" => {
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
                max_download_kib_raw = Some(value);
            }
            "max_upload_kib" => {
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
                max_upload_kib_raw = Some(value);
            }
            "concurrency" => {
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
                concurrency_raw = Some(value);
            }
            "torrent_policy" => {
                let Some(policy) = TorrentPolicy::parse(value) else {
                    return Err(SettingsRefusal::UnknownTorrentPolicy);
                };
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
                torrent_policy = Some(policy);
            }
            _ => {
                // Unknown key — bounded and ignored after full admission.
                aggregate_retained = add_aggregate(aggregate_retained, value.len())?;
            }
        }
    }
    let max_download_kib = match max_download_kib_raw {
        Some(raw) => validate_rate(raw)?,
        None => None,
    };
    let max_upload_kib = match max_upload_kib_raw {
        Some(raw) => validate_rate(raw)?,
        None => None,
    };
    let concurrency = match concurrency_raw {
        Some(raw) => {
            let v: usize = raw
                .trim()
                .parse()
                .map_err(|_| SettingsRefusal::InvalidNumeric)?;
            if !(1..=MAX_CONCURRENCY).contains(&v) {
                return Err(SettingsRefusal::InvalidNumeric);
            }
            NonZeroUsize::new(v).ok_or(SettingsRefusal::InvalidNumeric)?
        }
        None => DEFAULT_CONCURRENCY,
    };
    Ok(SettingsCandidate {
        destination_uri,
        settings: DownloadSettings {
            max_download_kib,
            max_upload_kib,
            concurrency,
            torrent_policy: torrent_policy.unwrap_or(TorrentPolicy::DownloadOnly),
        },
    })
}

// A fully validated settings candidate ready for atomic installation.
#[derive(Clone, Debug)]
pub(super) struct SettingsCandidate {
    pub(super) destination_uri: Option<String>,
    pub(super) settings: DownloadSettings,
}

// Validates that a destination URI passes the same admission bounds the
// loader requires. Reused before reducer mutation for Apply and portal.
pub(crate) fn validate_destination_uri_for_mutation(uri: &str) -> bool {
    if uri.is_empty() {
        return true;
    }
    if uri.len() > SETTINGS_MAX_DECODED_URI_BYTES {
        return false;
    }
    if uri.bytes().any(is_control_byte) || has_unicode_control(uri) {
        return false;
    }
    validate_destination_uri(uri).is_ok()
}

// Validates that serialized settings bytes would pass the bounded loader.
pub(super) fn validate_settings_for_save(body: &[u8]) -> bool {
    if body.len() > SETTINGS_MAX_ENCODED_BYTES {
        return false;
    }
    parse_bounded_settings(body).is_ok()
}

// How download settings are persisted. A missing authority blocks writes in
// production; the test-disarmed mode is inert.
#[derive(Debug)]
pub(crate) enum SettingsPersistence {
    // Production: authority acquired and held.
    Held(StateAuthority),
    // Production: authority could not be acquired at boot. The boot-captured
    // path is retained so Retry can open a fresh authority; the path never
    // reaches UI.
    Unavailable {
        path: Option<PathBuf>,
        refusal: SettingsRefusal,
    },
    // Test-disarmed: no persistence.
    #[cfg(test)]
    Disarmed,
}

impl SettingsPersistence {
    // Read settings through the retained inode.
    pub(crate) fn read_bounded(&self) -> Result<CustodySnapshot, SettingsRefusal> {
        match self {
            Self::Held(auth) => {
                auth.read_bounded(SETTINGS_MAX_ENCODED_BYTES)
                    .map_err(|e| match e {
                        enzui_core::state_authority::ReadError::ConcurrentMutation => {
                            SettingsRefusal::ConcurrentMutation
                        }
                        _ => SettingsRefusal::ReadFailed,
                    })
            }
            Self::Unavailable { refusal, .. } => Err(*refusal),
            #[cfg(test)]
            Self::Disarmed => Ok(CustodySnapshot::Absent),
        }
    }

    // Publish bytes through the retained authority.
    pub(crate) fn publish(
        &mut self,
        bytes: &[u8],
    ) -> Result<StateIdentity, SettingsPublishOutcome> {
        match self {
            Self::Held(auth) => auth.publish(bytes).map_err(|err| {
                let custody = err.custody();
                match custody {
                    PublishCustody::Unchanged => SettingsPublishOutcome::Unchanged,
                    PublishCustody::Rotated => SettingsPublishOutcome::Rotated,
                    PublishCustody::Lost => SettingsPublishOutcome::Lost,
                    _ => SettingsPublishOutcome::Unchanged,
                }
            }),
            Self::Unavailable { .. } => Err(SettingsPublishOutcome::Unavailable),
            #[cfg(test)]
            Self::Disarmed => Err(SettingsPublishOutcome::Disarmed),
        }
    }

    // Try to acquire a fresh authority at the boot-captured path, becoming
    // `Held` on success and unchanged on failure.
    pub(crate) fn try_reacquire(&mut self) -> bool {
        let (Some(path), _) = (match self {
            Self::Unavailable { path, .. } => (path.as_deref(), true),
            _ => (None, false),
        }) else {
            return false;
        };
        let path = path.to_path_buf();
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
}

// Typed publication outcome without exposing internal paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsPublishOutcome {
    // The destination was not changed.
    Unchanged,
    // The staged inode was published; only durability or late validation
    // failed. The authority owns the new inode.
    Rotated,
    // Custody is lost; no further publications are possible.
    Lost,
    // Production authority was never acquired.
    Unavailable,
    // Test-disarmed: no-op.
    #[cfg(test)]
    Disarmed,
}

// Outcome of a bounded settings load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsLoadOutcome {
    // Settings were admitted and atomically applied.
    Applied,
    // No settings file exists; defaults are in effect.
    Absent,
    // The source was refused; it remains byte-for-byte intact.
    Refused(SettingsRefusal),
    // Custody is lost; no further operations are possible.
    Lost,
}

// Cleanup authority for one paused download.
//
// Non-Clone: the retained authorities (descriptors, directory stages) are
// move-only. Exactly one cleanup authority exists per `(id, attempt)`.
pub(crate) struct PausedCleanupJob {
    pub(crate) id: DownloadId,
    pub(crate) attempt: Attempt,
    // Torrent: shared reference to the identity-bound directory stage.
    // The cleanup worker `take()`s and drops it for descriptor-relative cleanup.
    pub(super) torrent_stage: Option<SharedTorrentStage>,
    // HTTP: the reserved destination whose `remove_if_reserved` is
    // descriptor-identity-bound.
    pub(super) http_reservation: Option<ReservedDownloadDestination>,
}

impl std::fmt::Debug for PausedCleanupJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PausedCleanupJob")
            .field("id", &self.id)
            .field("attempt", &self.attempt)
            .field("has_torrent_stage", &self.torrent_stage.is_some())
            .field("has_http_reservation", &self.http_reservation.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TorrentTelemetry {
    // Stable queue identity of the transfer that produced this sample.
    pub(crate) id: DownloadId,
    // Attempt generation captured when the worker started. A retry keeps
    // `id` but advances this, so a late worker cannot decorate the new attempt.
    pub(crate) attempt: Attempt,
    pub(crate) received: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) rate: Option<String>,
    pub(crate) peers: Option<u32>,
    pub(crate) state: String,
}

impl Default for TorrentTelemetry {
    fn default() -> Self {
        Self {
            id: DownloadId(0),
            attempt: Attempt::FIRST,
            received: None,
            total: None,
            rate: None,
            peers: None,
            state: String::new(),
        }
    }
}

impl TorrentTelemetry {
    pub(super) fn secondary_detail(&self) -> String {
        let amount = match (self.received, self.total) {
            (Some(received), Some(total)) => crate::i18n::format(
                "files-of-f16446b9",
                &[
                    ("arg0", (crate::services::format_bytes(received))),
                    ("arg1", (crate::services::format_bytes(total))),
                ],
            ),
            (Some(received), None) => crate::services::format_bytes(received),
            _ => crate::i18n::tr("Calculating…").into(),
        };
        let rate = self.rate.as_deref().unwrap_or("—");
        let peers = self.peers.map_or_else(|| "—".into(), |n| n.to_string());
        crate::i18n::format(
            "files-peers-712dc6cf",
            &[
                ("arg0", (self.state).clone()),
                ("arg1", (amount)),
                ("arg2", (rate).to_owned()),
                ("arg3", (peers)),
            ],
        )
    }
}

// The torrent policy this backend supports.
//
// Files downloads the payload and stops; it never seeds in the background.
// The persisted type keeps the UI label tied to `aria2c --seed-time=0`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TorrentPolicy {
    #[default]
    DownloadOnly,
}

impl TorrentPolicy {
    pub(super) const PERSISTED_DOWNLOAD_ONLY: &'static str = "download-only";

    fn parse(raw: &str) -> Option<Self> {
        (raw == Self::PERSISTED_DOWNLOAD_ONLY).then_some(Self::DownloadOnly)
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::DownloadOnly => {
                crate::i18n::tr("Download only — stop when complete; do not seed")
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadSettings {
    pub(crate) max_download_kib: Option<NonZeroU32>,
    pub(crate) max_upload_kib: Option<NonZeroU32>,
    pub(crate) concurrency: NonZeroUsize,
    pub(crate) torrent_policy: TorrentPolicy,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DownloadSettingsDraft {
    pub(crate) max_download_kib: String,
    pub(crate) max_upload_kib: String,
    pub(crate) concurrency: String,
    pub(crate) error: Option<String>,
}

impl DownloadSettingsDraft {
    pub(super) fn from_settings(settings: &DownloadSettings) -> Self {
        Self {
            max_download_kib: settings
                .max_download_kib
                .map_or_else(|| "0".into(), |v| v.to_string()),
            max_upload_kib: settings
                .max_upload_kib
                .map_or_else(|| "0".into(), |v| v.to_string()),
            concurrency: settings.concurrency.to_string(),
            error: None,
        }
    }
}

// Default number of HTTP and torrent jobs that may transfer at once.
pub(super) const MAX_ACTIVE: usize = 5;
pub(super) const DEFAULT_CONCURRENCY: NonZeroUsize =
    NonZeroUsize::new(MAX_ACTIVE).expect("positive default concurrency");
