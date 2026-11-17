// Display names and the add-download dialog state.

use super::*;

// A readable name for a URI: the last path segment of an http(s) URL, or the
// `dn=` display-name of a magnet link, falling back to the raw URI.
pub(super) fn safe_display_component(raw: &str) -> Option<String> {
    use percent_encoding::percent_decode_str;
    let plus_decoded = raw.replace('+', " ");
    let decoded = percent_decode_str(&plus_decoded).decode_utf8().ok()?;
    let trimmed = decoded.trim().trim_matches('.');
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '\0'))
    {
        return None;
    }
    let value: String = trimmed.chars().take(120).collect();
    (!value.is_empty()).then_some(value)
}

pub(super) fn display_name(uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("magnet:") {
        for param in rest.trim_start_matches('?').split('&') {
            if let Some(dn) = param.strip_prefix("dn=")
                && let Some(name) = safe_display_component(dn)
            {
                return name;
            }
        }
        return crate::i18n::tr("Torrent download").to_owned();
    }
    let without_scheme = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
        .unwrap_or(uri);
    let candidate = without_scheme
        .split(['?', '#'])
        .next()
        .and_then(|authority_and_path| authority_and_path.split_once('/').map(|(_, path)| path))
        .unwrap_or("")
        .trim_end_matches('/')
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("");
    safe_display_component(candidate).unwrap_or_else(|| "Download".to_owned())
}

pub(crate) fn is_enqueueable(uri: &str) -> bool {
    let t = uri.trim();
    if t.is_empty() {
        return false;
    }
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("magnet:")
}

pub(crate) fn is_known_service_url(uri: &str) -> bool {
    let trimmed = uri.trim();
    let Some(rest) = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_and_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => host,
        Some(_) => return false,
        None => host_and_port,
    }
    .trim_end_matches('.')
    .to_ascii_lowercase();
    if host.is_empty()
        || host.split('.').any(|label| {
            label.is_empty()
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return false;
    }
    [
        "youtube.com",
        "youtu.be",
        "youtube-nocookie.com",
        "vimeo.com",
        "soundcloud.com",
    ]
    .into_iter()
    .any(|domain| host == domain || host.ends_with(&format!(".{domain}")))
}

// The Add-download dialog state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AddDownloadState {
    pub(crate) url: String,
    pub(crate) advanced_open: bool,
    pub(crate) speed_limit_kib: String,
    pub(crate) destination_hint: PathBuf,
    pub(crate) error: Option<String>,
    // The download whose destination request is in flight. While set, the dialog
    // stays open and the action is disabled against duplicate requests.
    pub(crate) submitting: Option<DownloadId>,
}

impl Default for AddDownloadState {
    fn default() -> Self {
        Self::new(PathBuf::new(), 0)
    }
}

impl AddDownloadState {
    pub(crate) fn new(destination_hint: PathBuf, speed_limit_kib: u32) -> Self {
        Self {
            url: String::new(),
            advanced_open: false,
            speed_limit_kib: speed_limit_kib.to_string(),
            destination_hint,
            error: None,
            submitting: None,
        }
    }

    pub(crate) fn speed_limit(&self) -> Result<u32, String> {
        let value = self
            .speed_limit_kib
            .trim()
            .parse::<u32>()
            .map_err(|_| crate::i18n::tr("Speed limit must be a whole number").to_owned())?;
        if value > MAX_RATE_KIB {
            Err(crate::i18n::format(
                "files-speed-limit-must-be-at-most-96ba5ae2",
                &[("MAX_RATE_KIB", (MAX_RATE_KIB).to_string())],
            ))
        } else {
            Ok(value)
        }
    }

    // Whether the URL is a media site such as YouTube, which Files does not
    // download from: it would only fetch the page, not the media.
    pub(crate) fn is_media_site(&self) -> bool {
        is_known_service_url(&self.url)
    }

    // Whether `Enqueue` may fire.
    pub(crate) fn can_enqueue(&self, torrent_backend: Aria2Status) -> bool {
        is_enqueueable(&self.url)
            && !self.is_media_site()
            && self.speed_limit().is_ok()
            && (!is_torrent_source(&self.url) || torrent_backend.is_ready())
    }
}
