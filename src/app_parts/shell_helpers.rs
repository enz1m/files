// Rename plans, downloads, picks, presentation, and launch helpers.

use super::*;

impl App {
    // The rename operations `Msg::ApplyBulkRename` would dispatch, or `None` when
    // the batch is refused: no open dialog, a blocked or empty plan, or a batch
    // already in flight.
    //
    // Separate from the arm so a refusal is observable; `Task` has no useful
    // `Debug`.
    pub(crate) fn planned_rename_operations(&self) -> Option<Vec<(PathBuf, PathBuf)>> {
        let state = self.model.bulk_rename.as_ref()?;
        if !state.can_apply() || self.model.bulk_rename_busy {
            return None;
        }
        Some(state.plan().operations())
    }

    pub(crate) fn add_download_submitting(&self) -> Option<enzui_core::download::DownloadId> {
        self.model.downloads.add.as_ref().and_then(|a| a.submitting)
    }

    // Admits and starts downloads whose Save dialogs granted a destination.
    // Cancellation and denial create no row or file; refusals keep their own
    // explanations.
    pub(crate) fn finish_download_destinations(&mut self) -> Task<Msg> {
        let decisions = self.model.downloads.pump_destinations();
        let mut granted = false;
        for decision in decisions {
            match decision {
                downloads::DestinationDecision::Granted { id, admitted_hint } => {
                    granted = true;
                    self.publish_download(id);
                    if self.add_download_submitting() == Some(id) {
                        self.model.downloads.add = None;
                    }
                    // With existing debt or refusal, leave the hint unpublished;
                    // the download still starts.
                    if self.model.downloads.is_settings_refused()
                        || self.model.downloads.settings_durability_debt
                    {
                        continue;
                    }
                    // The prior hint is grant N-1's committed result.
                    let prior_hint = self.model.downloads.last_destination_uri.clone();
                    if let Some(ref uri) = admitted_hint {
                        self.model.downloads.last_destination_uri = Some(uri.clone());
                    }
                    match self
                        .model
                        .downloads
                        .try_publish(&mut self.downloads_settings)
                    {
                        Ok(()) => {}
                        Err(downloads::SettingsPublishOutcome::Unchanged) => {
                            self.model.downloads.last_destination_uri = prior_hint;
                            self.post_status(
                                crate::i18n::tr("settings could not be saved").to_owned(),
                                ToastTone::Error,
                            );
                        }
                        // Rotated freezes the intended model; later grants cannot
                        // change the hint until Retry.
                        Err(downloads::SettingsPublishOutcome::Rotated) => {
                            self.model.downloads.settings_durability_debt = true;
                        }
                        Err(downloads::SettingsPublishOutcome::Lost) => {
                            self.model.downloads.last_destination_uri = prior_hint;
                            self.model.downloads.settings_refusal =
                                Some(downloads::SettingsRefusal::ReadFailed);
                        }
                        Err(downloads::SettingsPublishOutcome::Unavailable) => {
                            self.model.downloads.last_destination_uri = prior_hint;
                            self.model.downloads.settings_refusal =
                                Some(downloads::SettingsRefusal::AuthorityUnavailable);
                        }
                        #[cfg(test)]
                        Err(downloads::SettingsPublishOutcome::Disarmed) => {}
                    }
                }
                downloads::DestinationDecision::Declined(id, outcome) => {
                    // A final refusal shows inline and keeps the dialog open; a
                    // plain cancellation closes it.
                    if self.add_download_submitting() == Some(id) {
                        match downloads::destination_outcome_text(&outcome) {
                            Some(text) => {
                                if let Some(add) = self.model.downloads.add.as_mut() {
                                    add.error = Some(text.to_owned());
                                    add.submitting = None;
                                }
                            }
                            None => self.model.downloads.add = None,
                        }
                    } else if let Some(text) = downloads::destination_outcome_text(&outcome) {
                        let tone = if outcome.is_error() {
                            ToastTone::Error
                        } else {
                            ToastTone::Warning
                        };
                        self.post_status(text, tone);
                    }
                }
                downloads::DestinationDecision::Refused(id, error) => {
                    if self.add_download_submitting() == Some(id) {
                        if let Some(add) = self.model.downloads.add.as_mut() {
                            add.error = Some(error);
                            add.submitting = None;
                        }
                    } else {
                        self.post_status(error, ToastTone::Error);
                    }
                }
            }
        }
        // Once the native picker opens, the app dialog steps aside.
        if let Some(id) = self.add_download_submitting()
            && self.model.downloads.destination_started(id)
        {
            self.model.downloads.add = None;
        }
        if granted {
            self.start_promoted()
        } else {
            Task::none()
        }
    }

    pub(crate) fn finish_picker_request(
        &mut self,
        request: picker::PickerRequest,
        outcome: enzui_widgets::file_chooser::PortalOutcome<
            enzui_widgets::file_chooser::FileResult,
        >,
    ) -> Task<Msg> {
        let Some((target, context_valid)) = self.picker.settle(request, &self.model) else {
            return Task::none();
        };
        if matches!(
            outcome,
            enzui_widgets::file_chooser::PortalOutcome::Completed(_)
        ) && !context_valid
        {
            self.post_status(crate::i18n::tr("The selection or location changed while choosing a destination. Nothing was moved or copied."), ToastTone::Warning);
            return Task::none();
        }
        if target == picker::PickerTarget::ExtractArchive
            && !matches!(
                outcome,
                enzui_widgets::file_chooser::PortalOutcome::Completed(_)
            )
        {
            self.pending_extract = None;
        }
        self.finish_pick(target, outcome)
    }

    pub(crate) fn finish_pick(
        &mut self,
        target: picker::PickerTarget,
        outcome: enzui_widgets::file_chooser::PortalOutcome<
            enzui_widgets::file_chooser::FileResult,
        >,
    ) -> Task<Msg> {
        use enzui_widgets::file_chooser::PortalOutcome;
        match &outcome {
            PortalOutcome::Completed(result) => match target {
                picker::PickerTarget::InstallExtension => match result.first_local_path() {
                    Some(path) => self.update(Msg::ReviewExtension(path)),
                    None => {
                        self.post_status(
                            crate::i18n::tr("Choose a local .enzfiles package"),
                            ToastTone::Warning,
                        );
                        Task::none()
                    }
                },
                // The pointer drop's validation, so the keyboard route cannot
                // perform a move the pointer path would refuse.
                target @ (picker::PickerTarget::MoveSelection
                | picker::PickerTarget::CopySelection) => {
                    let copy = matches!(target, picker::PickerTarget::CopySelection);
                    match result.first_local_path() {
                        Some(dir) => {
                            if copy {
                                self.update(Msg::CopySelectionToFolder(dir))
                            } else {
                                self.update(Msg::MoveSelectionToFolder(dir))
                            }
                        }
                        None => {
                            self.post_status(
                                crate::i18n::tr(
                                    "That is not a local folder — nothing to move or copy into",
                                ),
                                ToastTone::Warning,
                            );
                            Task::none()
                        }
                    }
                }
                picker::PickerTarget::LinkSelection => match result.first_local_path() {
                    Some(dest_dir) => self.update(Msg::LinkSources {
                        sources: self.model.selection.iter().cloned().collect(),
                        dest_dir,
                    }),
                    None => {
                        self.post_status(
                            crate::i18n::tr("Choose a local folder"),
                            ToastTone::Warning,
                        );
                        Task::none()
                    }
                },
                picker::PickerTarget::ExtractArchive => {
                    let Some(archive) = self.pending_extract.take() else {
                        return Task::none();
                    };
                    match result.first_local_path() {
                        Some(dir) => self.update(Msg::ExtractArchive {
                            archive,
                            dest_dir: Some(dir),
                        }),
                        None => {
                            self.post_status(
                                crate::i18n::tr(
                                    "That is not a local folder — nothing to extract into",
                                ),
                                ToastTone::Warning,
                            );
                            Task::none()
                        }
                    }
                }
            },
            // Cancellation is a decision: no message, selection intact.
            PortalOutcome::Cancelled(_) => Task::none(),
            other => {
                if target == picker::PickerTarget::InstallExtension {
                    self.post_status(crate::i18n::tr("Could not choose an extension package. Close Settings and double-click a .enzfiles file in Files to review it."), ToastTone::Warning);
                    return Task::none();
                }
                if let Some(text) = enzui_widgets::file_chooser::failure_text(other) {
                    self.post_status(text, ToastTone::Error);
                }
                Task::none()
            }
        }
    }

    pub(crate) fn sidebar_presentation(&self) -> enzui_widgets::nav_split::SidebarPresentation {
        view::shell_geometry(&self.model, self.themes.theme(), self.window_width).sidebar
    }

    pub(crate) fn after_screen_change(&mut self, screen: Screen) -> Task<Msg> {
        self.arm_dir_watch();
        // A pending Trash scan is stale; a Trash visit dispatches a newer one.
        self.trash_load_generation = self.trash_load_generation.wrapping_add(1);
        match screen {
            Screen::Directory => self.reload(),
            Screen::Computer => Task::perform(load_drives(), Msg::DrivesLoaded),
            // Re-probed on every visit, so installing aria2 needs no restart.
            Screen::Downloads => {
                if !self.model.features.enabled(features::FeatureId::Downloads) {
                    Task::none()
                } else {
                    self.model.downloads.backend = downloads::Aria2Status::Probing;
                    Task::perform(downloads::probe_aria2c(), Msg::Aria2Probed)
                }
            }
            Screen::Network => {
                if !self.model.features.enabled(features::FeatureId::Network) {
                    Task::none()
                } else if matches!(self.model.network.discovery, network::DiscoveryState::Idle) {
                    self.update_inner(Msg::DiscoverNetwork)
                } else {
                    Task::none()
                }
            }
            // Read off-thread on every visit, so newly trashed files appear.
            Screen::Trash => self.dispatch_trash_load(),
            // Not a sidebar destination; nothing loads.
            Screen::Recent => Task::none(),
        }
    }

    pub(crate) fn open_with_default_handler(&mut self, path: PathBuf) -> Task<Msg> {
        let launcher = std::sync::Arc::clone(&self.launcher);
        let spec = launch::LaunchSpec::default_handler();
        let (generation, cancel) = self.begin_launch();
        let task = Task::perform(
            crate::shell_work::run(move || launcher.run_command_cancellable(&spec, &path, &cancel)),
            move |result| Msg::OpenLaunched { generation, result },
        );
        self.own_launch(generation, task)
    }
}
