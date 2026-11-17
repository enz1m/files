// Features, activity, unmount, pins, and portal reducer arms.

use super::*;

impl App {
    pub(crate) fn aux_toggle_feature(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleFeature(id) = message else {
            unreachable!("misrouted message in aux_toggle_feature");
        };
        // While refusal or debt exists, toggles are inert.
        if self.model.features_refusal.is_some() || self.model.features_durability_debt {
            return Task::none();
        }
        let prior = self.model.features.clone();
        let _ = self.model.features.toggle(id);
        if self.features_persistence.is_write_blocked() {
            self.model.features = prior;
            self.model.features_refusal = Some(features::FeaturesRefusal::CustodyLost);
            return Task::none();
        }
        match features::save_bounded(&self.model.features, &mut self.features_persistence) {
            Ok(_) => {}
            Err(features::FeaturesPublishOutcome::Unchanged) => {
                self.model.features = prior;
                self.model.features_refusal = Some(features::FeaturesRefusal::ReadFailed);
                return Task::none();
            }
            Err(features::FeaturesPublishOutcome::Rotated) => {
                self.model.features_durability_debt = true;
            }
            Err(features::FeaturesPublishOutcome::Lost) => {
                self.model.features = prior;
                self.model.features_refusal = Some(features::FeaturesRefusal::CustodyLost);
                return Task::none();
            }
            Err(features::FeaturesPublishOutcome::Unavailable) => {
                self.model.features = prior;
                self.model.features_refusal = Some(features::FeaturesRefusal::AuthorityUnavailable);
                return Task::none();
            }
            #[cfg(test)]
            Err(features::FeaturesPublishOutcome::Disarmed) => {}
        }
        self.reconcile_feature_changes(&prior)
    }

    pub(crate) fn aux_retry_feature_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetryFeatureSettings = message else {
            unreachable!("misrouted message in aux_retry_feature_settings");
        };
        if self.model.features_durability_debt && self.model.features_refusal.is_none() {
            match features::save_bounded(&self.model.features, &mut self.features_persistence) {
                Ok(_) => {
                    self.model.features_durability_debt = false;
                }
                Err(features::FeaturesPublishOutcome::Unchanged) => {}
                Err(features::FeaturesPublishOutcome::Rotated) => {}
                Err(features::FeaturesPublishOutcome::Lost) => {
                    self.model.features_durability_debt = false;
                    self.model.features_refusal = Some(features::FeaturesRefusal::CustodyLost);
                }
                Err(features::FeaturesPublishOutcome::Unavailable) => {
                    self.model.features_durability_debt = false;
                    self.model.features_refusal =
                        Some(features::FeaturesRefusal::AuthorityUnavailable);
                }
                #[cfg(test)]
                Err(features::FeaturesPublishOutcome::Disarmed) => {
                    self.model.features_durability_debt = false;
                }
            }
            return Task::none();
        }
        if self.model.features_refusal.is_some() {
            self.features_persistence.try_reacquire_for_recovery();
            if !self.features_persistence.is_write_blocked() {
                let prior = self.model.features.clone();
                let (loaded, outcome) = features::load_bounded(&self.features_persistence);
                match outcome {
                    features::FeaturesLoadOutcome::Applied
                    | features::FeaturesLoadOutcome::Absent => {
                        self.model.features = loaded;
                        self.model.features_refusal = None;
                        self.model.features_durability_debt = false;
                        return self.reconcile_feature_changes(&prior);
                    }
                    _ => {}
                }
            }
        }
        Task::none()
    }

    pub(crate) fn aux_replace_feature_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ReplaceFeatureSettings = message else {
            unreachable!("misrouted message in aux_replace_feature_settings");
        };
        if self.model.features_refusal.is_some() || self.features_persistence.is_write_blocked() {
            self.features_persistence.try_reacquire_for_recovery();
            if self.features_persistence.is_write_blocked() {
                return Task::none();
            }
        }
        let prior = self.model.features.clone();
        match features::publish_defaults(&mut self.features_persistence) {
            Ok(_) => {
                self.model.features = features::Features::default();
                self.model.features_refusal = None;
                self.model.features_durability_debt = false;
                return self.reconcile_feature_changes(&prior);
            }
            Err(features::FeaturesPublishOutcome::Rotated) => {
                self.model.features = features::Features::default();
                self.model.features_refusal = None;
                self.model.features_durability_debt = true;
                return self.reconcile_feature_changes(&prior);
            }
            Err(features::FeaturesPublishOutcome::Unchanged) => {}
            Err(features::FeaturesPublishOutcome::Lost) => {
                self.model.features_refusal = Some(features::FeaturesRefusal::CustodyLost);
                self.model.features_durability_debt = false;
            }
            Err(features::FeaturesPublishOutcome::Unavailable) => {}
            #[cfg(test)]
            Err(features::FeaturesPublishOutcome::Disarmed) => {
                self.model.features = features::Features::default();
                self.model.features_refusal = None;
                self.model.features_durability_debt = false;
                return self.reconcile_feature_changes(&prior);
            }
        }
        Task::none()
    }

    pub(crate) fn aux_torrent_download_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentDownloadLimitChanged(..) = message else {
            unreachable!("misrouted message in aux_torrent_download_limit_changed");
        };
        self.reduce_torrent_download_limit_changed(message)
    }

    pub(crate) fn aux_torrent_upload_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentUploadLimitChanged(..) = message else {
            unreachable!("misrouted message in aux_torrent_upload_limit_changed");
        };
        self.reduce_torrent_upload_limit_changed(message)
    }

    pub(crate) fn aux_download_concurrency_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadConcurrencyChanged(..) = message else {
            unreachable!("misrouted message in aux_download_concurrency_changed");
        };
        self.reduce_download_concurrency_changed(message)
    }

    pub(crate) fn aux_apply_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ApplyDownloadSettings = message else {
            unreachable!("misrouted message in aux_apply_download_settings");
        };
        self.reduce_apply_download_settings(message)
    }

    pub(crate) fn aux_retry_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetryDownloadSettings = message else {
            unreachable!("misrouted message in aux_retry_download_settings");
        };
        self.reduce_retry_download_settings(message)
    }

    pub(crate) fn aux_retire_download_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetireDownloadSettings = message else {
            unreachable!("misrouted message in aux_retire_download_settings");
        };
        self.reduce_retire_download_settings(message)
    }

    pub(crate) fn aux_aria2_probed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Aria2Probed(..) = message else {
            unreachable!("misrouted message in aux_aria2_probed");
        };
        self.reduce_aria2_probed(message)
    }

    pub(crate) fn aux_show_add_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowAddDownload = message else {
            unreachable!("misrouted message in aux_show_add_download");
        };
        self.reduce_show_add_download(message)
    }

    pub(crate) fn aux_close_add_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseAddDownload = message else {
            unreachable!("misrouted message in aux_close_add_download");
        };
        self.reduce_close_add_download(message)
    }

    pub(crate) fn aux_add_download_url_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AddDownloadUrlChanged(..) = message else {
            unreachable!("misrouted message in aux_add_download_url_changed");
        };
        self.reduce_add_download_url_changed(message)
    }

    pub(crate) fn aux_toggle_download_advanced(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleDownloadAdvanced = message else {
            unreachable!("misrouted message in aux_toggle_download_advanced");
        };
        self.reduce_toggle_download_advanced(message)
    }

    pub(crate) fn aux_add_download_limit_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AddDownloadLimitChanged(..) = message else {
            unreachable!("misrouted message in aux_add_download_limit_changed");
        };
        self.reduce_add_download_limit_changed(message)
    }

    pub(crate) fn aux_enqueue_download(&mut self, message: Msg) -> Task<Msg> {
        let Msg::EnqueueDownload = message else {
            unreachable!("misrouted message in aux_enqueue_download");
        };
        self.reduce_enqueue_download(message)
    }

    pub(crate) fn aux_enqueue_torrent(&mut self, message: Msg) -> Task<Msg> {
        let Msg::EnqueueTorrent(..) = message else {
            unreachable!("misrouted message in aux_enqueue_torrent");
        };
        self.reduce_enqueue_torrent(message)
    }

    pub(crate) fn aux_download_action(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadAction(..) = message else {
            unreachable!("misrouted message in aux_download_action");
        };
        self.reduce_download_action(message)
    }

    pub(crate) fn aux_torrent_telemetry(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TorrentTelemetry(..) = message else {
            unreachable!("misrouted message in aux_torrent_telemetry");
        };
        self.reduce_torrent_telemetry(message)
    }

    pub(crate) fn aux_transfer_event(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TransferEvent(..) = message else {
            unreachable!("misrouted message in aux_transfer_event");
        };
        self.reduce_transfer_event(message)
    }

    pub(crate) fn aux_transfer_outcome(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TransferOutcome(..) = message else {
            unreachable!("misrouted message in aux_transfer_outcome");
        };
        self.reduce_transfer_outcome(message)
    }

    pub(crate) fn aux_publication_reconciled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PublicationReconciled(..) = message else {
            unreachable!("misrouted message in aux_publication_reconciled");
        };
        self.reduce_publication_reconciled(message)
    }

    pub(crate) fn aux_paused_download_cleaned(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PausedDownloadCleaned(..) = message else {
            unreachable!("misrouted message in aux_paused_download_cleaned");
        };
        self.reduce_paused_download_cleaned(message)
    }

    pub(crate) fn aux_cleanup_retry_tick(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CleanupRetryTick = message else {
            unreachable!("misrouted message in aux_cleanup_retry_tick");
        };
        self.reduce_cleanup_retry_tick(message)
    }

    pub(crate) fn aux_activity_control(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ActivityControl(key, control) = message else {
            unreachable!("misrouted message in aux_activity_control");
        };
        if self
            .extension_activity
            .is_some_and(|activity| activity.key == key)
            && control == ActivityControl::Cancel
        {
            return self.reduce_extensions(Msg::CancelExtensionAction);
        }
        // The archive worker removes its staging, and settlement finishes the row.
        if key.provider == crate::activity::FILES
            && control == ActivityControl::Cancel
            && self.archive_work.cancel(key)
        {
            return Task::none();
        }
        if key.provider == enzui_core::operation_activity::downloads::DOWNLOADS {
            if control == ActivityControl::OpenPanel {
                return self.update(Msg::NavigateScreen(
                    Screen::Downloads,
                    PathBuf::from(services::DOWNLOADS_VIEW_ROOT),
                ));
            }
            if let Some(action) = enzui_core::operation_activity::downloads::action_of(control) {
                return self.update(Msg::DownloadAction(
                    enzui_core::download::DownloadId(key.op),
                    action,
                ));
            }
        }
        Task::none()
    }

    pub(crate) fn aux_show_activity_overflow(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowActivityOverflow(target) = message else {
            unreachable!("misrouted message in aux_show_activity_overflow");
        };
        use enzui_core::operation_activity::ActivityOverflowTarget;
        match target {
            ActivityOverflowTarget::Provider(provider)
                if provider == enzui_core::operation_activity::downloads::DOWNLOADS =>
            {
                self.update(Msg::NavigateScreen(
                    Screen::Downloads,
                    PathBuf::from(services::DOWNLOADS_VIEW_ROOT),
                ))
            }
            ActivityOverflowTarget::Provider(_) | ActivityOverflowTarget::AllActivity { .. } => {
                self.model.all_activity_open = true;
                Task::none()
            }
        }
    }

    pub(crate) fn aux_close_all_activity(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseAllActivity = message else {
            unreachable!("misrouted message in aux_close_all_activity");
        };
        self.model.all_activity_open = false;
        Task::none()
    }

    pub(crate) fn aux_activity_prune(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ActivityPrune = message else {
            unreachable!("misrouted message in aux_activity_prune");
        };
        self.activity_schedule.fired();
        let now = self.activity_now_ms();
        let _ = self.activity_center.prune(now);
        Task::none()
    }

    pub(crate) fn aux_unmount_drive(&mut self, message: Msg) -> Task<Msg> {
        let Msg::UnmountDrive(mount) = message else {
            unreachable!("misrouted message in aux_unmount_drive");
        };
        // Off-thread, since the unmount helper may block until its timeout. The
        // eject is an indeterminate activity row without controls; its key and
        // generation ride the reply so a retired attempt's late reply is refused.
        let activity = self.start_file_activity(
            ActivityKind::Unmount,
            crate::i18n::format(
                "files-ejecting-b3efa84b",
                &[("arg0", (mount.to_string_lossy()).to_string())],
            ),
            1,
        );
        let (generation, cancel) = self.begin_unmount();
        let service = self.unmount_service.clone();
        let task = Task::perform(unmount_drive(service, mount, cancel), move |result| {
            Msg::DriveUnmounted {
                generation,
                activity,
                result,
            }
        });
        self.own_unmount(generation, task)
    }

    pub(crate) fn aux_drive_unmounted(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DriveUnmounted {
            generation,
            activity,
            result,
        } = message
        else {
            unreachable!("misrouted message in aux_drive_unmounted");
        };
        if !self.finish_unmount(generation) {
            return Task::none();
        }
        match result {
            Ok(mount) => {
                let detail = crate::i18n::format(
                    "files-unmounted-366ff84b",
                    &[("arg0", (mount.to_string_lossy()).to_string())],
                );
                self.finish_file_activity(activity, 1, TerminalOutcome::Completed, detail.clone());
                self.post_status(detail, ToastTone::Success);
                // The drive list is stale.
                return Task::perform(load_drives(), Msg::DrivesLoaded);
            }
            Err(e) => {
                let detail =
                    crate::i18n::format("files-could-not-unmount-e-9b93cd07", &[("e", (e))]);
                self.finish_file_activity(activity, 0, TerminalOutcome::Failed, detail.clone());
                self.post_status(detail, ToastTone::Error);
            }
        }
        Task::none()
    }

    pub(crate) fn aux_pin_drag_hover(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PinDragHover(path) = message else {
            unreachable!("misrouted message in aux_pin_drag_hover");
        };
        self.model.pin_hover_begin(path);
        Task::none()
    }

    pub(crate) fn aux_pin_drag_leave(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PinDragLeave = message else {
            unreachable!("misrouted message in aux_pin_drag_leave");
        };
        self.model.pin_hover_end();
        Task::none()
    }

    pub(crate) fn aux_pin_drop(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PinDrop(path) = message else {
            unreachable!("misrouted message in aux_pin_drop");
        };
        if self.model.pins_refused.is_some() {
            self.post_status(
                crate::i18n::tr("Bookmarks file is refused — pin blocked").to_owned(),
                ToastTone::Error,
            );
            return Task::none();
        }
        match self.model.pin_path(path.clone()) {
            Ok(true) => match self.save_bookmarks() {
                SaveOutcome::Unchanged => {
                    self.model.unpin_path(&path);
                    self.model.refresh_pin_counters();
                    self.post_status(
                        crate::i18n::tr("Could not save pin").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Lost => {
                    self.post_status(
                        crate::i18n::tr("Bookmarks custody lost").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Rotated => {
                    self.post_status(
                        crate::i18n::format(
                            "files-pinned-durability-uncertain-91697b0e",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Warning,
                    );
                }
                SaveOutcome::Published | SaveOutcome::Disarmed => {
                    self.post_status(
                        crate::i18n::format(
                            "files-pinned-94df0efa",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Success,
                    );
                }
                SaveOutcome::Blocked => {}
            },
            Err(refusal) => {
                self.post_status(
                    crate::i18n::format(
                        "files-could-not-pin-refusal-75edd692",
                        &[("refusal", (refusal).to_string())],
                    ),
                    ToastTone::Error,
                );
            }
            Ok(false) => {}
        }
        Task::none()
    }

    pub(crate) fn aux_reorder_sidebar_pins(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ReorderSidebarPins(request) = message else {
            unreachable!("misrouted message in aux_reorder_sidebar_pins");
        };
        if self.model.pins_refused.is_some() {
            return Task::none();
        }
        let snapshot = self.model.bookmarks.clone();
        let snapshot_rows = self.model.pin_retained_rows;
        let snapshot_bytes = self.model.pin_retained_bytes;
        if self.model.reorder_sidebar_pins(&request) {
            match self.save_bookmarks() {
                SaveOutcome::Unchanged => {
                    self.model.bookmarks = snapshot;
                    self.model.pin_retained_rows = snapshot_rows;
                    self.model.pin_retained_bytes = snapshot_bytes;
                    self.post_status(
                        crate::i18n::tr("Could not save reorder").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Lost => {
                    self.post_status(
                        crate::i18n::tr("Bookmarks custody lost").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Rotated => {
                    self.post_status(
                        crate::i18n::tr("Reorder saved — durability uncertain").to_owned(),
                        ToastTone::Warning,
                    );
                }
                SaveOutcome::Published | SaveOutcome::Disarmed => {}
                SaveOutcome::Blocked => {}
            }
        }
        Task::none()
    }

    pub(crate) fn aux_pin_to_sidebar(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PinToSidebar(path) = message else {
            unreachable!("misrouted message in aux_pin_to_sidebar");
        };
        if self.model.pins_refused.is_some() {
            self.post_status(
                crate::i18n::tr("Bookmarks file is refused — pin blocked").to_owned(),
                ToastTone::Error,
            );
            return Task::none();
        }
        match self.model.pin_path(path.clone()) {
            Ok(true) => match self.save_bookmarks() {
                SaveOutcome::Unchanged => {
                    self.model.unpin_path(&path);
                    self.model.refresh_pin_counters();
                    self.post_status(
                        crate::i18n::tr("Could not save pin").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Lost => {
                    self.post_status(
                        crate::i18n::tr("Bookmarks custody lost").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Rotated => {
                    self.post_status(
                        crate::i18n::format(
                            "files-pinned-durability-uncertain-91697b0e",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Warning,
                    );
                }
                SaveOutcome::Published | SaveOutcome::Disarmed => {
                    self.post_status(
                        crate::i18n::format(
                            "files-pinned-94df0efa",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Success,
                    );
                }
                SaveOutcome::Blocked => {}
            },
            Err(refusal) => {
                self.post_status(
                    crate::i18n::format(
                        "files-could-not-pin-refusal-75edd692",
                        &[("refusal", (refusal).to_string())],
                    ),
                    ToastTone::Error,
                );
            }
            Ok(false) => {}
        }
        Task::none()
    }

    pub(crate) fn aux_unpin_from_sidebar(&mut self, message: Msg) -> Task<Msg> {
        let Msg::UnpinFromSidebar(path) = message else {
            unreachable!("misrouted message in aux_unpin_from_sidebar");
        };
        if self.model.pins_refused.is_some() {
            self.post_status(
                crate::i18n::tr("Bookmarks file is refused — unpin blocked").to_owned(),
                ToastTone::Error,
            );
            return Task::none();
        }
        let snapshot = self.model.bookmarks.clone();
        let snapshot_rows = self.model.pin_retained_rows;
        let snapshot_bytes = self.model.pin_retained_bytes;
        if self.model.unpin_path(&path) {
            self.model.refresh_pin_counters();
            match self.save_bookmarks() {
                SaveOutcome::Unchanged => {
                    self.model.bookmarks = snapshot;
                    self.model.pin_retained_rows = snapshot_rows;
                    self.model.pin_retained_bytes = snapshot_bytes;
                    self.post_status(
                        crate::i18n::tr("Could not save unpin").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Lost => {
                    self.post_status(
                        crate::i18n::tr("Bookmarks custody lost").to_owned(),
                        ToastTone::Error,
                    );
                }
                SaveOutcome::Rotated => {
                    self.post_status(
                        crate::i18n::format(
                            "files-unpinned-durability-uncertain-248b2cce",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Warning,
                    );
                }
                SaveOutcome::Published | SaveOutcome::Disarmed => {
                    self.post_status(
                        crate::i18n::format(
                            "files-unpinned-df01fcc1",
                            &[("arg0", (path.to_string_lossy()).to_string())],
                        ),
                        ToastTone::Success,
                    );
                }
                SaveOutcome::Blocked => {}
            }
        }
        Task::none()
    }

    pub(crate) fn aux_retry_pins(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetryPins = message else {
            unreachable!("misrouted message in aux_retry_pins");
        };
        // Republish the current model through the retained authority.
        if self.model.pin_durability_debt && self.pin_authority.is_some() {
            match self.save_bookmarks() {
                SaveOutcome::Published => {
                    self.post_status(
                        crate::i18n::tr("Bookmarks republished successfully"),
                        ToastTone::Success,
                    );
                }
                _ => {
                    self.post_status(
                        crate::i18n::tr("Could not republish bookmarks"),
                        ToastTone::Error,
                    );
                }
            }
            return Task::none();
        }

        let path = self
            .pin_path
            .clone()
            .or_else(|| self.pin_authority.as_ref().map(|a| a.path().to_path_buf()));
        let Some(path) = path else {
            return Task::none();
        };
        match enzui_core::state_authority::StateAuthority::open_or_create_tightening(&path) {
            Ok(new_authority) => {
                let defaults = services::default_places(&self.model.home);
                let default_pins: Vec<_> = defaults
                    .into_iter()
                    .filter(|p| {
                        matches!(
                            p.kind,
                            crate::model::PlaceKind::Home | crate::model::PlaceKind::XdgDir
                        )
                    })
                    .collect();
                match services::load_sidebar_pins_from_authority(&new_authority, &default_pins) {
                    services::PinLoadOutcome::Admitted {
                        pins,
                        retained_rows,
                        retained_bytes,
                    } => {
                        self.model.bookmarks = pins;
                        self.model.pin_retained_rows = retained_rows;
                        self.model.pin_retained_bytes = retained_bytes;
                        self.model.pins_refused = None;
                        self.model.pin_durability_debt = false;
                        self.pin_authority = Some(new_authority);
                        self.post_status(
                            crate::i18n::tr("Bookmarks reloaded successfully"),
                            ToastTone::Success,
                        );
                    }
                    services::PinLoadOutcome::Refused(refusal) => {
                        self.model.pins_refused = Some(refusal);
                        if self.pin_authority.is_none() {
                            self.pin_authority = Some(new_authority);
                        }
                        self.post_status(
                            crate::i18n::tr("Bookmarks still refused"),
                            ToastTone::Error,
                        );
                    }
                }
            }
            Err(_) => {
                self.post_status(
                    crate::i18n::tr("Could not reacquire bookmarks"),
                    ToastTone::Error,
                );
            }
        }
        Task::none()
    }

    pub(crate) fn aux_retire_pins(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RetirePins = message else {
            unreachable!("misrouted message in aux_retire_pins");
        };
        use enzui_core::state_authority::PublishCustody;

        let default_outcome = services::default_pins(&self.model.home);
        let defaults = match default_outcome {
            services::PinLoadOutcome::Admitted { pins, .. } => pins,
            services::PinLoadOutcome::Refused(_) => {
                self.post_status(
                    crate::i18n::tr("Default pins refused by admission"),
                    ToastTone::Error,
                );
                return Task::none();
            }
        };

        let path = self
            .pin_path
            .clone()
            .or_else(|| self.pin_authority.as_ref().map(|a| a.path().to_path_buf()));
        let Some(path) = path else {
            #[cfg(test)]
            if self.pin_mode == PinPersistenceMode::TestDisarmed {
                self.model.bookmarks = defaults;
                self.model.pins_refused = None;
                self.model.pin_durability_debt = false;
                self.model.refresh_pin_counters();
            }
            return Task::none();
        };

        match enzui_core::state_authority::StateAuthority::open_or_create_tightening(&path) {
            Ok(mut new_authority) => {
                match services::publish_bookmarks(&mut new_authority, &defaults) {
                    Ok(()) => {
                        self.pin_authority = Some(new_authority);
                        self.model.bookmarks = defaults;
                        self.model.pins_refused = None;
                        self.model.pin_durability_debt = false;
                        self.model.refresh_pin_counters();
                        self.post_status(
                            crate::i18n::tr("Bookmarks replaced with defaults"),
                            ToastTone::Success,
                        );
                    }
                    Err(e) => match e.custody() {
                        None | Some(PublishCustody::Unchanged) => {
                            self.post_status(
                                crate::i18n::tr("Could not replace bookmarks"),
                                ToastTone::Error,
                            );
                        }
                        Some(PublishCustody::Rotated) => {
                            self.pin_authority = Some(new_authority);
                            self.model.bookmarks = defaults;
                            self.model.pins_refused = None;
                            self.model.pin_durability_debt = true;
                            self.model.refresh_pin_counters();
                            self.post_status(
                                crate::i18n::tr("Defaults written — durability uncertain"),
                                ToastTone::Warning,
                            );
                        }
                        Some(_) => {
                            self.model.pins_refused = Some(services::PinRefusal::CustodyLost);
                            self.post_status(
                                crate::i18n::tr("Bookmarks custody lost"),
                                ToastTone::Error,
                            );
                        }
                    },
                }
            }
            Err(_) => {
                self.post_status(
                    crate::i18n::tr("Could not reacquire bookmarks"),
                    ToastTone::Error,
                );
            }
        }
        Task::none()
    }

    pub(crate) fn aux_portal_tick(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PortalTick = message else {
            unreachable!("misrouted message in aux_portal_tick");
        };
        let folder_task = self
            .picker
            .pump()
            .map_or_else(Task::none, |(request, outcome)| {
                self.finish_picker_request(request, outcome)
            });
        let download_task = self.finish_download_destinations();
        let mut tasks = vec![folder_task, download_task];
        for completion in self.file_opener.pump() {
            if portal_open::outcome_should_fallback(&completion.outcome) {
                tasks.push(self.open_with_default_handler(completion.path));
            } else if let Some(text) =
                portal_open::failure_text(&completion.path, &Ok(completion.outcome))
            {
                self.post_status(text, ToastTone::Error);
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn aux_portal_outcome(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PortalOutcome(request, outcome) = message else {
            unreachable!("misrouted message in aux_portal_outcome");
        };
        self.finish_picker_request(request, outcome)
    }
}
