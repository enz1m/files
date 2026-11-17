// Service-page downloads require inspection and explicit format selection before transfer admission.

// [`probe_aria2c`] runs through `Task::perform` and only looks for an executable
// on `PATH`, spawning nothing, so a missing tool is a deterministic state rather
// than a hang.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use enzui_core::state_authority::{CustodySnapshot, PublishCustody, StateAuthority, StateIdentity};

use enzui_core::download::transfer::{
    TransferControl, TransferError, TransferOutcome, TransferRequest, TransportCategory,
    UnavailableTransport, run_transfer,
};
use enzui_core::download::{
    Attempt, Download, DownloadAction, DownloadEvent, DownloadId, DownloadQueue, DownloadTransport,
    Phase, TransportResponseMeta, Validator,
};
use enzui_core::download_destination::{
    DestinationBroker, DestinationOutcome, ReservedDownloadDestination,
};
use enzui_core::io_class::IoFailureCategory;
use enzui_core::portal::backend::PortalBackend;
use enzui_core::portal::chooser::ChooserOptions;
use enzui_core::portal::dbus_backend::DbusBackend;
use enzui_core::portal::{CancelReason, PortalParent};
use enzui_resources::curl_download_transport::CurlDownloadTransport;

#[path = "downloads/aria.rs"]
mod aria;
#[path = "downloads/cleanup.rs"]
mod cleanup;
#[path = "downloads/publication.rs"]
mod publication;
#[path = "downloads/settings.rs"]
mod settings;

use aria::fixture_backend_path;
use aria::run_aria2_torrent;
#[cfg(test)]
use aria::run_aria2_torrent_with_program;
pub(crate) use cleanup::{
    CleanupOwnerState, CleanupRefusal, CleanupSettlement, CleanupSlot, CleanupWorker, HoldRefusal,
    ShutdownOutcome, cleanup_task,
};
use cleanup::{CleanupPhase, SharedTorrentStage};
#[cfg(test)]
use publication::PUBLISH_MAX_DEPTH;
use publication::run_publication_reconciliation;
use publication::{PublicationOutcome, publish_owned_payload};
pub(crate) use publication::{PublicationSettlement, SharedPublicationDebt};
pub(crate) use settings::ARIA2C_BACKEND;
pub(crate) use settings::{
    ADD_DOWNLOAD_HINT, ADD_DOWNLOAD_LABEL, ADD_DOWNLOAD_PLACEHOLDER, ARIA2C_INSTALL_HINT,
    ARIA2C_MISSING_TITLE, DownloadSettings, DownloadSettingsDraft, ENQUEUE_LABEL,
    MEDIA_SITE_UNSUPPORTED, NO_DOWNLOADS, PausedCleanupJob, SETTINGS_REFUSED_TITLE,
    SettingsLoadOutcome, SettingsPersistence, SettingsPublishOutcome, SettingsRefusal,
    TRANSFER_FAILED_TITLE, TorrentPolicy, TorrentTelemetry,
};
use settings::{DEFAULT_CONCURRENCY, MAX_CONCURRENCY, MAX_RATE_KIB};
use settings::{
    SettingsCandidate, io_failure_reason, parse_bounded_settings,
    validate_destination_uri_for_mutation, validate_settings_for_save,
};

#[path = "downloads/core.rs"]
mod core;
#[path = "downloads/destinations.rs"]
mod destinations;
#[path = "downloads/formats.rs"]
mod formats;
#[path = "downloads/state/queue.rs"]
mod queue;
#[path = "downloads/state/recovery.rs"]
mod recovery;
#[path = "downloads/state/setup.rs"]
mod setup;
#[path = "downloads/workers.rs"]
mod workers;

pub(crate) use core::TransportFactory;
pub(crate) use core::is_torrent_source;
pub(crate) use core::{Aria2Status, EmptyState, SystemTransports, TransferJob, run_job};
use core::{
    RateLimitedTransport, consume_cleaned_stage, directory_bytes, obtain_stage_path,
    torrent_failure, try_cleanup_stage,
};
use destinations::DownloadDestinations;
pub(crate) use destinations::{
    DestinationDecision, DestinationRequestError, destination_outcome_text,
};
pub(crate) use formats::{AddDownloadState, is_enqueueable, is_known_service_url};
use formats::{display_name, safe_display_component};
#[cfg(test)]
use workers::ChildGuard;
#[cfg(test)]
#[cfg(target_os = "linux")]
pub(crate) use workers::isolate_process_group;
pub(crate) use workers::{Aria2cProbeResult, probe_aria2c, transfer_task};

#[derive(Debug)]

// The panel's state: the framework queue, the names it cannot carry, the
// backend status and the Add-download dialog.
pub(crate) struct DownloadsState {
    pub(crate) backend: Aria2Status,
    // Owned bounded cleanup executor for paused-download partial removal.
    pub(crate) cleanup_worker: CleanupWorker,
    pub(crate) cleanup_owner_state: CleanupOwnerState,
    // The torrent backend executable, snapshotted at construction. Always
    // `aria2c` except under the capture-only `seed_torrent_fixture`.
    backend_program: PathBuf,
    // The framework queue: admission order, active cap, action legality and
    // bounded history.
    pub(crate) queue: DownloadQueue,
    // Display name per id; the transport-agnostic `Download` carries none.
    names: HashMap<u64, String>,
    // The originating URI per id.
    uris: HashMap<u64, String>,
    // The Add-download dialog, or `None` when closed.
    pub(crate) add: Option<AddDownloadState>,
    // Monotonic id source, so ids are never reused.
    next_id: u64,
    // Transport source, injected so tests move real bytes without a socket.
    pub(crate) transports: Arc<dyn TransportFactory>,
    // Applied, validated plugin settings and the editable draft. Holds no
    // destination authority: each transfer gets its own [`DestinationBroker`] grant.
    pub(crate) settings: DownloadSettings,
    pub(crate) settings_draft: DownloadSettingsDraft,
    // Portal destination authority; only a completed SaveFile outcome grants one.
    destinations: DownloadDestinations,
    // Last URI from a successful portal grant, kept only as the picker's starting
    // hint; loading it never restores write authority.
    pub(crate) last_destination_uri: Option<String>,
    // Validated sources awaiting the save-picker decision; admitted only after the
    // grant is decoded and reserved.
    pending_sources: HashMap<u64, String>,
    pending_display_names: HashMap<u64, String>,
    pending_download_limits: HashMap<u64, u32>,
    download_limits: HashMap<u64, u32>,
    // Per-process namespace beneath a granted torrent container directory.
    staging_namespace: String,
    // The live pause/cancel handle per download.
    controls: HashMap<u64, TransferControl>,
    // HTTP destinations reserved by this session; cancellation removes only these.
    owned_http_partials: HashMap<u64, PathBuf>,
    // Retained file+parent descriptors for identity-bound HTTP partial cleanup.
    http_cleanup_authorities: HashMap<u64, ReservedDownloadDestination>,
    // Torrent root and stage per job, fixed so a settings change cannot redirect
    // a paused job's resume or cleanup.
    torrent_stages: HashMap<u64, (PathBuf, PathBuf)>,
    // Shared torrent stage authorities keyed by download id.
    torrent_shared_stages: HashMap<u64, SharedTorrentStage>,
    torrent_output_dirs: HashMap<u64, enzui_security::secure_dir::RetainedDir>,
    // Retained publication debt from partial-publication failures.
    pub(crate) publication_debts: HashMap<u64, SharedPublicationDebt>,
    // The message a failed transfer ended with, per id, rendered in the panel.
    errors: HashMap<u64, String>,
    // The folder holding each completed download, for the row's Show in
    // Folder action: the saved file's parent, or a torrent's container.
    completed_folders: HashMap<u64, PathBuf>,
    // The validator the first response carried, per id. See
    // [`TransferJob::validator`].
    validators: HashMap<u64, Validator>,
    // Live aria2 detail keyed by download id and attempt, so a sample attaches
    // only to its owning row.
    torrent_telemetry: HashMap<(DownloadId, Attempt), String>,
    // Actionable cleanup failure rendered in the Downloads panel.
    pub(crate) cleanup_error: Option<String>,
    // Outstanding cleanup per download: at most one slot, keyed by the attempt
    // that created the partial.
    cleanup_slots: HashMap<u64, CleanupSlot>,
    // Why the settings source was refused at load. While set, Apply, destination
    // and close-time writes are blocked and the banner offers Retry/Retire.
    pub(crate) settings_refusal: Option<SettingsRefusal>,
    // Set when a Rotated publication left durability or late validation
    // unsettled; Retry republishes the same bytes and success clears it.
    pub(crate) settings_durability_debt: bool,
    // One injected reservation fault, consumed by the real admission path.
    #[cfg(test)]
    test_reservation_error: Option<std::io::Error>,
}

impl Default for DownloadsState {
    fn default() -> Self {
        let settings = DownloadSettings {
            max_download_kib: None,
            max_upload_kib: None,
            concurrency: DEFAULT_CONCURRENCY,
            torrent_policy: TorrentPolicy::DownloadOnly,
        };
        let staging_namespace = format!(
            "session-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        Self {
            backend: Aria2Status::default(),
            cleanup_worker: CleanupWorker::new().unwrap_or_else(CleanupWorker::unavailable),
            cleanup_owner_state: CleanupOwnerState::Ready,
            backend_program: PathBuf::from(ARIA2C_BACKEND),
            queue: DownloadQueue::new(settings.concurrency.get()),
            names: HashMap::new(),
            uris: HashMap::new(),
            add: None,
            next_id: 0,
            transports: Arc::new(SystemTransports),
            settings_draft: DownloadSettingsDraft::from_settings(&settings),
            settings,
            destinations: DownloadDestinations::default(),
            last_destination_uri: None,
            pending_sources: HashMap::new(),
            pending_display_names: HashMap::new(),
            pending_download_limits: HashMap::new(),
            download_limits: HashMap::new(),
            staging_namespace,
            controls: HashMap::new(),
            owned_http_partials: HashMap::new(),
            http_cleanup_authorities: HashMap::new(),
            torrent_stages: HashMap::new(),
            torrent_shared_stages: HashMap::new(),
            publication_debts: HashMap::new(),
            torrent_output_dirs: HashMap::new(),
            errors: HashMap::new(),
            completed_folders: HashMap::new(),
            validators: HashMap::new(),
            torrent_telemetry: HashMap::new(),
            cleanup_error: None,
            cleanup_slots: HashMap::new(),
            settings_refusal: None,
            settings_durability_debt: false,
            #[cfg(test)]
            test_reservation_error: None,
        }
    }
}

impl DownloadsState {
    pub(crate) fn default_download_limit_kib(&self) -> u32 {
        self.settings.max_download_kib.map_or(0, NonZeroU32::get)
    }
}

#[cfg(test)]
#[cfg(not(target_os = "linux"))]
pub(crate) fn isolate_process_group(_command: &mut std::process::Command) {}

#[cfg(test)]
mod transfer_tests;

#[cfg(test)]
mod tests;
