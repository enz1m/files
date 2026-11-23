// Credentials remain in the secure field; discovery parsing itself performs no network I/O.

// Whether the attach form needs a password, and how to label the field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasswordRequirement {
    // Anonymous or guest access; labelled plain `"Password"`. Discovery does
    // not yet produce it.

    // The share may take a password but does not require one.
    Optional,
    // The share refused anonymous access — a password is required.
    Required,
}

impl PasswordRequirement {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PasswordRequirement::Optional => crate::i18n::tr("Password (optional)"),
            PasswordRequirement::Required => crate::i18n::tr("Password Required"),
        }
    }

    pub(crate) fn allows_empty(self) -> bool {
        !matches!(self, PasswordRequirement::Required)
    }
}

// The protocol of a network location. Kept as an enum rather than a string so
// the attach form cannot submit a typo'd scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkProtocol {
    Smb,
    Nfs,
    Sftp,
    Webdav,
}

impl NetworkProtocol {
    // The URI scheme (`gio mount` verb) for this protocol.
    pub(crate) fn scheme(self) -> &'static str {
        match self {
            NetworkProtocol::Smb => "smb",
            NetworkProtocol::Nfs => "nfs",
            NetworkProtocol::Sftp => "sftp",
            NetworkProtocol::Webdav => "dav",
        }
    }
}

// The parallel arrays for the protocol `enz_radio_group` (index↔enum wire).
pub(crate) const PROTOCOL_LABELS: [&str; 4] = ["SMB / CIFS", "NFS", "SFTP", "WebDAV"];

// The Attach dialog's username field label. Optional because
// `AttachState::mount_uri` composes an anonymous URI from an empty username.
pub(crate) const USERNAME_FIELD_LABEL: &str = "Username (optional)";
pub(crate) const PROTOCOLS: [NetworkProtocol; 4] = [
    NetworkProtocol::Smb,
    NetworkProtocol::Nfs,
    NetworkProtocol::Sftp,
    NetworkProtocol::Webdav,
];

// The index of `protocol` in [`PROTOCOLS`].
pub(crate) fn protocol_index(protocol: NetworkProtocol) -> usize {
    PROTOCOLS.iter().position(|p| *p == protocol).unwrap_or(0)
}

// The [`NetworkProtocol`] at `index`, or [`NetworkProtocol::Smb`] when out of
// range.
pub(crate) fn protocol_from_index(index: usize) -> NetworkProtocol {
    PROTOCOLS
        .get(index)
        .copied()
        .unwrap_or(NetworkProtocol::Smb)
}

// A discovered host/share from an SMB/mDNS browse. Pure data parsed by
// [`parse_discovery`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoveredShare {
    pub(crate) host: String,
    pub(crate) share: String,
    // Whether the browse reported the share as requiring authentication.
    pub(crate) needs_auth: bool,
}

impl DiscoveredShare {
    // The `smb://host/share` URI for this discovered share.

    pub(crate) fn uri(&self) -> String {
        if self.share.is_empty() {
            format!("smb://{}", self.host)
        } else {
            format!("smb://{}/{}", self.host, self.share)
        }
    }

    pub(crate) fn password_requirement(&self) -> PasswordRequirement {
        if self.needs_auth {
            PasswordRequirement::Required
        } else {
            PasswordRequirement::Optional
        }
    }
}

// The `user@` part of a mount address, escaped; empty for no user or for one
// with address syntax in it (`me@outlook.com`, `DOMAIN\user`, `dom;user`),
// which the mount's password prompt supplies instead.
pub(crate) fn uri_user_part(user: &str) -> String {
    let user = user.trim();
    if user.is_empty() || user.contains(['\\', '@', ';', '/', ':']) {
        return String::new();
    }
    format!(
        "{}@",
        percent_encoding::utf8_percent_encode(user, percent_encoding::NON_ALPHANUMERIC)
    )
}

// A share path for a mount address: each segment escaped (`Public #1` must
// not become a fragment), the separators kept.
pub(crate) fn uri_share_part(share: &str) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
    const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~')
        .remove(b'$');
    share
        .trim()
        .trim_matches('/')
        .split('/')
        .map(|segment| utf8_percent_encode(segment, SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

// Keeps the first of each address, so a server that answered on several
// interfaces is one row (and one widget key).
pub(crate) fn unique_shares(mut shares: Vec<DiscoveredShare>) -> Vec<DiscoveredShare> {
    let mut seen = std::collections::HashSet::new();
    shares.retain(|share| seen.insert(share.uri().to_lowercase()));
    shares
}

// Discovery fixtures use HOST<TAB>SHARE[<TAB>auth]; header and incomplete lines are ignored.
pub(crate) fn parse_discovery(raw: &str) -> Vec<DiscoveredShare> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t').map(str::trim);
        let host = match fields.next() {
            Some(h) if !h.is_empty() => h,
            _ => continue,
        };
        let share = match fields.next() {
            Some(s) if !s.is_empty() => s,
            _ => continue,
        };
        let needs_auth = fields.next() == Some("auth");
        out.push(DiscoveredShare {
            host: host.to_owned(),
            share: share.to_owned(),
            needs_auth,
        });
    }
    out
}

use std::sync::Arc;

#[cfg(test)]
use enzui_core::form::SecretText;

// Cooperative cancellation shared between an iced task and its subprocess
// worker. Cancelling kills the helper; aborting only the Task would discard
// the reply while leaving a hung network process alive.
#[derive(Clone, Debug, Default)]
pub(crate) struct OperationCancel {
    token: enzui_core::cancellation::CancelToken,
}

impl OperationCancel {
    pub(crate) fn cancel(&self) {
        self.token
            .cancel(enzui_core::cancellation::CancelCause::Cancelled);
    }
    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_terminal()
    }
    #[cfg(not(test))]
    pub(crate) fn token(&self) -> &enzui_core::cancellation::CancelToken {
        &self.token
    }
}

pub(crate) const OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

// How much helper stdout is retained. Discovery output is controlled by every
// host on the LAN, so the parse buffer is bounded.
pub(crate) const HELPER_STDOUT_CAP: usize = 1 << 20;
// How much helper stderr is retained. Only the first diagnostic line is ever
// shown, so the cap is far tighter than stdout's.
pub(crate) const HELPER_STDERR_CAP: usize = 64 << 10;

use enzui_core::cancellation::{CancelCause, CancelToken};
use enzui_services::process_transport::{
    self, CaptureCompleteness, ProcessErrorKind, ProcessOutput, ProcessRequest, StdinPolicy,
};

// Session variables forwarded, when present, to network helpers spawned
// with `env_clear`: D-Bus for `gio mount`, display, locale and XDG runtime.
const NETWORK_HELPER_SESSION_KEYS: &[&str] = &[
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "LANG",
    "LC_ALL",
];

// Capture allowed session variables for network helpers. Absent variables are
// omitted rather than set empty.
fn capture_session_env() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    NETWORK_HELPER_SESSION_KEYS
        .iter()
        .filter_map(|key| {
            std::env::var_os(key).map(|value| (std::ffi::OsString::from(*key), value))
        })
        .collect()
}

// Resolve an owner-private (0700, per-uid) cwd for helpers. Failure is
// visible; there is no shared `/tmp` fallback.
fn private_cwd() -> Result<std::path::PathBuf, String> {
    enzui_core::runtime_dir::runtime_dir()
        .map(|rd| rd.path().to_path_buf())
        .map_err(|e| {
            crate::i18n::format(
                "files-owner-private-runtime-directory-unavailable-e-5e275edc",
                &[("e", (e).to_string())],
            )
        })
}

fn network_helper_request(
    program: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
    stdout_cap: usize,
    stderr_cap: usize,
) -> Result<ProcessRequest, String> {
    let session_vars = capture_session_env();
    let cwd = private_cwd()?;
    let request = ProcessRequest::new(
        program.as_os_str().to_os_string(),
        StdinPolicy::Null,
        stdout_cap,
        stderr_cap,
        timeout,
    )
    .args(args.iter().map(|a| std::ffi::OsString::from(*a)))
    .envs(session_vars)
    .current_dir(cwd);
    Ok(request)
}

// One bounded network-helper run: the exit status plus the retained head of
// each stream, with a per-stream flag saying whether anything was dropped.
//
// The truncation flags are carried rather than inferred from length:
// "exactly the cap" and "the cap out of a gigabyte" are different facts.
#[derive(Debug)]
pub(crate) struct HelperOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

impl HelperOutput {
    // The suffix a human-facing message carries when output was dropped, so a
    // diagnostic built from a capped stream never reads as the whole story.
    fn truncation_note(&self) -> &'static str {
        if self.stdout_truncated || self.stderr_truncated {
            crate::i18n::tr(" (helper output truncated)")
        } else {
            ""
        }
    }
}

impl From<ProcessOutput> for HelperOutput {
    fn from(out: ProcessOutput) -> Self {
        Self {
            status: out.status,
            stdout: out.stdout,
            stderr: out.stderr,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }
}

// Outcome of running a network helper through `process_transport`.
// Distinguishes complete output from overflow truncation and other failures.
#[derive(Debug)]
pub(crate) enum HelperOutcome {
    // Complete bounded output.
    Complete(HelperOutput),
    // stdout or stderr exceeded its cap. The retained head is present but
    // must not be treated as the whole output.
    Truncated { output: HelperOutput },
    // The helper failed before a complete exit.
    Failed(String),
}

// Run a network helper under [`OPERATION_TIMEOUT`] through the framework's
// `process_transport` (process-group ownership, capped drains, TERM/KILL,
// exact reap).
//
// `program` must be an absolute path resolved through
// [`crate::exe_resolver::SearchPath`].
fn run_network_helper(
    program: &std::path::Path,
    args: &[&str],
    cancel: &OperationCancel,
) -> HelperOutcome {
    run_network_helper_within(program, args, cancel, OPERATION_TIMEOUT)
}

// Like [`run_network_helper`] with a caller-chosen deadline.
pub(crate) fn run_network_helper_within(
    program: &std::path::Path,
    args: &[&str],
    cancel: &OperationCancel,
    timeout: std::time::Duration,
) -> HelperOutcome {
    let request = match network_helper_request(
        program,
        args,
        timeout,
        HELPER_STDOUT_CAP,
        HELPER_STDERR_CAP,
    ) {
        Ok(r) => r,
        Err(msg) => return HelperOutcome::Failed(msg),
    };

    let token = CancelToken::new();
    if cancel.is_cancelled() {
        token.cancel(CancelCause::Cancelled);
    }

    let watcher_cancel = cancel.clone();
    let watcher_token = token.share();
    let watcher = std::thread::Builder::new()
        .name("files-net-cancel-bridge".to_owned())
        .spawn(move || {
            while !watcher_token.is_terminal() {
                if watcher_cancel.is_cancelled() {
                    watcher_token.cancel(CancelCause::Cancelled);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        });

    let result = process_transport::run(request, &token);

    // Stops the watcher.
    token.cancel(CancelCause::Cancelled);
    if let Ok(handle) = watcher {
        let _ = handle.join();
    }

    match result {
        Ok(output) => HelperOutcome::Complete(HelperOutput::from(output)),
        Err(error) => match error.kind() {
            ProcessErrorKind::Cancelled(_) => {
                HelperOutcome::Failed(crate::i18n::tr("Network operation cancelled").to_owned())
            }
            ProcessErrorKind::Deadline => {
                HelperOutcome::Failed(crate::i18n::tr("Network operation timed out").to_owned())
            }
            ProcessErrorKind::OutputLimitExceeded(_) => {
                let stdout_truncated =
                    error.stdout_completeness() == CaptureCompleteness::OverflowTruncated;
                let stderr_truncated =
                    error.stderr_completeness() == CaptureCompleteness::OverflowTruncated;
                let output = HelperOutput {
                    // Overflow kills the child, so there is no real status.
                    status: {
                        // Callers of truncated results ignore the status.
                        use std::os::unix::process::ExitStatusExt;
                        std::process::ExitStatus::from_raw(1)
                    },
                    stdout: error.stdout().to_vec(),
                    stderr: error.stderr().to_vec(),
                    stdout_truncated,
                    stderr_truncated,
                };
                HelperOutcome::Truncated { output }
            }
            ProcessErrorKind::Spawn => {
                let detail = error
                    .diagnostic()
                    .unwrap_or_else(|| crate::i18n::tr("spawn failed"));
                if detail.contains("not found") || detail.contains("No such file") {
                    let prog = program.display();
                    HelperOutcome::Failed(crate::i18n::format(
                        "files-prog-not-found-199c69ee",
                        &[("prog", (prog).to_string())],
                    ))
                } else {
                    let prog = program.display();
                    HelperOutcome::Failed(crate::i18n::format(
                        "files-could-not-start-prog-detail-732c858a",
                        &[
                            ("prog", (prog).to_string()),
                            ("detail", (detail).to_owned()),
                        ],
                    ))
                }
            }
            ProcessErrorKind::ExecutableAuthority => {
                let prog = program.display();
                HelperOutcome::Failed(crate::i18n::format(
                    "files-prog-executable-authority-refused-path-must-9efe88f0",
                    &[("prog", (prog).to_string())],
                ))
            }
            _ => {
                let prog = program.display();
                HelperOutcome::Failed(crate::i18n::format(
                    "files-network-helper-failed-prog-error-c5134b13",
                    &[("prog", (prog).to_string()), ("error", (error).to_string())],
                ))
            }
        },
    }
}

// Discovery and mount are injected traits stored as `Arc<dyn ..>`, because a
// test that ran the real `avahi-browse` or `gio mount` could wedge the host.

// What one discovery scan reported: shares on success, a human line on
// failure, so "found nothing" stays distinct from "the scan failed".
pub(crate) type ScanResult = Result<Vec<DiscoveredShare>, ScanFailure>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScanFailure {
    pub(crate) message: String,
    pub(crate) preserve_previous: bool,
}

impl From<String> for ScanFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            preserve_previous: false,
        }
    }
}

impl From<&str> for ScanFailure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

// The seam discovery runs through, called off-thread by the reducer.
pub(crate) trait ShareScanner: Send + Sync {
    fn scan(&self) -> ScanResult;
    fn scan_cancellable(&self, cancel: &OperationCancel) -> ScanResult {
        if cancel.is_cancelled() {
            Err(crate::i18n::tr("Network operation cancelled").into())
        } else {
            self.scan()
        }
    }
}

pub(crate) type ShareScannerHandle = Arc<dyn ShareScanner>;

// A scanner that returns a scripted [`ScanResult`] and records that it ran.

// A mounter that records every URI it was asked to mount and returns a
// scripted result. It records only the URI and has no password channel.

// A scrubbed record of one authenticated mount attempt: URI, username,
// generation and password length, never the password bytes. Leak tests scan
// these fields for the correct password.
#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub(crate) struct AuthAttemptRecord {
    pub(crate) call_count: usize,
    pub(crate) last_uri: Option<String>,
    pub(crate) last_username: Option<String>,
    pub(crate) last_domain: Option<String>,
    pub(crate) last_generation: Option<u64>,
    pub(crate) last_password_len: Option<usize>,
    pub(crate) last_error: Option<String>,
    pub(crate) last_result_uri: Option<String>,
    // Whether the last observed password equalled the expected one; kept as
    // a boolean so the record holds no plaintext.
    pub(crate) last_password_matched: Option<bool>,
    // Whether the last handoff carried a question port.
    pub(crate) last_asks_questions: Option<bool>,
    // argv a process-spawning backend would have used; empty for this
    // in-process fixture.
    pub(crate) argv_capture: Vec<Vec<String>>,
    // Environment a process-spawning backend would have set; empty here.
    pub(crate) env_capture: Vec<Vec<(String, String)>>,
    // Redacted log lines the fixture emitted.
    pub(crate) log_capture: Vec<String>,
}

// A `Mounter` that accepts one scripted password and records each
// credential handoff in an [`AuthAttemptRecord`].
//
// Any other password fails with `"Authentication failed"`. A generation
// that differs from [`Self::set_current_generation`] is refused as
// `"Stale credential generation"` before the password is read.
#[cfg(test)]
pub(crate) struct AuthenticatingRecordingMounter {
    expected: SecretText,
    inner: std::sync::Mutex<AuthAttemptRecord>,
    current_generation: std::sync::atomic::AtomicU64,
    supports: fn(NetworkProtocol) -> bool,
    cancel_before_call: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl AuthenticatingRecordingMounter {
    pub(crate) fn expecting(expected: impl Into<String>) -> Self {
        Self {
            expected: SecretText::new(expected.into()),
            inner: std::sync::Mutex::new(AuthAttemptRecord::default()),
            current_generation: std::sync::atomic::AtomicU64::new(0),
            supports: |_| true,
            cancel_before_call: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn set_current_generation(&self, generation: u64) {
        self.current_generation
            .store(generation, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn cancel_at_call_start(&self) {
        self.cancel_before_call
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn record(&self) -> AuthAttemptRecord {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Mounter for AuthenticatingRecordingMounter {
    fn mount(&self, uri: &str) -> MountResult {
        // A process backend would put the URI in argv; recording it there lets
        // leak tests check the anonymous path too.
        let mut rec = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rec.argv_capture
            .push(vec!["gio".into(), "mount".into(), uri.into()]);
        Ok(uri.to_owned())
    }

    fn supports_authenticated_mount(&self, protocol: NetworkProtocol) -> bool {
        (self.supports)(protocol)
    }

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        cancel: &OperationCancel,
    ) -> MountResult {
        // Cancellation is checked before the password is read.
        if self
            .cancel_before_call
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            cancel.cancel();
        }
        if cancel.is_cancelled() {
            let mut rec = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rec.call_count += 1;
            rec.last_error = Some("Network operation cancelled".to_owned());
            // Drop the credentials unread; the last handle wipes the plaintext.
            drop(credentials);
            return Err("Network operation cancelled".to_owned());
        }

        // A stale generation is refused before the password is compared.
        let current = self
            .current_generation
            .load(std::sync::atomic::Ordering::SeqCst);
        if credentials.generation() != current {
            let mut rec = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rec.call_count += 1;
            rec.last_generation = Some(credentials.generation());
            rec.last_uri = Some(credentials.uri());
            rec.last_error = Some(format!(
                "Stale credential generation: got {}, current {current}",
                credentials.generation(),
            ));
            rec.log_capture.push(format!(
                "auth-mount refused: stale generation {} != current {current}",
                credentials.generation(),
            ));
            drop(credentials);
            return Err("Stale credential generation".to_owned());
        }

        // Only the comparison result is kept; the bytes are never copied out.
        let uri = credentials.uri();
        let generation = credentials.generation();
        let username = credentials.username().to_owned();
        let domain = credentials.domain().to_owned();
        let matches = credentials.expose_password() == self.expected.expose_secret();
        let password_len = credentials.expose_password().len();

        let mut rec = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rec.call_count += 1;
        rec.last_uri = Some(uri.clone());
        rec.last_username = Some(username);
        rec.last_domain = Some(domain);
        rec.last_generation = Some(generation);
        rec.last_password_len = Some(password_len);
        rec.last_password_matched = Some(matches);
        rec.last_asks_questions = Some(credentials.question_port().is_some());
        rec.log_capture.push(format!(
            "auth-mount attempt: uri={uri} generation={generation} \
             password_bytes=[redacted {password_len}]",
        ));
        if matches {
            rec.last_error = None;
            rec.last_result_uri = Some(uri.clone());
            drop(rec);
            drop(credentials);
            Ok(uri)
        } else {
            let err = "Authentication failed".to_owned();
            rec.last_error = Some(err.clone());
            drop(rec);
            drop(credentials);
            Err(err)
        }
    }
}

#[path = "network/mount.rs"]
mod mount;
#[path = "network/mount_ask.rs"]
mod mount_ask;
#[path = "network/policy.rs"]
mod policy;
#[path = "network/scanners.rs"]
mod scanners;
#[path = "network/state.rs"]
mod state;

#[cfg(test)]
pub(crate) use mount::{AuthenticatedMountBackend, GioBackend, MountDriver};
pub(crate) use mount::{MountCredentials, MountResult, Mounter, MounterHandle, default_mounter};
pub(crate) use mount_ask::{AskedQuestion, MountAnswer, MountQuestion, question_channel};
pub(crate) use policy::{
    DiscoveryContext, NetworkContextProbeHandle, NetworkTrustStatus, SettingsOpenerHandle,
    default_context_probe, default_settings_opener,
};
pub(crate) use scanners::default_scanner;
pub(crate) use state::{
    AttachState, BackendStatus, DiscoveryState, NetworkState, ServiceTarget, TargetPurpose,
};

#[cfg(test)]
mod tests;
