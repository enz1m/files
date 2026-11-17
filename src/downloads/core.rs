// Download core: status, transports, jobs, and staging.

use super::*;

// Whether the `aria2c` binary is available.
//
// `Probing` keeps the panel from claiming "not installed" before the probe
// returns.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Aria2Status {
    // The probe has not returned yet.
    #[default]
    Probing,
    // `aria2c` resolved on `PATH`.
    Ready,
    // `aria2c` did not resolve; the normal state on a machine without it.
    Absent,
}

impl Aria2Status {
    // Whether torrent sources can be enqueued. HTTP/HTTPS does not consult
    // this optional-backend status.
    pub(crate) fn is_ready(self) -> bool {
        matches!(self, Aria2Status::Ready)
    }
}

// Which empty state the panel renders, or `None` when there are jobs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EmptyState {
    // No queued transfers; HTTP/HTTPS remains available independently of the
    // optional torrent backend.
    QueueEmpty,
}

impl EmptyState {
    pub(crate) fn resolve(_status: Aria2Status, job_count: usize) -> Option<EmptyState> {
        (job_count == 0).then_some(EmptyState::QueueEmpty)
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            EmptyState::QueueEmpty => crate::i18n::tr(NO_DOWNLOADS),
        }
    }
}

// Where a transfer's bytes come from; injected so tests never open a socket.
//
// A factory because each [`DownloadTransport`] drives one transfer and the
// backend depends on the URI scheme (`magnet:` versus `https:`).
pub(crate) trait TransportFactory: std::fmt::Debug + Send + Sync + 'static {
    // The transport for `uri`, given what the `aria2c` probe last said.
    fn transport(&self, uri: &str, backend: Aria2Status) -> Box<dyn DownloadTransport>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemTransports;

impl TransportFactory for SystemTransports {
    fn transport(&self, uri: &str, _backend: Aria2Status) -> Box<dyn DownloadTransport> {
        if uri.trim().starts_with("magnet:") {
            return Box::new(UnavailableTransport::new(
                ARIA2C_BACKEND,
                crate::i18n::tr(ARIA2C_INSTALL_HINT),
            ));
        }
        // Media sites serve a page, not the media; Files does not download
        // from them.
        if is_known_service_url(uri) {
            return Box::new(UnavailableTransport::new(
                crate::i18n::tr("media site"),
                crate::i18n::tr(MEDIA_SITE_UNSUPPORTED),
            ));
        }
        Box::new(CurlDownloadTransport::new())
    }
}

#[derive(Debug)]
pub(super) struct RateLimitedTransport {
    inner: Box<dyn DownloadTransport>,
    bytes_per_second: u64,
    progress: Mutex<(std::time::Instant, u64)>,
}

impl RateLimitedTransport {
    pub(crate) fn new(inner: Box<dyn DownloadTransport>, kib_per_second: u32) -> Self {
        Self {
            inner,
            bytes_per_second: u64::from(kib_per_second) * 1024,
            progress: Mutex::new((std::time::Instant::now(), 0)),
        }
    }
}

impl DownloadTransport for RateLimitedTransport {
    fn start(
        &self,
        url: &str,
        plan: Option<&enzui_core::download::RangeRequestPlan>,
        cancel: &enzui_core::download::CancelSignal,
    ) -> Result<TransportResponseMeta, enzui_core::download::TransportError> {
        *self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = (std::time::Instant::now(), 0);
        self.inner.start(url, plan, cancel)
    }

    fn read_chunk(
        &self,
        cancel: &enzui_core::download::CancelSignal,
    ) -> Result<enzui_core::download::BodyChunk, enzui_core::download::TransportError> {
        let chunk = self.inner.read_chunk(cancel)?;
        if let enzui_core::download::BodyChunk::Bytes(bytes) = &chunk {
            let (started, transferred) = &mut *self
                .progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *transferred = transferred.saturating_add(bytes.len() as u64);
            let target = std::time::Duration::from_secs_f64(
                *transferred as f64 / self.bytes_per_second as f64,
            );
            while let Some(delay) = target.checked_sub(started.elapsed()) {
                if cancel.is_cancelled() {
                    return Err(enzui_core::download::TransportError::Cancelled);
                }
                std::thread::sleep(delay.min(std::time::Duration::from_millis(25)));
            }
        }
        Ok(chunk)
    }
}

// One transfer, assembled by [`DownloadsState`] and run by [`run_job`] on a
// background thread.
#[derive(Debug)]
pub(crate) struct TransferJob {
    pub(crate) id: DownloadId,
    // Which attempt; stale events are dropped by the queue.
    pub(crate) attempt: Attempt,
    // The URI handed to the transport verbatim.
    pub(crate) uri: String,
    // The file the engine writes. Its current length is the resume offset.
    pub(crate) destination: PathBuf,
    // The pause/cancel handle; a clone stays in [`DownloadsState`].
    pub(crate) control: TransferControl,
    // The validator the first response carried, if stored.
    //
    // Without it a `206` cannot be spliced onto the partial file (no
    // `If-Range`), so the transfer restarts from zero.
    pub(crate) validator: Option<Validator>,
    pub(crate) transport: Box<dyn DownloadTransport>,
    // Transfer limits in KiB/s; zero means unlimited. HTTP consumes the
    // download limit while upload applies only to aria2 torrents.
    pub(crate) download_limit_kib: u32,
    pub(crate) upload_limit_kib: u32,
    // Torrent completion/seeding behavior captured when this attempt starts.
    pub(crate) torrent_policy: TorrentPolicy,
    // Retained output directory for descriptor-relative publication.
    pub(crate) torrent_output_dir: Option<enzui_security::secure_dir::RetainedDir>,
    // Shared torrent stage authority for descriptor-relative cleanup.
    pub(crate) torrent_shared_stage: Option<SharedTorrentStage>,
    // Shared slot for partial-publication debt. Set by the worker thread
    // during publication failure; read by the reducer.
    pub(crate) publication_debt: SharedPublicationDebt,
    // The torrent backend executable, from
    // [`DownloadsState::backend_program`]; no environment is read mid-transfer.
    pub(crate) backend_program: PathBuf,
}

// Runs one job to completion, blocking, emitting every [`DownloadEvent`].
//
// The engine applies the range decision itself.
pub(crate) fn run_job(job: &TransferJob, mut emit: impl FnMut(DownloadEvent)) -> TransferOutcome {
    if is_torrent_source(&job.uri) {
        return run_aria2_torrent(job, emit, |_| {});
    }
    let request = TransferRequest::new(job.id, job.attempt, &job.uri, &job.destination)
        .with_validator(job.validator.clone());
    run_transfer(job.transport.as_ref(), &request, &job.control, &mut emit)
}

pub(crate) fn is_torrent_source(source: &str) -> bool {
    source.trim().starts_with("magnet:")
        || source
            .split(['?', '#'])
            .next()
            .is_some_and(|path| path.to_ascii_lowercase().ends_with(".torrent"))
}

pub(super) fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => directory_bytes(&path),
                Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
                _ => 0,
            }
        })
        .fold(0, u64::saturating_add)
}

// Obtain the stage path from the shared authority. Refuses if absent.
pub(super) fn obtain_stage_path(job: &TransferJob) -> Result<PathBuf, TransferError> {
    let shared = job
        .torrent_shared_stage
        .as_ref()
        .ok_or_else(|| TransferError::Io {
            category: IoFailureCategory::Unknown,
            detail: crate::i18n::tr("torrent job has no retained stage authority").into(),
        })?;
    let guard = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let stage = guard.as_ref().ok_or_else(|| TransferError::Io {
        category: IoFailureCategory::Unknown,
        detail: crate::i18n::tr("torrent stage authority was already consumed").into(),
    })?;
    Ok(stage.path().to_path_buf())
}

// Attempt `try_cleanup` on the shared stage. Returns whether cleanup completed.
pub(super) fn try_cleanup_stage(job: &TransferJob) -> bool {
    let Some(shared) = &job.torrent_shared_stage else {
        return true;
    };
    let mut guard = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(stage) = guard.as_mut() else {
        return true;
    };
    matches!(stage.try_cleanup(), Ok(true))
}

// Consume the stage after successful `try_cleanup`.
pub(super) fn consume_cleaned_stage(job: &TransferJob) {
    if let Some(shared) = &job.torrent_shared_stage {
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

// Reports a torrent failure without cleanup; the reducer stages exact
// cleanup through a `CleanupSlot`.
pub(super) fn torrent_failure(
    job: &TransferJob,
    error: TransferError,
    emit: &mut impl FnMut(DownloadEvent),
) -> TransferOutcome {
    emit(DownloadEvent::failed(job.id, job.attempt, error.message()));
    TransferOutcome::Failed {
        error,
        bytes_on_disk: directory_bytes(&job.destination),
    }
}
