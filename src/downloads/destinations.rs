use super::*;
use enzui_widgets::file_chooser::{FallbackBackend, FallbackEvent};

// The lazy backend avoids starting a portal runtime until a download needs consent.
pub(super) struct DownloadDestinations {
    broker: DestinationBroker,
    backend: Option<FallbackBackend>,
    #[cfg(test)]
    last_requested_folder: Option<PathBuf>,
}

impl std::fmt::Debug for DownloadDestinations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadDestinations")
            .field(
                "backend",
                &self.backend.as_ref().map(PortalBackend::describe),
            )
            .finish_non_exhaustive()
    }
}

impl Default for DownloadDestinations {
    fn default() -> Self {
        Self {
            broker: DestinationBroker::new(),
            backend: None,
            #[cfg(test)]
            last_requested_folder: None,
        }
    }
}

impl DownloadDestinations {
    pub(crate) fn with_backend(backend: Box<dyn PortalBackend>) -> Self {
        Self {
            broker: DestinationBroker::new(),
            backend: Some(FallbackBackend::new(Some(backend))),
            #[cfg(test)]
            last_requested_folder: None,
        }
    }

    fn request(
        &mut self,
        id: DownloadId,
        filename: &str,
        current_folder: Option<&Path>,
    ) -> Option<DestinationOutcome> {
        #[cfg(test)]
        {
            self.last_requested_folder = current_folder.map(Path::to_path_buf);
        }
        let mut options = ChooserOptions::new()
            .title(crate::i18n::tr("Save download"))
            .current_name(filename);
        if let Some(folder) = current_folder {
            options = options.current_folder(folder);
        }
        if self.backend.is_none() {
            let portal = DbusBackend::new()
                .ok()
                .map(|backend| Box::new(backend) as Box<dyn PortalBackend>);
            self.backend = Some(FallbackBackend::new(portal));
        }
        let backend = self.backend.as_mut()?;
        self.broker
            .request_with_options(id, backend, PortalParent::None, options, |_| true)
    }

    pub(crate) fn pump(&mut self) -> Vec<(DownloadId, DestinationOutcome)> {
        let Some(backend) = self.backend.as_mut() else {
            return Vec::new();
        };
        self.broker.pump(backend)
    }

    pub(crate) fn cancel(&mut self, id: DownloadId) {
        let _ = self.broker.cancel(id, CancelReason::Application);
    }

    // Whether the request for `id` has opened its dialog (vs still deferring
    // while the backend's availability settles).
    fn is_started(&self, id: DownloadId) -> bool {
        self.broker.is_started(id)
    }

    fn destination(
        &self,
        id: DownloadId,
    ) -> Option<&enzui_core::download_destination::DownloadDestination> {
        self.broker.destination(id)
    }
}

// A Save request refused before a chooser can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DestinationRequestError {
    // The URL or local torrent source is not supported.
    InvalidSource,
    // A torrent source was requested while aria2c is unavailable.
    TorrentBackendUnavailable,
    // The portal refused before opening a native dialog.
    Portal(DestinationOutcome),
}

// A settled chooser decision before transfer admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DestinationDecision {
    // The selected local path was reserved and the queue row admitted.
    // `admitted_hint` is the validated portal URI for the picker starting
    // hint; the reducer captures `prior_hint` from the live model
    // immediately before each grant so sequential grants compose.
    Granted {
        id: DownloadId,
        // The validated URI this grant admitted, or `None` when the portal
        // URI did not pass admission bounds.
        admitted_hint: Option<String>,
    },
    // The portal completed without granting destination authority.
    Declined(DownloadId, DestinationOutcome),
    // A grant was returned but could not safely become a local reservation.
    Refused(DownloadId, String),
}

// User-facing explanation for a non-success destination outcome.
//
// Cancellation is normal control flow and deliberately returns `None`.
// Every operational refusal remains distinct so an unavailable portal is not
// reported as a denial or a malformed response.
pub(crate) fn destination_outcome_text(outcome: &DestinationOutcome) -> Option<&'static str> {
    match outcome {
        DestinationOutcome::Completed(_) | DestinationOutcome::Cancelled(_) => None,
        DestinationOutcome::Denied => Some(crate::i18n::tr("The save request was denied.")),
        DestinationOutcome::NotDeclared => Some(crate::i18n::tr(
            "This build is not permitted to open the system Save dialog.",
        )),
        DestinationOutcome::Unsupported => Some(crate::i18n::tr(
            "This desktop portal does not support saving files.",
        )),
        DestinationOutcome::Unavailable => Some(crate::i18n::tr(
            "No system Save dialog is available on this desktop.",
        )),
        DestinationOutcome::ParentUnavailable => Some(crate::i18n::tr(
            "The Save dialog could not be attached to this window.",
        )),
        DestinationOutcome::Failed => Some(crate::i18n::tr("The system Save dialog failed.")),
    }
}

impl DownloadsState {
    pub(crate) fn destination_fallback_open(&self) -> bool {
        self.destinations
            .backend
            .as_ref()
            .is_some_and(FallbackBackend::is_dialog_open)
    }

    pub(crate) fn destination_view<'a, Message: Clone + 'a>(
        &'a self,
        content: iced::Element<'a, Message>,
        on_event: impl Fn(FallbackEvent) -> Message + Clone + 'a,
    ) -> iced::Element<'a, Message> {
        match &self.destinations.backend {
            Some(backend) => backend.view(content, on_event),
            None => content,
        }
    }

    pub(crate) fn handle_destination_fallback<Message: Send + 'static>(
        &mut self,
        event: FallbackEvent,
    ) -> (bool, iced::Task<Message>) {
        self.destinations.backend.as_mut().map_or_else(
            || (false, iced::Task::none()),
            |backend| backend.handle(event),
        )
    }

    // An open chooser grants nothing; admission requires a completed broker grant and reservation.
    pub(crate) fn request_destination(
        &mut self,
        uri: &str,
    ) -> Result<DownloadId, DestinationRequestError> {
        self.request_destination_with_limit(
            uri,
            self.settings.max_download_kib.map_or(0, NonZeroU32::get),
            None,
        )
    }

    pub(crate) fn request_destination_with_limit(
        &mut self,
        uri: &str,
        download_limit_kib: u32,
        destination_hint: Option<&Path>,
    ) -> Result<DownloadId, DestinationRequestError> {
        let uri = uri.trim();
        if !is_enqueueable(uri) || is_known_service_url(uri) {
            return Err(DestinationRequestError::InvalidSource);
        }
        if is_torrent_source(uri) && !self.backend.is_ready() {
            return Err(DestinationRequestError::TorrentBackendUnavailable);
        }
        if is_torrent_source(uri) && self.has_torrent_source(uri) {
            return Err(DestinationRequestError::InvalidSource);
        }
        self.start_destination_request(uri.to_owned(), download_limit_kib, destination_hint)
    }

    pub(crate) fn start_destination_request(
        &mut self,
        source: String,
        download_limit_kib: u32,
        destination_hint: Option<&Path>,
    ) -> Result<DownloadId, DestinationRequestError> {
        let id = DownloadId(self.next_id);
        self.next_id += 1;
        let filename = display_name(&source);
        let current_folder = self
            .last_destination_uri
            .as_deref()
            .and_then(|uri| enzui_core::portal::uri::file_uri_to_path(uri).ok())
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| destination_hint.map(Path::to_path_buf));
        self.pending_sources.insert(id.0, source);
        self.pending_display_names.insert(id.0, filename.clone());
        self.pending_download_limits
            .insert(id.0, download_limit_kib);
        if let Some(outcome) = self
            .destinations
            .request(id, &filename, current_folder.as_deref())
        {
            self.pending_sources.remove(&id.0);
            self.pending_display_names.remove(&id.0);
            self.pending_download_limits.remove(&id.0);
            return Err(DestinationRequestError::Portal(outcome));
        }
        Ok(id)
    }

    // The live save-destination wake, delegated to the broker's pending-gated
    // `completion_subscription` (broker-owned request identity, the
    // cancel-before-pump gate, and stream-drop waiter cleanup).
    pub(crate) fn destination_completion_subscription(
        &self,
    ) -> iced::Subscription<enzui_widgets::file_chooser::PortalWake> {
        self.destinations.broker.completion_subscription(
            self.destinations
                .backend
                .as_ref()
                .map(|backend| backend as &dyn PortalBackend),
        )
    }

    // The Add-download dialog remains until a Save chooser starts or the request settles.
    pub(crate) fn destination_started(&self, id: DownloadId) -> bool {
        self.destinations.is_started(id)
    }

    // Cancels and settles a destination request; the pump stops a deferred
    // request from opening and sends a started request's `Close`.
    pub(crate) fn cancel_destination(&mut self, id: DownloadId) {
        self.destinations.cancel(id);
        let _ = self.pump_destinations();
    }

    // Poll the destination broker and admit only downloads carrying a grant.
    pub(crate) fn pump_destinations(&mut self) -> Vec<DestinationDecision> {
        self.destinations
            .pump()
            .into_iter()
            .map(|(id, outcome)| match outcome {
                DestinationOutcome::Completed(_) => match self.admit_granted(id) {
                    Ok(admitted_hint) => DestinationDecision::Granted { id, admitted_hint },
                    Err(error) => DestinationDecision::Refused(id, error),
                },
                other => {
                    self.pending_sources.remove(&id.0);
                    self.pending_display_names.remove(&id.0);
                    self.pending_download_limits.remove(&id.0);
                    DestinationDecision::Declined(id, other)
                }
            })
            .collect()
    }

    // Creates the torrent staging hierarchy and returns its `DirectoryStage`.
    // The destination must satisfy `SecureRoot` private-owner policy.
    pub(crate) fn create_torrent_stage(
        &self,
        destination: &Path,
        id: DownloadId,
    ) -> std::io::Result<enzui_security::tempstage::DirectoryStage> {
        use enzui_security::secure_dir::SecureRoot;
        use enzui_security::tempstage::DirectoryStage;
        use std::ffi::OsString;

        std::fs::create_dir_all(destination)?;
        let controlled_path = destination.join(".files-app-transfers");
        enzui_core::secure_dir::ensure_private_dir(&controlled_path).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                crate::i18n::format(
                    "files-the-selected-torrent-destination-does-not-8bdf5599",
                    &[("e", (e).to_string())],
                ),
            )
        })?;
        let controlled = SecureRoot::open(&controlled_path).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                crate::i18n::format(
                    "files-could-not-open-private-staging-root-75c5aa26",
                    &[("e", (e).to_string())],
                ),
            )
        })?;
        let session = controlled.ensure_subroot(std::ffi::OsStr::new(&self.staging_namespace))?;
        let stage_name = OsString::from(format!("torrent-{}", id.0));
        DirectoryStage::create(&session, &stage_name)
    }

    // Reserves the granted local path before admitting the source to the queue.
    // Leaves `last_destination_uri` alone; the reducer applies the hint.
    fn admit_granted(&mut self, id: DownloadId) -> Result<Option<String>, String> {
        let source = self.pending_sources.remove(&id.0).ok_or_else(|| {
            crate::i18n::tr("The save request no longer has a download source").to_owned()
        })?;
        let display_name = self
            .pending_display_names
            .remove(&id.0)
            .unwrap_or_else(|| display_name(&source));
        let download_limit_kib = self.pending_download_limits.remove(&id.0).unwrap_or(0);
        let granted = self.destinations.destination(id).ok_or_else(|| {
            crate::i18n::tr("The save request completed without a destination grant").to_owned()
        })?;
        let destination_uri = granted.uri().to_owned();
        let destination = granted.to_path().map_err(|error| {
            crate::i18n::format(
                "files-the-selected-destination-is-not-a-e999f51c",
                &[("error", (error).to_string())],
            )
        })?;
        if !destination.is_absolute() {
            return Err(
                crate::i18n::tr("The selected destination is not an absolute local path").into(),
            );
        }

        let torrent = is_torrent_source(&source);
        #[cfg(test)]
        let injected = self.test_reservation_error.take();
        #[cfg(not(test))]
        let injected: Option<std::io::Error> = None;
        let mut http_reservation = None;
        let mut torrent_stage: Option<enzui_security::tempstage::DirectoryStage> = None;
        let reservation = if let Some(error) = injected {
            Err(error)
        } else if torrent {
            self.create_torrent_stage(&destination, id).map(|stage| {
                torrent_stage = Some(stage);
            })
        } else {
            granted.reserve_file().map(|reservation| {
                http_reservation = Some(reservation);
            })
        };
        reservation.map_err(|error| {
            crate::i18n::format(
                "files-could-not-reserve-the-selected-93d9f9e4",
                &[
                    (
                        "arg0",
                        (if torrent {
                            crate::i18n::tr("torrent destination")
                        } else {
                            crate::i18n::tr("download filename")
                        })
                        .to_owned(),
                    ),
                    (
                        "arg1",
                        (io_failure_reason(IoFailureCategory::classify(&error))).to_owned(),
                    ),
                ],
            )
        })?;

        if !self.queue.admit(id) {
            if let Some(reservation) = &http_reservation {
                let _ = reservation.remove_if_reserved();
            }
            drop(torrent_stage);
            return Err(
                crate::i18n::tr("Could not admit the granted download to the queue").into(),
            );
        }
        self.names.insert(id.0, display_name);
        self.uris.insert(id.0, source);
        self.download_limits.insert(id.0, download_limit_kib);
        let admitted_hint = if validate_destination_uri_for_mutation(&destination_uri) {
            Some(destination_uri)
        } else {
            None
        };
        if torrent {
            let stage_obj = torrent_stage.expect("torrent stage created at admission");
            let stage_path = stage_obj.path().to_path_buf();
            let shared: SharedTorrentStage = Arc::new(Mutex::new(Some(stage_obj)));
            if let Ok(retained) = enzui_security::secure_dir::RetainedDir::open(&destination) {
                self.torrent_output_dirs.insert(id.0, retained);
            }
            self.torrent_stages.insert(id.0, (destination, stage_path));
            self.torrent_shared_stages.insert(id.0, shared);
        } else {
            self.owned_http_partials.insert(id.0, destination);
            if let Some(reservation) = http_reservation {
                self.http_cleanup_authorities.insert(id.0, reservation);
            }
        }
        Ok(admitted_hint)
    }
}
