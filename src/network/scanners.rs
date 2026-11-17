// Production share scanner: resolve and run avahi-browse with cleared
// env and structured argv.

use super::{
    DiscoveredShare, HelperOutcome, OperationCancel, ScanResult, ShareScanner, ShareScannerHandle,
    run_network_helper,
};
use std::sync::Arc;

// The production scanner: `avahi-browse` resolved once through the boot
// `SearchPath` and run via `process_transport` with cleared env, structured
// argv and a private cwd.
//
// Overflowing stdout is an `Err`, never a truncated or empty set reported as
// complete.
pub(crate) struct AvahiScanner {
    resolved: Option<crate::exe_resolver::ResolvedHelper>,
}

impl AvahiScanner {
    // Create a scanner that resolves `avahi-browse` through `search`; when
    // it is missing, `scan` reports the absence.
    pub(crate) fn new(search: &crate::exe_resolver::SearchPath) -> Self {
        Self {
            resolved: search.resolve_authority("avahi-browse"),
        }
    }
}

impl ShareScanner for AvahiScanner {
    fn scan(&self) -> ScanResult {
        self.scan_cancellable(&OperationCancel::default())
    }

    fn scan_cancellable(&self, cancel: &OperationCancel) -> ScanResult {
        let Some(resolved) = &self.resolved else {
            return Err(crate::i18n::tr(
                "Network discovery uses avahi-browse and avahi-daemon: \
                     avahi-browse not found. Install/start Avahi, or use \
                     Connect to Server without discovery.",
            )
            .into());
        };
        // `-t` terminate after dumping, `-a` all services, `-r` resolve,
        // `-p` parseable (semicolon-delimited) output.
        match run_network_helper(resolved.path(), &["-tarp", "--no-db-lookup"], cancel) {
            HelperOutcome::Complete(out) if out.status.success() => {
                Ok(parse_avahi_browse(&String::from_utf8_lossy(&out.stdout)))
            }
            HelperOutcome::Complete(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let line = stderr.trim();
                let detail = if line.is_empty() {
                    crate::i18n::tr("avahi-browse failed")
                } else {
                    line.lines().next().unwrap_or(line)
                };
                let note = out.truncation_note();
                Err(crate::i18n::format(
                    "files-network-discovery-uses-the-avahi-service-37c9fbd3",
                    &[("detail", (detail).to_owned()), ("note", (note).to_owned())],
                )
                .into())
            }
            HelperOutcome::Truncated { .. } => {
                // Overflow is a typed incomplete result; it must not replace
                // the last good set or read as an empty one.
                Err(super::ScanFailure {
                    preserve_previous: true,
                    message: crate::i18n::tr(
                        "Network discovery output exceeded the safety limit and \
                     was truncated. The previous discovery results (if any) \
                     are preserved. Retry, or use Connect to Server.",
                    )
                    .to_owned(),
                })
            }
            HelperOutcome::Failed(error) => Err(crate::i18n::format(
                "files-network-discovery-uses-avahi-browse-and-78b19e6e",
                &[("error", (error))],
            )
            .into()),
        }
    }
}

// The production default scanner, resolving through the boot `SearchPath`.
pub(crate) fn default_scanner() -> ShareScannerHandle {
    let search = crate::exe_resolver::SearchPath::from_env();
    Arc::new(ServiceScanner(AvahiScanner::new(&search)))
}

// Discovers through the enzimOS mount service when it is present, otherwise
// through Avahi.
struct ServiceScanner(AvahiScanner);

impl ShareScanner for ServiceScanner {
    fn scan(&self) -> ScanResult {
        self.scan_cancellable(&OperationCancel::default())
    }

    fn scan_cancellable(&self, cancel: &OperationCancel) -> ScanResult {
        if !crate::mount_service::detected() {
            return self.0.scan_cancellable(cancel);
        }
        let shares = crate::mount_service::block_on(crate::mount_service::discover())
            .map_err(|error| error.to_string())?;
        if cancel.is_cancelled() {
            return Err(crate::i18n::tr("Network operation cancelled").into());
        }
        Ok(shares)
    }
}

// Resolved avahi records use semicolon fields: service name at index 3 and
// host at index 6.
//
// `_smb._tcp` advertises a server, not a share: the service name is the
// server's display name ("My NAS"), so each record becomes a bare server
// (`smb://host`) whose shares are listed when connecting. A server answers
// once per interface and address family; it is listed once.
pub(crate) fn parse_avahi_browse(raw: &str) -> Vec<DiscoveredShare> {
    let mut out: Vec<DiscoveredShare> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('=') {
            continue;
        }
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < 8 {
            continue;
        }
        let service = fields[4].trim();
        let hostname = fields[6].trim().trim_end_matches('.');
        if service != "_smb._tcp" || hostname.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|known| known.host.eq_ignore_ascii_case(hostname))
        {
            continue;
        }
        out.push(DiscoveredShare {
            host: hostname.to_owned(),
            share: String::new(),
            // avahi does not report auth state; assume optional until a mount
            // attempt proves otherwise.
            needs_auth: false,
        });
    }
    out
}
