// Downloads-panel reducer arms.

use super::*;

impl App {
    pub(crate) fn reduce_torrent_download_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentDownloadLimitChanged(value) = message else {
            unreachable!("misrouted message in reduce_torrent_download_limit_changed");
        };
        self.model.downloads.settings_draft.max_download_kib = value;
        self.model.downloads.settings_draft.error = None;
        Task::none()
    }

    pub(crate) fn reduce_torrent_upload_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentUploadLimitChanged(value) = message else {
            unreachable!("misrouted message in reduce_torrent_upload_limit_changed");
        };
        self.model.downloads.settings_draft.max_upload_kib = value;
        self.model.downloads.settings_draft.error = None;
        Task::none()
    }

    pub(crate) fn reduce_download_concurrency_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadConcurrencyChanged(value) = message else {
            unreachable!("misrouted message in reduce_download_concurrency_changed");
        };
        self.model.downloads.settings_draft.concurrency = value;
        self.model.downloads.settings_draft.error = None;
        Task::none()
    }

    pub(crate) fn reduce_apply_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ApplyDownloadSettings = message else {
            unreachable!("misrouted message in reduce_apply_download_settings");
        };
        if self.model.downloads.has_settings_debt_or_refusal() {
            self.model.downloads.settings_draft.error = Some(
                if self.model.downloads.is_settings_refused() {
                    crate::i18n::tr("Settings are refused; use Retry or Replace")
                } else {
                    crate::i18n::tr("Settings have a pending durability issue; use Retry")
                }
                .into(),
            );
            return Task::none();
        }
        // Restored if the publish leaves the file unchanged.
        let old_settings = self.model.downloads.settings.clone();
        let old_draft = self.model.downloads.settings_draft.clone();
        let old_uri = self.model.downloads.last_destination_uri.clone();
        match self.model.downloads.apply_settings() {
            Ok(()) => {
                match self
                    .model
                    .downloads
                    .try_publish(&mut self.downloads_settings)
                {
                    // Ok: commit the new model and clear durability debt.
                    Ok(()) => {
                        self.model.downloads.settings_durability_debt = false;
                        return self.start_promoted();
                    }
                    // Unchanged: roll back to the pre-Apply snapshot.
                    Err(downloads::SettingsPublishOutcome::Unchanged) => {
                        self.model.downloads.settings = old_settings;
                        self.model.downloads.settings_draft = old_draft;
                        self.model.downloads.last_destination_uri = old_uri;
                        self.model
                            .downloads
                            .queue
                            .set_max_active(self.model.downloads.settings.concurrency.get());
                        self.model.downloads.settings_draft.error =
                            Some(crate::i18n::tr("settings could not be saved").into());
                    }
                    // Rotated: the new model is live but owes durability.
                    Err(downloads::SettingsPublishOutcome::Rotated) => {
                        self.model.downloads.settings_durability_debt = true;
                        return self.start_promoted();
                    }
                    Err(downloads::SettingsPublishOutcome::Lost) => {
                        self.model.downloads.settings_refusal =
                            Some(downloads::SettingsRefusal::ReadFailed);
                    }
                    Err(downloads::SettingsPublishOutcome::Unavailable) => {
                        self.model.downloads.settings_refusal =
                            Some(downloads::SettingsRefusal::AuthorityUnavailable);
                    }
                    #[cfg(test)]
                    Err(downloads::SettingsPublishOutcome::Disarmed) => {
                        return self.start_promoted();
                    }
                }
            }
            Err(error) => self.model.downloads.settings_draft.error = Some(error),
        }
        Task::none()
    }

    pub(crate) fn reduce_retry_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetryDownloadSettings = message else {
            unreachable!("misrouted message in reduce_retry_download_settings");
        };
        if self.model.downloads.settings_durability_debt
            && self.model.downloads.settings_refusal.is_none()
        {
            // Durability debt republishes the retained model bytes.
            match self
                .model
                .downloads
                .try_publish(&mut self.downloads_settings)
            {
                Ok(()) => {
                    self.model.downloads.settings_durability_debt = false;
                }
                Err(downloads::SettingsPublishOutcome::Unchanged) => {
                    // Retained model unchanged, debt persists.
                }
                Err(downloads::SettingsPublishOutcome::Rotated) => {
                    // Already rotated; debt persists.
                }
                Err(downloads::SettingsPublishOutcome::Lost) => {
                    self.model.downloads.settings_durability_debt = false;
                    self.model.downloads.settings_refusal =
                        Some(downloads::SettingsRefusal::ReadFailed);
                }
                Err(downloads::SettingsPublishOutcome::Unavailable) => {
                    self.model.downloads.settings_durability_debt = false;
                    self.model.downloads.settings_refusal =
                        Some(downloads::SettingsRefusal::AuthorityUnavailable);
                }
                #[cfg(test)]
                Err(downloads::SettingsPublishOutcome::Disarmed) => {
                    self.model.downloads.settings_durability_debt = false;
                }
            }
        } else if self.model.downloads.settings_refusal.is_some() {
            // Refusal: reacquire the authority if needed, then reload.
            if self.downloads_settings.is_unavailable() {
                self.downloads_settings.try_reacquire();
            }
            if !self.downloads_settings.is_unavailable() {
                let outcome = self
                    .model
                    .downloads
                    .load_settings_bounded(&self.downloads_settings);
                match outcome {
                    downloads::SettingsLoadOutcome::Applied
                    | downloads::SettingsLoadOutcome::Absent => {
                        self.model.downloads.settings_refusal = None;
                        self.model.downloads.settings_durability_debt = false;
                    }
                    _ => {} // refusal persists from load
                }
            }
        }
        // Ordinary Retry with neither refusal nor debt is inert.
        Task::none()
    }

    pub(crate) fn reduce_retire_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetireDownloadSettings = message else {
            unreachable!("misrouted message in reduce_retire_download_settings");
        };
        match downloads::DownloadsState::try_publish_defaults(&mut self.downloads_settings) {
            Ok(()) => {
                self.model.downloads.adopt_defaults();
            }
            // Unchanged keeps the old model and refusal.
            Err(downloads::SettingsPublishOutcome::Unchanged) => {}
            // Rotated adopts the defaults and clears the refusal, owing durability.
            Err(downloads::SettingsPublishOutcome::Rotated) => {
                self.model.downloads.adopt_defaults();
                self.model.downloads.settings_durability_debt = true;
            }
            // Lost keeps the old model and enters custody refusal.
            Err(downloads::SettingsPublishOutcome::Lost) => {
                self.model.downloads.settings_refusal =
                    Some(downloads::SettingsRefusal::ReadFailed);
                self.model.downloads.settings_durability_debt = false;
            }
            Err(downloads::SettingsPublishOutcome::Unavailable) => {
                // Preserve old model and refusal.
            }
            #[cfg(test)]
            Err(downloads::SettingsPublishOutcome::Disarmed) => {
                self.model.downloads.adopt_defaults();
            }
        }
        Task::none()
    }

    pub(crate) fn reduce_aria2_probed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Aria2Probed(result) = message else {
            unreachable!("misrouted message in reduce_aria2_probed");
        };
        if self.model.features.enabled(features::FeatureId::Downloads) {
            self.model.downloads.set_backend_from_probe(result);
        }
        Task::none()
    }

    pub(crate) fn reduce_show_add_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowAddDownload = message else {
            unreachable!("misrouted message in reduce_show_add_download");
        };
        // Guarded by feature enablement; HTTP remains available even
        // when the optional torrent backend is absent.
        if self.model.features.enabled(features::FeatureId::Downloads)
            && self.model.downloads.can_add()
        {
            self.model.downloads.add = Some(downloads::AddDownloadState::new(
                self.model.home.join("Downloads"),
                self.model.downloads.default_download_limit_kib(),
            ));
        }
        Task::none()
    }

    pub(crate) fn reduce_close_add_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseAddDownload = message else {
            unreachable!("misrouted message in reduce_close_add_download");
        };
        // Cancel and settle before the pending-gated wake disappears.
        if let Some(id) = self.model.downloads.add.as_ref().and_then(|a| a.submitting) {
            self.model.downloads.cancel_destination(id);
        }
        self.model.downloads.add = None;
        Task::none()
    }

    pub(crate) fn reduce_add_download_url_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AddDownloadUrlChanged(url) = message else {
            unreachable!("misrouted message in reduce_add_download_url_changed");
        };
        if let Some(a) = &mut self.model.downloads.add {
            a.url = url;
            a.error = None;
        }
        Task::none()
    }

    pub(crate) fn reduce_toggle_download_advanced(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleDownloadAdvanced = message else {
            unreachable!("misrouted message in reduce_toggle_download_advanced");
        };
        if let Some(add) = &mut self.model.downloads.add {
            add.advanced_open = !add.advanced_open;
        }
        Task::none()
    }

    pub(crate) fn reduce_add_download_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AddDownloadLimitChanged(value) = message else {
            unreachable!("misrouted message in reduce_add_download_limit_changed");
        };
        if let Some(add) = &mut self.model.downloads.add {
            add.speed_limit_kib = value;
            add.error = None;
        }
        Task::none()
    }

    pub(crate) fn reduce_enqueue_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::EnqueueDownload = message else {
            unreachable!("misrouted message in reduce_enqueue_download");
        };
        if !self.model.features.enabled(features::FeatureId::Downloads) {
            return Task::none();
        }
        // One destination request per dialog; the backend may still be
        // settling availability.
        if self
            .model
            .downloads
            .add
            .as_ref()
            .and_then(|a| a.submitting)
            .is_some()
        {
            return Task::none();
        }
        let Some(add) = self.model.downloads.add.as_ref() else {
            return Task::none();
        };
        let url = add.url.clone();
        let destination_hint = add.destination_hint.clone();
        let download_limit_kib = match add.speed_limit() {
            Ok(limit) => limit,
            Err(error) => {
                if let Some(add) = self.model.downloads.add.as_mut() {
                    add.error = Some(error);
                }
                return Task::none();
            }
        };
        if add.is_media_site() {
            if let Some(add) = self.model.downloads.add.as_mut() {
                add.error = Some(crate::i18n::tr(downloads::MEDIA_SITE_UNSUPPORTED).into());
            }
            return Task::none();
        }
        let request = self.model.downloads.request_destination_with_limit(
            &url,
            download_limit_kib,
            Some(&destination_hint),
        );
        match request {
            Ok(id) => {
                // Keep the dialog open and disabled until the native picker
                // opens or a terminal outcome arrives, so a portal still
                // initialising never reads as a missing Save dialog.
                if let Some(add) = self.model.downloads.add.as_mut() {
                    add.submitting = Some(id);
                    add.error = None;
                }
            }
            Err(downloads::DestinationRequestError::InvalidSource) => {
                if let Some(add) = self.model.downloads.add.as_mut() {
                    add.error = Some(
                        crate::i18n::tr("Enter an HTTP or HTTPS URL, or a magnet link.").into(),
                    );
                }
            }
            Err(downloads::DestinationRequestError::TorrentBackendUnavailable) => {
                if let Some(add) = self.model.downloads.add.as_mut() {
                    add.error = Some(
                        crate::i18n::tr("Magnet links require the optional aria2 torrent backend.")
                            .into(),
                    );
                }
            }
            Err(downloads::DestinationRequestError::Portal(outcome)) => {
                if let Some(add) = self.model.downloads.add.as_mut() {
                    add.error = downloads::destination_outcome_text(&outcome).map(str::to_owned);
                }
            }
        }
        Task::none()
    }

    pub(crate) fn reduce_enqueue_torrent(&mut self, message: Msg) -> Task<Msg> {
        let Msg::EnqueueTorrent(path) = message else {
            unreachable!("misrouted message in reduce_enqueue_torrent");
        };
        if !self.model.features.enabled(features::FeatureId::Downloads) {
            return Task::none();
        }
        let duplicate = self.model.downloads.has_torrent_path(&path);
        match self.model.downloads.request_torrent_destination(&path) {
            Ok(_) => Task::none(),
            Err(downloads::DestinationRequestError::Portal(outcome)) => {
                if let Some(text) = downloads::destination_outcome_text(&outcome) {
                    self.post_status(text, ToastTone::Error);
                }
                Task::none()
            }
            Err(downloads::DestinationRequestError::TorrentBackendUnavailable) => {
                self.post_status(
                    crate::i18n::tr("Torrent downloads require the Downloads extension and aria2c"),
                    ToastTone::Warning,
                );
                Task::none()
            }
            Err(downloads::DestinationRequestError::InvalidSource) => {
                self.post_status(
                        if duplicate {
                            crate::i18n::tr("That torrent is already in Download Manager; use its existing row to resume or retry")
                        } else {
                            crate::i18n::tr("Only a regular local .torrent file can be downloaded")
                        },
                        ToastTone::Warning,
                    );
                Task::none()
            }
        }
    }

    pub(crate) fn reduce_download_action(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadAction(id, action) = message else {
            unreachable!("misrouted message in reduce_download_action");
        };
        if !self.model.features.enabled(features::FeatureId::Downloads) {
            return Task::none();
        }
        // Retry is refused before queue mutation while cleanup is unresolved.
        if action == enzui_core::download::DownloadAction::Retry
            && !self.model.downloads.can_retry(id)
        {
            return Task::none();
        }
        let was_paused = self
            .model
            .downloads
            .queue
            .get(id)
            .is_some_and(|download| download.phase == enzui_core::download::Phase::Paused);
        if !enzui_core::operation_activity::downloads::apply(
            &mut self.model.downloads.queue,
            enzui_core::operation_activity::downloads::key_of(id),
            enzui_core::operation_activity::downloads::control_of(action),
        ) {
            return Task::none();
        }
        let paused_cleanup = (was_paused && action == enzui_core::download::DownloadAction::Cancel)
            .then(|| self.model.downloads.paused_cleanup_job(id))
            .flatten();
        if matches!(
            action,
            enzui_core::download::DownloadAction::Cancel
                | enzui_core::download::DownloadAction::Retry
        ) {
            self.model.downloads.clear_torrent_telemetry(id);
        }
        if action == enzui_core::download::DownloadAction::Retry
            && !self.model.downloads.begin_retry(id)
        {
            return Task::none();
        }
        match action {
            enzui_core::download::DownloadAction::Pause => {
                self.model.downloads.pause(id);
            }
            enzui_core::download::DownloadAction::Cancel if !was_paused => {
                self.model.downloads.cancel(id);
            }
            enzui_core::download::DownloadAction::Cancel
            | enzui_core::download::DownloadAction::Resume
            | enzui_core::download::DownloadAction::Retry => {}
        }
        self.publish_download(id);
        let promoted = self.start_promoted();
        match paused_cleanup {
            Some(job) => {
                let cid = job.id;
                if self.model.downloads.hold_cleanup(job).is_ok() {
                    match self.model.downloads.submit_held_cleanup(cid) {
                        Some(cleanup) => Task::batch([promoted, cleanup]),
                        None => promoted,
                    }
                } else {
                    promoted
                }
            }
            None => promoted,
        }
    }

    pub(crate) fn reduce_torrent_telemetry(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentTelemetry(progress) = message else {
            unreachable!("misrouted message in reduce_torrent_telemetry");
        };
        if self.model.features.enabled(features::FeatureId::Downloads) {
            let _ = self.model.downloads.record_torrent_telemetry(progress);
        }
        Task::none()
    }

    pub(crate) fn reduce_transfer_event(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TransferEvent(event) = message else {
            unreachable!("misrouted message in reduce_transfer_event");
        };
        // Staleness is decided first, so a superseded attempt's late response
        // cannot overwrite the current resume validator or failure copy.
        let id = self.model.downloads.apply_transfer_event(&event);
        self.publish_download(id);
        Task::none()
    }

    pub(crate) fn reduce_transfer_outcome(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TransferOutcome(id, attempt, outcome) = message else {
            unreachable!("misrouted message in reduce_transfer_outcome");
        };
        let has_pub_debt = self.model.downloads.has_publication_debt(id);
        // Paused stages remain resumable; publication debt retains source
        // identities that reconciliation still needs.
        let cleanup_allowed = !has_pub_debt
            && !matches!(
                outcome,
                enzui_core::download::transfer::TransferOutcome::Paused { .. }
            );
        if cleanup_allowed {
            // Stage cleanup is captured before `record_outcome`.
            if let Some(stage_job) = self.model.downloads.stage_cleanup_job(id, attempt) {
                match self.model.downloads.hold_cleanup(stage_job) {
                    Ok(()) | Err(downloads::HoldRefusal::AlreadyHeld) => {}
                    Err(downloads::HoldRefusal::Conflict) => {}
                }
            }
        }
        if self.model.downloads.record_outcome(id, attempt, &outcome) {
            self.publish_download(id);
            let promoted = self.start_promoted();
            if cleanup_allowed {
                if let Some(cleanup) = self.model.downloads.submit_held_cleanup(id) {
                    return Task::batch([promoted, cleanup]);
                }
                if let Some(cleanup) = self.model.downloads.submit_drain_slot(id) {
                    return Task::batch([promoted, cleanup]);
                }
            }
            promoted
        } else {
            Task::none()
        }
    }

    pub(crate) fn reduce_publication_reconciled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PublicationReconciled(ps) = message else {
            unreachable!("misrouted message in reduce_publication_reconciled");
        };
        let id = ps.id;
        if !self.model.downloads.accepts_attempt(id, ps.attempt) {
            // Stale settlement; leave custody visible but inert.
            return Task::none();
        }
        if ps.complete {
            self.model.downloads.publication_debts.remove(&id.0);
            self.publish_download(id);
            // With publication debt cleared, a live stage may be cleaned up.
            if let Some(stage_job) = self.model.downloads.stage_cleanup_job(id, ps.attempt) {
                let _ = self.model.downloads.hold_cleanup(stage_job);
                if let Some(cleanup) = self.model.downloads.submit_drain_slot(id) {
                    return cleanup;
                }
            }
            if self.close_pending.is_some()
                && !self.model.downloads.has_cleanup_debt()
                && self.model.downloads.cleanup_idle()
            {
                return self.update(Msg::CleanupDrainComplete);
            }
        } else if let Some(detail) = ps.detail {
            self.model.downloads.record_cleanup_error(id, detail);
        }
        Task::none()
    }

    pub(crate) fn reduce_paused_download_cleaned(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PausedDownloadCleaned(settlement) = message else {
            unreachable!("misrouted message in reduce_paused_download_cleaned");
        };
        let id = settlement.id;
        let attempt = settlement.attempt;
        match settlement.result {
            Ok(()) => {
                self.model.downloads.finish_paused_cleanup(id, attempt);
            }
            Err(downloads::CleanupRefusal::Io | downloads::CleanupRefusal::Incomplete) => {
                self.model.downloads.record_cleanup_debt(id, attempt);
                if let Some(msg) = settlement.detail {
                    self.model.downloads.record_cleanup_error(id, msg);
                }
            }
            Err(
                downloads::CleanupRefusal::Busy
                | downloads::CleanupRefusal::Cancelled
                | downloads::CleanupRefusal::Panicked,
            ) => {
                self.model.downloads.record_cleanup_debt(id, attempt);
            }
            Err(downloads::CleanupRefusal::Shutdown) => {
                self.model.downloads.record_cleanup_debt(id, attempt);
            }
        }
        self.publish_download(id);
        if self.close_pending.is_some()
            && self.model.downloads.cleanup_idle()
            && !self.model.downloads.has_cleanup_debt()
        {
            return self.update(Msg::CleanupDrainComplete);
        }
        Task::none()
    }

    pub(crate) fn reduce_cleanup_retry_tick(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CleanupRetryTick = message else {
            unreachable!("misrouted message in reduce_cleanup_retry_tick");
        };
        if let Some(handle) = &self.shutdown_helper {
            if handle.is_finished() {
                let Some(handle) = self.shutdown_helper.take() else {
                    return Task::none();
                };
                let outcome = match handle.join() {
                    Ok(o) => o,
                    Err(_) => downloads::ShutdownOutcome::HelperPanicked,
                };
                return self.update(Msg::ShutdownHelperDone(outcome));
            }
            return Task::none();
        }
        if self.close_pending.is_some()
            && let Some(deadline) = self.close_deadline
            && std::time::Instant::now() >= deadline
        {
            return Task::none();
        }
        if self.close_pending.is_some()
            && self.extension_cancel.is_none()
            && !self.model.downloads.has_cleanup_debt()
            && self.model.downloads.cleanup_idle()
            && self.model.downloads.cleanup_owner_state == downloads::CleanupOwnerState::Ready
        {
            return self.update(Msg::CleanupDrainComplete);
        }
        let tasks = self.model.downloads.retry_cleanup_debts();
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }
}
