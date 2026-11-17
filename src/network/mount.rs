#[cfg(not(test))]
use super::mount_ask::{configure_ask_password_callback, configure_ask_question_callback};
// Mount backends: the typed credential handoff, the gio backend, and the
// private-context mount driver.

#[cfg(not(test))]
use super::PROTOCOLS;
use super::mount_ask::QuestionPort;
use super::{HelperOutcome, NetworkProtocol, OperationCancel, run_network_helper};
use enzui_core::form::SecretText;
use std::sync::Arc;

// What one mount attempt reported: the mounted URI on success, a human line
// on failure.
pub(crate) type MountResult = Result<String, String>;

// Passwords stay in wipe-on-drop shared storage, never URIs or argv; dialog generations reject stale callbacks.
pub(crate) struct MountCredentials {
    generation: u64,
    protocol: NetworkProtocol,
    server: String,
    share: String,
    domain: String,
    username: String,
    password: SecretText,
    // Where the backend posts the questions it cannot answer itself
    // (an unknown SFTP host key). `None` declines every question.
    questions: Option<QuestionPort>,
    // The user asked for the password to be kept in the keyring.
    remember: bool,
}

impl MountCredentials {
    // Assemble a typed handoff. `password` is moved (not copied); the
    // caller's `SecretText` handle is Arc-shared with this value.
    pub(crate) fn new(
        generation: u64,
        protocol: NetworkProtocol,
        server: String,
        share: String,
        domain: String,
        username: String,
        password: SecretText,
    ) -> Self {
        Self {
            generation,
            protocol,
            server,
            share,
            domain,
            username,
            password,
            questions: None,
            remember: false,
        }
    }

    // Keep the password in the keyring when the backend can store it.
    // Only the user's explicit choice sets this.
    #[must_use]
    pub(crate) fn remembering(mut self, remember: bool) -> Self {
        self.remember = remember;
        self
    }

    #[cfg(not(test))]
    pub(crate) fn remembers_password(&self) -> bool {
        self.remember
    }

    // Route the backend's questions to the user through `port`.
    // Without a port every question is declined.
    #[must_use]
    pub(crate) fn asking(mut self, port: QuestionPort) -> Self {
        self.questions = Some(port);
        self
    }

    pub(crate) fn question_port(&self) -> Option<QuestionPort> {
        self.questions.clone()
    }

    pub(crate) fn protocol(&self) -> NetworkProtocol {
        self.protocol
    }

    pub(crate) fn domain(&self) -> &str {
        &self.domain
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    // The URI the backend mounts. Never contains the password.
    pub(crate) fn uri(&self) -> String {
        let auth = super::uri_user_part(&self.username);
        format!(
            "{}://{auth}{}/{}",
            self.protocol.scheme(),
            self.server,
            super::uri_share_part(&self.share),
        )
    }

    // Short-lived borrow of the password plaintext. The borrow cannot
    // outlive `self`, so it cannot survive the wipe when the last
    // [`SecretText`] handle drops.
    pub(crate) fn expose_password(&self) -> &str {
        self.password.expose_secret()
    }
}

impl std::fmt::Debug for MountCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the password even in Debug so `dbg!` or a panic cannot spill it.
        f.debug_struct("MountCredentials")
            .field("generation", &self.generation)
            .field("protocol", &self.protocol)
            .field("server", &self.server)
            .field("share", &self.share)
            .field("domain", &self.domain)
            .field("username", &self.username)
            .field("password", &"SecretText([redacted])")
            .field("asks", &self.questions.is_some())
            .finish()
    }
}

// The seam an Attach runs through. Anonymous mounts use [`Self::mount`] /
// [`Self::mount_cancellable`]; authenticated mounts use
// [`Self::mount_authenticated`] and require [`Self::supports_authenticated_mount`]
// to have returned `true` for the credential's protocol.
pub(crate) trait Mounter: Send + Sync {
    fn mount(&self, uri: &str) -> MountResult;
    fn mount_cancellable(&self, uri: &str, cancel: &OperationCancel) -> MountResult {
        if cancel.is_cancelled() {
            Err(crate::i18n::tr("Network operation cancelled").into())
        } else {
            self.mount(uri)
        }
    }

    // Whether this backend can carry an authenticated mount for `protocol`
    // without exposing the password in a URI, argv, environment, log or
    // persistent state.
    //
    // Defaults to `false`. The Attach dialog disables its password field for
    // protocols this backend cannot serve safely.
    fn supports_authenticated_mount(&self, _protocol: NetworkProtocol) -> bool {
        false
    }

    // Mount with typed credentials, consuming them so the password is wiped
    // when this call returns on success, failure, or cancellation.
    //
    // The default refuses with an availability message. A backend that
    // supports authenticated mounts overrides this and
    // [`Self::supports_authenticated_mount`]; it must never silently discard
    // the credential.
    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        _cancel: &OperationCancel,
    ) -> MountResult {
        // The message names the target; the password stays in `credentials`.
        let uri = credentials.uri();
        let scheme = credentials.protocol().scheme();
        drop(credentials);
        Err(crate::i18n::format(
            "files-cannot-mount-uri-with-a-password-2705be16",
            &[("uri", (uri)), ("scheme", (scheme).to_owned())],
        ))
    }
}

pub(crate) type MounterHandle = Arc<dyn Mounter>;

// Helpers use captured executable authority; authenticated mounts go through the GIO backend.
pub(crate) struct GioMounter {
    // Resolved `gio` executable. `None` when the binary is absent from
    // the boot search path; `mount_cancellable` then reports the absence.
    resolved_gio: Option<crate::exe_resolver::ResolvedHelper>,
    backend: Arc<dyn AuthenticatedMountBackend>,
}

impl GioMounter {
    // Production constructor: resolve `gio` once and probe GVfs.
    #[cfg(not(test))]
    pub(crate) fn production() -> Self {
        let search = crate::exe_resolver::SearchPath::from_env();
        Self {
            resolved_gio: search.resolve_authority("gio"),
            backend: GioBackend::probe_or_unsupported(),
        }
    }
}

impl std::fmt::Debug for GioMounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GioMounter")
            .field(
                "gio",
                &self
                    .resolved_gio
                    .as_ref()
                    .map(|r| r.path().display().to_string()),
            )
            .field("backend", &self.backend.name())
            .finish()
    }
}

impl Mounter for GioMounter {
    fn mount(&self, uri: &str) -> MountResult {
        self.mount_cancellable(uri, &OperationCancel::default())
    }

    fn mount_cancellable(&self, uri: &str, cancel: &OperationCancel) -> MountResult {
        let Some(resolved) = &self.resolved_gio else {
            return Err(crate::i18n::format(
                "files-could-not-mount-uri-gio-not-5ababf9f",
                &[("uri", (uri).to_owned())],
            ));
        };
        match run_network_helper(resolved.path(), &["mount", uri], cancel) {
            HelperOutcome::Complete(out) if out.status.success() => Ok(uri.to_owned()),
            HelperOutcome::Complete(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let line = stderr.trim();
                let note = out.truncation_note();
                Err(if line.is_empty() {
                    crate::i18n::format(
                        "files-could-not-mount-uri-note-0ffa39c5",
                        &[("uri", (uri).to_owned()), ("note", (note).to_owned())],
                    )
                } else {
                    crate::i18n::format(
                        "files-could-not-mount-uri-note-55c1fef7",
                        &[
                            ("uri", (uri).to_owned()),
                            ("arg0", (line.lines().next().unwrap_or(line)).to_owned()),
                            ("note", (note).to_owned()),
                        ],
                    )
                })
            }
            HelperOutcome::Truncated { output, .. } => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let line = stderr.trim();
                let note = output.truncation_note();
                Err(if line.is_empty() {
                    crate::i18n::format(
                        "files-could-not-mount-uri-note-output-3be9977f",
                        &[("uri", (uri).to_owned()), ("note", (note).to_owned())],
                    )
                } else {
                    crate::i18n::format(
                        "files-could-not-mount-uri-note-55c1fef7",
                        &[
                            ("uri", (uri).to_owned()),
                            ("arg0", (line.lines().next().unwrap_or(line)).to_owned()),
                            ("note", (note).to_owned()),
                        ],
                    )
                })
            }
            HelperOutcome::Failed(error) => Err(crate::i18n::format(
                "files-could-not-mount-uri-error-ffdd5600",
                &[("uri", (uri).to_owned()), ("error", (error))],
            )),
        }
    }

    fn supports_authenticated_mount(&self, protocol: NetworkProtocol) -> bool {
        // NFS authenticates by UID/GID and export lists, never by password, so
        // this stays false whatever the injected backend claims.
        if matches!(protocol, NetworkProtocol::Nfs) {
            return false;
        }
        self.backend.supports(protocol)
    }

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        cancel: &OperationCancel,
    ) -> MountResult {
        if matches!(credentials.protocol(), NetworkProtocol::Nfs) {
            let uri = credentials.uri();
            drop(credentials);
            return Err(crate::i18n::format(
                "files-cannot-mount-uri-with-a-password-40e90a65",
                &[("uri", (uri))],
            ));
        }
        self.backend.mount_authenticated(credentials, cancel)
    }
}

// The production default mounter.
//
// Under `cfg(test)` this is `GioMounter::unsupported`, so unit tests never
// depend on host GVfs or attempt a live mount.
pub(crate) fn default_mounter() -> MounterHandle {
    #[cfg(test)]
    {
        Arc::new(GioMounter::unsupported())
    }
    #[cfg(not(test))]
    {
        Arc::new(ServiceAwareMounter(GioMounter::production()))
    }
}

// With the enzimOS mount service present, the attach flow mounts through the
// service, which carries passwords for every protocol but NFS; this mounter
// then only answers for the form. Otherwise it is the GIO mounter.
#[cfg(not(test))]
struct ServiceAwareMounter(GioMounter);

#[cfg(not(test))]
impl Mounter for ServiceAwareMounter {
    fn mount(&self, uri: &str) -> MountResult {
        self.0.mount(uri)
    }

    fn mount_cancellable(&self, uri: &str, cancel: &OperationCancel) -> MountResult {
        self.0.mount_cancellable(uri, cancel)
    }

    fn supports_authenticated_mount(&self, protocol: NetworkProtocol) -> bool {
        if crate::mount_service::detected() {
            return !matches!(protocol, NetworkProtocol::Nfs);
        }
        self.0.supports_authenticated_mount(protocol)
    }

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        cancel: &OperationCancel,
    ) -> MountResult {
        self.0.mount_authenticated(credentials, cancel)
    }
}

// Decouples [`GioMounter`]'s authenticated path from the GIO invocation.
//
// Implementations consume [`MountCredentials`] by value and must not copy
// the password into any long-lived surface.
pub(crate) trait AuthenticatedMountBackend: Send + Sync + std::fmt::Debug {
    // A short human-readable name used only for `Debug`. Never carries
    // a secret or a URI.
    fn name(&self) -> &'static str;

    // Whether this backend can carry an authenticated mount for
    // `protocol`. May reflect a runtime probe (e.g. GVfs advertising
    // the scheme in `Vfs::supported_uri_schemes`).
    fn supports(&self, protocol: NetworkProtocol) -> bool;

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        cancel: &OperationCancel,
    ) -> MountResult;
}

// Fallback backend without an in-process credential channel: every attempt
// is refused with the explicit-availability message, so hosts without GVfs
// fail visibly instead of pretending success.
#[derive(Debug, Default)]
pub(crate) struct UnsupportedMountBackend;

impl AuthenticatedMountBackend for UnsupportedMountBackend {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn supports(&self, _protocol: NetworkProtocol) -> bool {
        false
    }

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        _cancel: &OperationCancel,
    ) -> MountResult {
        let uri = credentials.uri();
        let scheme = credentials.protocol().scheme();
        drop(credentials);
        Err(crate::i18n::format(
            "files-cannot-mount-uri-with-a-password-2705be16",
            &[("uri", (uri)), ("scheme", (scheme).to_owned())],
        ))
    }
}

// The production in-process backend: one bounded [`glib::MainContext`] on a
// dedicated worker thread per attempt, so the thread's exit proves the
// [`gio::MountOperation`] closure and its captured credentials were dropped.
pub(crate) struct GioBackend {
    // Which URI schemes GVfs advertised at construction time. NFS is
    // filtered out by [`GioMounter::supports_authenticated_mount`]
    // regardless.
    supported_schemes: Vec<String>,
    // The mount driver; tests inject one that never returns.
    driver: MountDriver,
    // How long the caller waits for the worker's first result.
    outer_timeout: std::time::Duration,
    // After the outer wait expires and cancellation trips, how long to wait
    // for the worker to report.
    grace: std::time::Duration,
    // Finish flags of detached workers. New admissions are refused while any
    // is still `false`, bounding leaked threads and secrets to stuck GIO calls.
    orphans: std::sync::Mutex<Vec<Arc<std::sync::atomic::AtomicBool>>>,
}

impl std::fmt::Debug for GioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Reap in Debug too so the printed count matches the check.
        let live = self.live_orphan_count_internal();
        f.debug_struct("GioBackend")
            .field("supported_schemes", &self.supported_schemes)
            .field("outer_timeout", &self.outer_timeout)
            .field("grace", &self.grace)
            .field("live_orphans", &live)
            .finish()
    }
}

// The mount driver, invoked once per authenticated job with ownership of
// the credentials.
//
// `Arc<Fn>` rather than a generic keeps [`GioBackend`] storable behind
// `Arc<dyn AuthenticatedMountBackend>`. Production uses
// [`drive_mount_on_private_context`].
pub(crate) type MountDriver =
    Arc<dyn Fn(String, MountCredentials, OperationCancel) -> MountResult + Send + Sync + 'static>;

// Caller-side wall-clock budget for one authenticated mount attempt.
#[cfg(not(test))]
const GIO_MOUNT_OUTER_TIMEOUT_DEFAULT: std::time::Duration = std::time::Duration::from_secs(90);
// Grace after a caller-side timeout for the worker to observe the cancel.
// A worker still silent afterwards is detached and keeps its
// [`MountCredentials`] in memory until GIO releases the operation.
#[cfg(not(test))]
const GIO_MOUNT_JOIN_GRACE_DEFAULT: std::time::Duration = std::time::Duration::from_secs(5);
// The worker's inner deadline, shorter than the outer budget so a timeout
// normally surfaces the GIO diagnostic.
#[cfg(not(test))]
const GIO_MOUNT_INNER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);
// How often the worker's private context checks `OperationCancel`.
#[cfg(not(test))]
const GIO_CANCEL_POLL_MS: u64 = 50;
// Detached workers tolerated before new authenticated mounts are refused;
// zero caps leaked threads and secrets to what GIO has actually stuck.
const GIO_MAX_LIVE_ORPHANS: usize = 0;

impl GioBackend {
    // Probe GVfs at runtime: install a [`GioBackend`] when it is active and
    // advertises a supported scheme, else the [`UnsupportedMountBackend`].
    #[cfg(not(test))]
    pub(crate) fn probe_or_unsupported() -> Arc<dyn AuthenticatedMountBackend> {
        use gio::prelude::*;
        let vfs = gio::Vfs::default();
        if !vfs.is_active() {
            return Arc::new(UnsupportedMountBackend);
        }
        let schemes: Vec<String> = vfs
            .supported_uri_schemes()
            .into_iter()
            .map(|g| g.to_string())
            .collect();
        // GVfs exposing only local schemes cannot deliver the capability.
        let advertises_any = PROTOCOLS
            .iter()
            .any(|p| !matches!(p, NetworkProtocol::Nfs) && schemes.iter().any(|s| s == p.scheme()));
        if !advertises_any {
            return Arc::new(UnsupportedMountBackend);
        }
        Arc::new(GioBackend {
            supported_schemes: schemes,
            driver: production_mount_driver(),
            outer_timeout: GIO_MOUNT_OUTER_TIMEOUT_DEFAULT,
            grace: GIO_MOUNT_JOIN_GRACE_DEFAULT,
            orphans: std::sync::Mutex::new(Vec::new()),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_config(
        schemes: Vec<String>,
        driver: MountDriver,
        outer_timeout: std::time::Duration,
        grace: std::time::Duration,
    ) -> Self {
        Self {
            supported_schemes: schemes,
            driver,
            outer_timeout,
            grace,
            orphans: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn live_orphan_count(&self) -> usize {
        self.live_orphan_count_internal()
    }

    fn live_orphan_count_internal(&self) -> usize {
        let mut orphans = self
            .orphans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        orphans.retain(|f| !f.load(std::sync::atomic::Ordering::Acquire));
        orphans.len()
    }
}

// Flips a shared flag when the worker closure retires, including by panic,
// so a stuck worker that eventually finishes unblocks new admissions.
struct FinishGuard {
    flag: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.flag.store(true, std::sync::atomic::Ordering::Release);
    }
}

// The production [`MountDriver`]: spin a private `MainContext`,
// run one `gio::File::mount_enclosing_volume`, deliver its result.
#[cfg(not(test))]
fn production_mount_driver() -> MountDriver {
    Arc::new(drive_mount_on_private_context)
}

impl AuthenticatedMountBackend for GioBackend {
    fn name(&self) -> &'static str {
        "gio-in-process"
    }

    fn supports(&self, protocol: NetworkProtocol) -> bool {
        self.supported_schemes
            .iter()
            .any(|s| s == protocol.scheme())
    }

    fn mount_authenticated(
        &self,
        credentials: MountCredentials,
        cancel: &OperationCancel,
    ) -> MountResult {
        // The worker owns the credentials; the URI is captured for the outer
        // error paths.
        let uri = credentials.uri();
        if !self.supports(credentials.protocol()) {
            let scheme = credentials.protocol().scheme();
            drop(credentials);
            return Err(crate::i18n::format(
                "files-cannot-mount-uri-with-a-password-31917305",
                &[("uri", (uri)), ("scheme", (scheme).to_owned())],
            ));
        }

        // Refuse while any timed-out worker is still live, so repeated timeouts
        // cannot leak unbounded threads or captured secrets.
        let live_orphans = self.live_orphan_count_internal();
        if live_orphans > GIO_MAX_LIVE_ORPHANS {
            drop(credentials);
            return Err(crate::i18n::format(
                "files-cannot-mount-uri-with-a-password-f6bd31f8",
                &[("uri", (uri)), ("live_orphans", (live_orphans).to_string())],
            ));
        }

        let (tx, rx) = std::sync::mpsc::sync_channel::<MountResult>(1);
        let finish_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finish_flag_worker = Arc::clone(&finish_flag);
        let op_cancel = cancel.clone();
        let uri_for_thread = uri.clone();
        let driver = Arc::clone(&self.driver);
        let handle = std::thread::Builder::new()
            .name("files-gvfs-mount".to_owned())
            .spawn(move || {
                // Fires on every exit path, including panic unwind.
                let _finish_guard = FinishGuard {
                    flag: finish_flag_worker,
                };
                let result = (driver)(uri_for_thread, credentials, op_cancel);
                // A retired caller has dropped `rx`, so a late result cannot
                // reach the current Attach generation.
                let _ = tx.send(result);
            })
            .map_err(|e| {
                crate::i18n::format(
                    "files-could-not-spawn-gio-mount-worker-3ec6f153",
                    &[("uri", (uri).clone()), ("e", (e).to_string())],
                )
            })?;

        // On timeout, trip the cancel so the worker asks GIO to cancel, then
        // wait the grace for its report.
        match rx.recv_timeout(self.outer_timeout) {
            Ok(result) => {
                // The worker has already sent its result, so this join is
                // bounded; it is the only join.
                let _ = handle.join();
                result
            }
            Err(_) => {
                cancel.cancel();
                match rx.recv_timeout(self.grace) {
                    Ok(result) => {
                        let _ = handle.join();
                        result
                    }
                    Err(_) => {
                        // Detach: `join` is unbounded and GIO may never
                        // return. The worker's credentials stay alive until
                        // GIO releases the operation.
                        {
                            let mut orphans = self
                                .orphans
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            orphans.retain(|f| !f.load(std::sync::atomic::Ordering::Acquire));
                            orphans.push(Arc::clone(&finish_flag));
                        }
                        drop(handle);
                        Err(crate::i18n::format(
                            "files-could-not-mount-uri-gio-worker-f8e56650",
                            &[
                                ("uri", (uri)),
                                ("arg0", (self.outer_timeout.as_secs()).to_string()),
                                ("arg1", (self.grace.as_secs()).to_string()),
                            ],
                        ))
                    }
                }
            }
        }
    }
}

// The production driver: create a private [`glib::MainContext`], push
// it as the thread-default, run the mount to completion or
// cancellation on a [`glib::MainLoop`], and drop everything. Called
// on a dedicated worker thread by [`GioBackend::mount_authenticated`].
#[cfg(not(test))]
fn drive_mount_on_private_context(
    uri: String,
    credentials: MountCredentials,
    op_cancel: OperationCancel,
) -> MountResult {
    use gio::prelude::*;
    use glib::{ControlFlow, MainContext, MainLoop, Priority};
    use std::sync::Mutex;

    let ctx = MainContext::new();
    let uri_for_result = uri.clone();
    match ctx.with_thread_default(|| -> MountResult {
        let ml = MainLoop::new(Some(&ctx), false);
        let cancellable = gio::Cancellable::new();
        let file = gio::File::for_uri(&uri);
        let mount_op = gio::MountOperation::new();

        // Questions (an unknown SFTP host key) go to the user; the port
        // is taken before the password callback consumes `credentials`.
        configure_ask_question_callback(&mount_op, credentials.question_port(), op_cancel.clone());

        // The closure owns `credentials` and wipes them on drop.
        let _call_counter = configure_ask_password_callback(&mount_op, credentials);

        let result_slot: Arc<Mutex<Option<Result<(), glib::Error>>>> = Arc::new(Mutex::new(None));
        let result_slot_cb = Arc::clone(&result_slot);
        let ml_done = ml.clone();

        file.mount_enclosing_volume(
            gio::MountMountFlags::NONE,
            Some(&mount_op),
            Some(&cancellable),
            move |result| {
                if let Ok(mut slot) = result_slot_cb.lock() {
                    *slot = Some(result);
                }
                ml_done.quit();
            },
        );

        // Bridge the app-side cancel to the `gio::Cancellable`; the mount
        // callback then reports the cancelled error and quits the loop.
        let cancellable_poll = cancellable.clone();
        let op_cancel_poll = op_cancel.clone();
        let cancel_source = glib::timeout_source_new(
            std::time::Duration::from_millis(GIO_CANCEL_POLL_MS),
            Some("files-cancel-watch"),
            Priority::DEFAULT,
            move || {
                if op_cancel_poll.is_cancelled() {
                    cancellable_poll.cancel();
                    ControlFlow::Break
                } else {
                    ControlFlow::Continue
                }
            },
        );
        cancel_source.attach(Some(&ctx));

        // Inner deadline: cancel if GIO hangs.
        let cancellable_deadline = cancellable;
        let deadline_source = glib::timeout_source_new(
            GIO_MOUNT_INNER_TIMEOUT,
            Some("files-mount-deadline"),
            Priority::DEFAULT,
            move || {
                cancellable_deadline.cancel();
                ControlFlow::Break
            },
        );
        deadline_source.attach(Some(&ctx));

        ml.run();

        let outcome = match result_slot.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        match outcome {
            Some(Ok(())) => Ok(uri_for_result.clone()),
            Some(Err(e)) => {
                let msg = e.message().to_owned();
                Err(crate::i18n::format(
                    "files-could-not-mount-uri-for-result-c4a0c052",
                    &[("uri_for_result", (uri_for_result).clone()), ("msg", (msg))],
                ))
            }
            None => Err(crate::i18n::format(
                "files-could-not-mount-uri-for-result-418230ed",
                &[("uri_for_result", (uri_for_result).clone())],
            )),
        }
    }) {
        Ok(r) => r,
        Err(bool_err) => Err(crate::i18n::format(
            "files-could-not-mount-uri-worker-failed-0318c131",
            &[("uri", (uri)), ("bool_err", (bool_err).to_string())],
        )),
    }
}

#[cfg(test)]
impl GioMounter {
    pub(crate) fn unsupported() -> Self {
        Self {
            resolved_gio: None,
            backend: Arc::new(UnsupportedMountBackend),
        }
    }
}

#[cfg(test)]
impl MountCredentials {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}
