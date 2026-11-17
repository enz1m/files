// Enqueue, jobs, transfers, pause, and cancel.

use super::*;

impl DownloadsState {
    #[cfg(test)]
    pub(crate) fn set_destination_backend(&mut self, backend: Box<dyn PortalBackend>) {
        self.destinations = DownloadDestinations::with_backend(backend);
    }

    // Ask where a selected local `.torrent` payload may write.
    //
    // The source is canonicalised before it becomes an aria2 positional
    // argument, but that readable source path grants no destination authority;
    // queue admission still waits for the SaveFile broker.
    pub(crate) fn request_torrent_destination(
        &mut self,
        path: &Path,
    ) -> Result<DownloadId, DestinationRequestError> {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| DestinationRequestError::InvalidSource)?;
        if !self.backend.is_ready() {
            return Err(DestinationRequestError::TorrentBackendUnavailable);
        }
        if !metadata.file_type().is_file()
            || !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
        {
            return Err(DestinationRequestError::InvalidSource);
        }
        let source = path
            .canonicalize()
            .map_err(|_| DestinationRequestError::InvalidSource)?
            .to_string_lossy()
            .into_owned();
        if self.has_torrent_source(&source) {
            return Err(DestinationRequestError::InvalidSource);
        }
        self.start_destination_request(
            source,
            self.settings.max_download_kib.map_or(0, NonZeroU32::get),
            None,
        )
    }

    // Whether this exact torrent source already has a live or retryable row.
    // Failed and cancelled rows count because they offer Retry; completed
    // history does not block a deliberate new download.
    pub(crate) fn has_torrent_source(&self, source: &str) -> bool {
        self.pending_sources
            .values()
            .any(|pending| pending == source)
            || self.uris.iter().any(|(raw, existing)| {
                existing == source
                    && self
                        .queue
                        .get(DownloadId(*raw))
                        .is_some_and(|download| download.phase != Phase::Complete)
            })
    }

    pub(crate) fn has_torrent_path(&self, path: &Path) -> bool {
        path.canonicalize()
            .ok()
            .is_some_and(|path| self.has_torrent_source(&path.to_string_lossy()))
    }

    // Apply a row action through the framework queue, returning whether it was
    // accepted; legality is `Download::apply`'s decision.
    pub(crate) fn apply(&mut self, id: DownloadId, action: DownloadAction) -> bool {
        self.queue.apply(id, action)
    }

    // The display name for a download, for `DownloadRow::new`.
    pub(crate) fn name_of(&self, id: DownloadId) -> &str {
        self.names.get(&id.0).map_or("", String::as_str)
    }

    // The originating URI for a download. The panel renders its derived name.

    // The downloads to render, in admission order.
    pub(crate) fn downloads(&self) -> impl Iterator<Item = &Download> {
        self.queue.iter()
    }

    // Accept one aria2 sample only for the live generation that produced it.
    //
    // Lookup is by id and attempt, so a late worker from an earlier retry
    // cannot decorate the row now showing its replacement. Terminal rows and a
    // removed provider reject samples and clear existing detail.
    pub(crate) fn record_torrent_telemetry(&mut self, sample: TorrentTelemetry) -> bool {
        let Some(download) = self.queue.get(sample.id) else {
            return false;
        };
        if !self.backend.is_ready()
            || download.attempt != sample.attempt
            || download.phase.is_terminal()
        {
            return false;
        }
        let key = (sample.id, sample.attempt);
        let detail = sample.secondary_detail();
        self.torrent_telemetry.retain(|(id, _), _| *id != sample.id);
        self.torrent_telemetry.insert(key, detail);
        true
    }

    // Secondary detail belonging to exactly this stable row generation.
    pub(crate) fn torrent_detail(&self, download: &Download) -> Option<&str> {
        (!download.phase.is_terminal())
            .then(|| {
                self.torrent_telemetry
                    .get(&(download.id, download.attempt))
                    .map(String::as_str)
            })
            .flatten()
    }

    // Retire every generation of one row's transport detail.
    pub(crate) fn clear_torrent_telemetry(&mut self, id: DownloadId) {
        self.torrent_telemetry
            .retain(|(sample_id, _), _| *sample_id != id);
    }

    // Clear detail once the authoritative queue says the row is terminal.
    pub(crate) fn clear_terminal_torrent_telemetry(&mut self, id: DownloadId) {
        if self
            .queue
            .get(id)
            .is_none_or(|download| download.phase.is_terminal())
        {
            self.clear_torrent_telemetry(id);
        }
    }

    // Provider disable/removal retires every sample immediately.
    pub(crate) fn clear_all_torrent_telemetry(&mut self) {
        self.torrent_telemetry.clear();
    }

    // Install the probe result and retire telemetry if the provider is no
    // longer usable. A found `aria2c` is stored as its resolved absolute path,
    // so a later PATH change cannot redirect spawns; Ready without a resolved
    // path is refused.
    pub(crate) fn set_backend_from_probe(&mut self, result: Aria2cProbeResult) {
        match (result.status, result.resolved_path) {
            (Aria2Status::Ready, Some(path)) => {
                self.backend_program = path;
                self.backend = Aria2Status::Ready;
            }
            (Aria2Status::Ready, None) => {
                self.backend = Aria2Status::Absent;
                self.clear_all_torrent_telemetry();
            }
            (status, _) => {
                self.backend = status;
                if !status.is_ready() {
                    self.clear_all_torrent_telemetry();
                }
            }
        }
    }

    pub(crate) fn set_backend(&mut self, status: Aria2Status) {
        self.backend = status;
        if !status.is_ready() {
            self.clear_all_torrent_telemetry();
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.queue.len()
    }

    // The empty state to render, or `None` when jobs exist.
    pub(crate) fn empty_state(&self) -> Option<EmptyState> {
        EmptyState::resolve(self.backend, self.queue.len())
    }

    pub(crate) fn can_add(&self) -> bool {
        true
    }

    // Where an HTTP download's bytes are written, if its portal grant was
    // atomically reserved. `None` is the least-authority gate used by
    // [`job_for`](Self::job_for).
    pub(crate) fn destination(&self, id: DownloadId) -> Option<PathBuf> {
        self.owned_http_partials.get(&id.0).cloned()
    }

    // The transfers the queue's concurrency cap has just released, as
    // runnable jobs. Cap, order and the `Queued -> Active` transition belong to
    // `DownloadQueue`.
    pub(crate) fn promote(&mut self) -> Vec<TransferJob> {
        let mut jobs = Vec::new();
        loop {
            let _ = self.queue.promote();
            let startable: Vec<_> = self
                .queue
                .iter()
                .filter(|download| {
                    download.phase == Phase::Active && !self.controls.contains_key(&download.id.0)
                })
                .map(|download| (download.id, download.attempt))
                .collect();
            if startable.is_empty() {
                break;
            }
            let mut failed = false;
            for (id, attempt) in startable {
                match self.job_for(id) {
                    Some(job) => jobs.push(job),
                    None => {
                        failed = true;
                        self.errors.insert(
                            id.0,
                            crate::i18n::tr(
                                "Could not allocate a collision-free owned download file",
                            )
                            .into(),
                        );
                        let _ = self.queue.finish(id, attempt, Phase::Failed);
                    }
                }
            }
            if !failed {
                break;
            }
        }
        jobs
    }

    // Build the job for `id`, arming a fresh control.
    fn job_for(&mut self, id: DownloadId) -> Option<TransferJob> {
        let uri = self.uris.get(&id.0)?.clone();
        let attempt = self.queue.get(id)?.attempt;
        let torrent = is_torrent_source(&uri);
        let destination = if torrent {
            self.torrent_stages.get(&id.0)?.1.clone()
        } else {
            self.destination(id)?
        };
        let control = TransferControl::new();
        self.controls.insert(id.0, control.clone());
        let download_limit_kib = self
            .download_limits
            .get(&id.0)
            .copied()
            .unwrap_or_else(|| self.default_download_limit_kib());
        let transport = self.transports.transport(&uri, self.backend);
        let transport = if !torrent && download_limit_kib > 0 {
            Box::new(RateLimitedTransport::new(transport, download_limit_kib))
                as Box<dyn DownloadTransport>
        } else {
            transport
        };
        let validator = self.validators.get(&id.0).cloned();
        Some(TransferJob {
            id,
            attempt,
            uri,
            destination,
            control,
            validator,
            transport,
            download_limit_kib,
            upload_limit_kib: self.settings.max_upload_kib.map_or(0, NonZeroU32::get),
            torrent_policy: self.settings.torrent_policy,
            torrent_output_dir: self.torrent_output_dirs.get(&id.0).cloned(),
            torrent_shared_stage: self.torrent_shared_stages.get(&id.0).cloned(),
            publication_debt: {
                let arc = self
                    .publication_debts
                    .entry(id.0)
                    .or_insert_with(|| Arc::new(Mutex::new(None)));
                debug_assert!(
                    arc.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_none(),
                    "stale publication debt carried into new TransferJob"
                );
                std::sync::Arc::clone(arc)
            },
            backend_program: self.backend_program.clone(),
        })
    }

    // Only the current attempt may supply the validator used by a later conditional resume.
    pub(crate) fn remember_response(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
        meta: &TransportResponseMeta,
    ) -> bool {
        if !self
            .queue
            .get(id)
            .is_some_and(|download| download.accepts(attempt))
        {
            return false;
        }
        if let Some(validator) = meta.response_validator.clone() {
            self.validators.insert(id.0, validator);
        }
        true
    }

    // Fold one transfer-engine [`DownloadEvent`] into the panel, returning the
    // download it belongs to.
    //
    // The queue decides staleness before attempt-local state (resume
    // validator, failure copy) changes, so a superseded attempt cannot
    // overwrite the current one.
    pub(crate) fn apply_transfer_event(&mut self, event: &DownloadEvent) -> DownloadId {
        let id = event.id();
        let attempt = event.attempt();
        let applied = self.queue.apply_event(event);
        match event {
            DownloadEvent::ResponseMeta { meta, .. } => {
                self.remember_response(id, attempt, meta);
            }
            DownloadEvent::Failed { error, .. } if applied => {
                self.record_event_failure(id, error);
            }
            _ => {}
        }
        self.clear_terminal_torrent_telemetry(id);
        id
    }

    // Tell a running transfer to stop and keep its partial file.
    pub(crate) fn pause(&self, id: DownloadId) {
        if let Some(control) = self.controls.get(&id.0) {
            control.pause();
        }
    }

    // Stop every running transfer and revoke every pending Save dialog when
    // the whole module is disabled.
    pub(crate) fn cancel_all(&mut self) {
        for (_, control) in self
            .controls
            .iter()
            .collect::<std::collections::BTreeMap<_, _>>()
        {
            control.cancel();
        }
        let pending: Vec<_> = self.pending_sources.keys().copied().collect();
        for raw in pending {
            self.destinations.cancel(DownloadId(raw));
        }
        // Settle cancellation in either chooser backend before releasing pending source authority.
        let _ = self.destinations.pump();
        self.pending_sources.clear();
        self.pending_display_names.clear();
        self.pending_download_limits.clear();
    }

    // Tell a running transfer to stop and delete its partial file.
    pub(crate) fn cancel(&self, id: DownloadId) {
        if let Some(control) = self.controls.get(&id.0) {
            control.cancel();
        }
    }
}
