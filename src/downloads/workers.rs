// Transfer workers, bridges, probes, and guards.

use super::*;

pub(super) struct CancelTransferOnDrop(pub(super) TransferControl);
impl Drop for CancelTransferOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

// Joins the transfer worker when its iced `Task` is dropped mid-flight.
//
// A detached worker can be cut short by process exit before it reaps its
// aria2 process group. The join is bounded because [`CancelTransferOnDrop`]
// drops first and `process_transport` tears down within its cleanup ceiling.
pub(super) struct JoinWorkerOnDrop(pub(super) Option<std::thread::JoinHandle<()>>);
impl Drop for JoinWorkerOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle.join();
        }
    }
}

pub(super) const TRANSFER_LIFECYCLE_CAPACITY: usize = 4;

type TransferBridgeSender = enzui_core::worker_bridge::WorkerSender<crate::message::Msg>;
pub(super) type TransferBridgeReceiver =
    enzui_core::worker_bridge::WorkerReceiver<crate::message::Msg>;
type TransferBridgeClose = enzui_core::worker_bridge::WorkerBridgeClose<crate::message::Msg>;

pub(super) fn transfer_bridge() -> (
    TransferBridgeSender,
    TransferBridgeReceiver,
    TransferBridgeClose,
) {
    enzui_core::worker_bridge::channel(
        NonZeroUsize::new(TRANSFER_LIFECYCLE_CAPACITY)
            .expect("the transfer lifecycle capacity is nonzero"),
    )
}

// The worker-side classification used by the real transfer task.
//
// Progress and torrent telemetry are latest-value slots; response metadata
// stays ordered. The final event and typed outcome are committed together
// after both throttles flush, so the reducer never sees activity after the
// terminal records. A rejected send cancels the transfer.
pub(super) struct TransferWorkerEmitter {
    sender: TransferBridgeSender,
    control: TransferControl,
    event_paints: enzui_core::operation_activity::ActivityPaintThrottle<DownloadEvent>,
    telemetry_paints: enzui_core::operation_activity::ActivityPaintThrottle<TorrentTelemetry>,
    final_event: Option<DownloadEvent>,
}

impl TransferWorkerEmitter {
    fn new(sender: TransferBridgeSender, control: TransferControl) -> Self {
        Self::with_interval(
            sender,
            control,
            enzui_core::operation_activity::ActivityPaintThrottle::<DownloadEvent>::DEFAULT_INTERVAL,
        )
    }

    pub(crate) fn with_interval(
        sender: TransferBridgeSender,
        control: TransferControl,
        interval: std::time::Duration,
    ) -> Self {
        Self {
            sender,
            control,
            event_paints: enzui_core::operation_activity::ActivityPaintThrottle::new(interval),
            telemetry_paints: enzui_core::operation_activity::ActivityPaintThrottle::new(interval),
            final_event: None,
        }
    }

    fn cancel_on_rejected_send(&self, result: Result<(), crate::message::Msg>) {
        if result.is_err() {
            self.control.cancel();
        }
    }

    pub(crate) fn emit_event(&mut self, event: DownloadEvent) {
        use crate::message::Msg;

        let now = std::time::Instant::now();
        match event {
            event @ DownloadEvent::Progress { .. } => {
                if let Some(event) = self.event_paints.push(event, now) {
                    let result = self.sender.send_progress(Msg::TransferEvent(event));
                    self.cancel_on_rejected_send(result);
                }
            }
            event @ (DownloadEvent::Finished { .. }
            | DownloadEvent::Failed { .. }
            | DownloadEvent::Paused { .. }
            | DownloadEvent::Cancelled { .. }) => {
                self.final_event = Some(event);
            }
            event => {
                let result = self.sender.send_lifecycle(Msg::TransferEvent(event));
                self.cancel_on_rejected_send(result);
            }
        }
    }

    pub(crate) fn emit_telemetry(&mut self, telemetry: TorrentTelemetry) {
        if let Some(telemetry) = self
            .telemetry_paints
            .push(telemetry, std::time::Instant::now())
        {
            let result = self
                .sender
                .send_telemetry(crate::message::Msg::TorrentTelemetry(telemetry));
            self.cancel_on_rejected_send(result);
        }
    }

    pub(crate) fn finish(mut self, id: DownloadId, attempt: Attempt, outcome: TransferOutcome) {
        use crate::message::Msg;

        let now = std::time::Instant::now();
        if let Some(progress) = self.event_paints.flush(now) {
            let result = self.sender.send_progress(Msg::TransferEvent(progress));
            self.cancel_on_rejected_send(result);
        }
        if let Some(telemetry) = self.telemetry_paints.flush(now) {
            let result = self.sender.send_telemetry(Msg::TorrentTelemetry(telemetry));
            self.cancel_on_rejected_send(result);
        }
        let final_event = self.final_event.take().map(Msg::TransferEvent);
        let result = self
            .sender
            .finish(final_event, Msg::TransferOutcome(id, attempt, outcome));
        self.cancel_on_rejected_send(result);
    }
}

pub(crate) fn transfer_task(job: TransferJob) -> iced::Task<crate::message::Msg> {
    use crate::message::Msg;
    use iced::futures::StreamExt;

    let stream = iced::stream::channel(
        64,
        move |mut output: iced::futures::channel::mpsc::Sender<Msg>| async move {
            use iced::futures::SinkExt;

            let (tx, mut rx, close_bridge) = transfer_bridge();
            let id = job.id;
            let attempt = job.attempt;
            let abort_control = job.control.clone();
            let worker_control = abort_control.clone();
            let worker = std::thread::spawn(move || {
                let emitter = TransferWorkerEmitter::new(tx, worker_control);

                // Throttle before messages cross into iced: throttling after
                // `update` would still rebuild the whole shell for every chunk.
                let (outcome, emitter) = if is_torrent_source(&job.uri) {
                    // aria2's event and console-telemetry callbacks share the
                    // classifier only for this synchronous call.
                    let emitter = std::cell::RefCell::new(emitter);
                    let outcome = run_aria2_torrent(
                        &job,
                        |event| emitter.borrow_mut().emit_event(event),
                        |telemetry| emitter.borrow_mut().emit_telemetry(telemetry),
                    );
                    (outcome, emitter.into_inner())
                } else {
                    let mut emitter = emitter;
                    let outcome = run_job(&job, |event| emitter.emit_event(event));
                    (outcome, emitter)
                };
                emitter.finish(id, attempt, outcome);
            });

            // Reverse declaration order drops these as cancel, bridge close,
            // then join; reordering reopens a deadlock or orphan window.
            let _join_worker = JoinWorkerOnDrop(Some(worker));
            let _close_before_join = close_bridge.into_guard();
            let _cancel_on_abort = CancelTransferOnDrop(abort_control.clone());

            while let Some(msg) = rx.next().await {
                if output.send(msg).await.is_err() {
                    // The Task was aborted; close before the guards join so a
                    // full lifecycle FIFO cannot pin the worker.
                    abort_control.cancel();
                    rx.close();
                    break;
                }
            }
            rx.close();
        },
    );

    iced::Task::run(stream, |m| m)
}

// Result of the aria2c probe: the resolved absolute path, if found, and the
// status. The reducer stores the path as the authority for every spawn.
#[derive(Clone, Debug)]
pub(crate) struct Aria2cProbeResult {
    pub(crate) status: Aria2Status,
    pub(crate) resolved_path: Option<PathBuf>,
}

#[cfg(test)]
impl From<Aria2Status> for Aria2cProbeResult {
    fn from(status: Aria2Status) -> Self {
        Self {
            status,
            resolved_path: status
                .is_ready()
                .then(|| std::path::PathBuf::from("/nonexistent/aria2c-test-double")),
        }
    }
}

// Probe for `aria2c` on `PATH`, on the shell owner (`crate::shell_work`)
// rather than the render thread, returning the resolved path with the status.
pub(crate) fn probe_aria2c() -> impl std::future::Future<Output = Aria2cProbeResult> + Send + 'static
{
    crate::shell_work::run(move || {
        let path = aria2c_on_path();
        Aria2cProbeResult {
            status: if path.is_some() {
                Aria2Status::Ready
            } else {
                Aria2Status::Absent
            },
            resolved_path: path,
        }
    })
}

// The resolved path to `aria2c`, if any, through the boot
// [`crate::exe_resolver::SearchPath`] snapshot, so tests express a missing
// backend with an empty search path.
pub(crate) fn aria2c_on_path() -> Option<PathBuf> {
    crate::exe_resolver::SearchPath::from_env().resolve("aria2c")
}

// Test-only helpers for the `ChildGuard` unit tests in downloads/tests.rs.
#[cfg(test)]
pub(crate) struct ChildGuard {
    pub(crate) child: std::process::Child,
    #[cfg(target_os = "linux")]
    pub(crate) process_group: i32,
    pub(crate) cleaned: bool,
    observed_status: Option<std::process::ExitStatus>,
}

#[cfg(test)]
impl ChildGuard {
    pub(crate) fn new(child: std::process::Child) -> Self {
        Self {
            #[cfg(target_os = "linux")]
            process_group: child.id() as i32,
            child,
            cleaned: false,
            observed_status: None,
        }
    }

    #[allow(
        unsafe_code,
        reason = "WNOWAIT retains the child identity until group cleanup"
    )]
    pub(crate) fn exited_preserving_identity(&mut self) -> std::io::Result<bool> {
        #[cfg(target_os = "linux")]
        // SAFETY: waitid writes initialized siginfo storage; WNOWAIT retains the owned child.
        unsafe {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            let rc = libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            if rc != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(info.assume_init().si_pid() != 0)
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.observed_status = self.child.try_wait()?;
            Ok(self.observed_status.is_some())
        }
    }

    #[allow(
        unsafe_code,
        reason = "terminate the owned process group before reaping its leader"
    )]
    pub(crate) fn terminate_and_reap(&mut self) -> Option<std::process::ExitStatus> {
        if self.cleaned {
            return self.observed_status;
        }
        self.cleaned = true;
        #[cfg(target_os = "linux")]
        // SAFETY: the unreaped owned leader pins this positive process-group identity.
        unsafe {
            let _ = libc::kill(-self.process_group, libc::SIGTERM);
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = libc::kill(-self.process_group, libc::SIGKILL);
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.child.kill();
        }
        if self.observed_status.is_none() {
            self.observed_status = self.child.wait().ok();
        }
        self.observed_status
    }
}

#[cfg(test)]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
pub(crate) fn isolate_process_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}
