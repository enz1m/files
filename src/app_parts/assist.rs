// Window roles, activity, toasts, downloads, drops, and dialogs.

use super::*;

// How long typing rests before an Everywhere search walk starts.
const SEARCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

impl App {
    // Which of this app's roles the window `id` is showing, mapping the
    // process-local id to the launch-stable [`WindowRole`].
    pub(crate) fn role_of(&self, id: iced::window::Id) -> Option<WindowRole> {
        if self.main_window == Some(id) {
            return Some(WindowRole::Main);
        }
        self.previews.get(&id).map(preview::Preview::role)
    }

    // The bounds a restored window size is clamped against.
    //
    // Uses the main window's width, or a generous fallback before the first
    // resize, since clamping to zero would collapse every window.
    pub(crate) fn output_bounds(&self) -> (f32, f32) {
        if self.window_width > 0.0 {
            // Height is not tracked; this bound cannot shrink a restored window.
            (self.window_width.max(1024.0), 4320.0)
        } else {
            (7680.0, 4320.0)
        }
    }

    // Writes pending window geometry, if any, from the close arm.
    //
    // I/O failures are ignored: this runs on the path to `iced::exit()`, and a
    // read-only state directory must not block closing.
    pub(crate) fn flush_placement(&mut self) {
        let Some(path) = self.placement_path.clone() else {
            return;
        };
        if !self.placement.has_pending() {
            return;
        }
        let _ = self.placement.flush_to(&path);
    }

    pub(crate) fn title(&self, id: iced::window::Id) -> String {
        match self.previews.get(&id) {
            Some(p) => p.title(),
            None => crate::i18n::tr("Files").to_owned(),
        }
    }

    // The daemon's per-window theme, shared by every window.
    pub(crate) fn theme(&self, _id: iced::window::Id) -> iced::Theme {
        self.themes.iced_theme()
    }

    // Starts the recursive-search walk for the current query, rooted at `cwd`;
    // an empty query starts nothing.
    pub(crate) fn recursive_search_task(&mut self) -> Task<Msg> {
        // Bumped first, so clearing the query still invalidates an in-flight walk.
        let generation = self.model.begin_search();
        // A superseded walk stops: aborting drops its cancel probe.
        if let Some(previous) = self.search_walk.take() {
            previous.abort();
        }
        let query = self.model.search_query.clone();
        if query.is_empty() {
            self.model.search_results.clear();
            return Task::none();
        }
        let root = self.model.cwd.clone();
        let stamp_root = root.clone();
        let stamp_query = query.clone();
        // Typing settles before a walk starts, so a word is one walk, not one
        // per letter.
        let (task, handle) = Task::perform(
            async move {
                crate::shell_work::delay(SEARCH_SETTLE).await;
                recursive_search(root, query).await
            },
            move |hits| Msg::SearchResultsReady {
                generation,
                root: stamp_root.clone(),
                query: stamp_query,
                hits,
            },
        )
        .abortable();
        self.search_walk = Some(handle);
        task
    }

    pub(crate) fn reconcile_open_context_menu(&mut self) {
        let valid = self.model.context_menu.as_ref().is_none_or(|open| {
            context_menu::items_for_target(&self.model, &open.path, open.origin).is_some()
        });
        if !valid {
            self.model.context_menu = None;
        }
    }

    pub(crate) fn activity_now_ms(&self) -> u64 {
        self.activity_clock.elapsed().as_millis() as u64
    }

    // Posts a transient outcome to the [`ToastHost`] and mirrors it into
    // `model.status_message`, the only permitted writer.
    //
    // `Error` and `Warning` announce as `Role::Alert`, other tones as
    // `Role::Status`. Identical repeated messages coalesce into one row.
    pub(crate) fn post_status(&mut self, text: impl Into<String>, tone: ToastTone) {
        let text = self.note_status(text);
        self.toasts
            .push(ToastSpec::new(text.clone()).tone(tone).coalesce_key(text));
    }

    // Reports how a tracked operation ended. Completion and cancellation only
    // update the activity row; a failure also raises an error toast.
    pub(crate) fn post_outcome(&mut self, text: impl Into<String>, outcome: TerminalOutcome) {
        match outcome {
            TerminalOutcome::Failed => self.post_status(text, ToastTone::Error),
            TerminalOutcome::Completed | TerminalOutcome::Cancelled => {
                self.note_status(text);
            }
        }
    }

    // Raises the Downloads failure as a sticky error toast. The queue owns the
    // failure; a new one replaces the toast and a cleared one withdraws it.
    pub(crate) fn reconcile_transfer_failure_toast(&mut self) {
        let current = self.model.downloads.banner_error();
        if self
            .transfer_failure_toast
            .as_ref()
            .map(|(text, _)| text.as_str())
            == current
        {
            return;
        }
        if let Some((_, id)) = self.transfer_failure_toast.take() {
            self.toasts.dismiss(id);
        }
        if let Some(error) = current.map(str::to_owned) {
            let id = self.toasts.push(
                ToastSpec::new(crate::i18n::tr(crate::downloads::TRANSFER_FAILED_TITLE))
                    .description(error.clone())
                    .tone(ToastTone::Error)
                    .coalesce_key("files-transfer-failed"),
            );
            self.transfer_failure_toast = Some((error, id));
        }
    }

    fn note_status(&mut self, text: impl Into<String>) -> String {
        let text = text.into();
        self.model.status_message = Some(text.clone());
        text
    }

    pub(crate) fn start_file_activity(
        &mut self,
        kind: ActivityKind,
        label: impl Into<String>,
        items: usize,
    ) -> activity::FileActivity {
        let now = self.activity_now_ms();
        activity::start(
            &mut self.activity_center,
            &mut self.next_file_activity_id,
            kind,
            label,
            items,
            now,
        )
    }

    pub(crate) fn publish_file_activity_items(
        &mut self,
        token: activity::FileActivity,
        queue: &conflict::ConflictQueue,
    ) {
        use enzui_core::operation_activity::ActivityProgress;
        if let Some(row) = self.activity_center.get(token.key) {
            let total = match row.progress {
                ActivityProgress::Items { total, .. } => total,
                _ if queue.is_open() => {
                    Some((queue.resolved.len() + queue.remaining.len() + 1) as u64)
                }
                _ => return,
            };
            let done = queue.completed_items() as u64;
            let _ = self.activity_center.progress(
                token.key,
                token.generation,
                ActivityProgress::Items { done, total },
            );
        }
    }

    pub(crate) fn finish_file_activity(
        &mut self,
        token: activity::FileActivity,
        completed_items: usize,
        outcome: TerminalOutcome,
        detail: impl Into<String>,
    ) {
        self.native_drop.finish(
            token,
            completed_items,
            outcome == TerminalOutcome::Completed,
        );
        self.shell_activity.finish(token, outcome);
        let now = self.activity_now_ms();
        let _ = activity::finish(
            &mut self.activity_center,
            token,
            completed_items as u64,
            outcome,
            detail,
            now,
        );
    }

    // An operation with nothing to count, such as a mount or a format: its row
    // shows work under way rather than 0%.
    pub(crate) fn start_service_activity(
        &mut self,
        kind: ActivityKind,
        label: impl Into<String>,
    ) -> activity::FileActivity {
        let token = self.start_file_activity(kind, label, 1);
        let _ = self.activity_center.progress(
            token.key,
            token.generation,
            enzui_core::operation_activity::ActivityProgress::Indeterminate,
        );
        token
    }

    // Ends an activity that turned out to need the person (a password, a
    // choice) without leaving a "cancelled" row behind; the retry starts its own.
    pub(crate) fn drop_file_activity(&mut self, token: activity::FileActivity) {
        self.finish_file_activity(token, 0, TerminalOutcome::Cancelled, String::new());
        let _ = self.activity_center.dismiss(token.key);
    }

    pub(crate) fn set_file_activity_phase(
        &mut self,
        token: activity::FileActivity,
        phase: LivePhase,
    ) {
        self.shell_activity.phase(token, phase);
        let _ = self
            .activity_center
            .set_live_phase(token.key, token.generation, phase);
    }

    // Applies runtime consequences of a feature-state change from `prior`,
    // returning a reload task when the active screen becomes unavailable.
    pub(crate) fn reconcile_feature_changes(&mut self, prior: &features::Features) -> Task<Msg> {
        let mut needs_reload = false;
        for id in features::FeatureId::ALL {
            let was = prior.enabled(id);
            let now = self.model.features.enabled(id);
            if was == now {
                continue;
            }
            match id {
                features::FeatureId::Thumbnails => {
                    if !now {
                        for (_, handle) in self
                            .thumbnail_operations
                            .drain()
                            .collect::<std::collections::BTreeMap<_, _>>()
                        {
                            handle.abort();
                        }
                        self.thumbnail_queue.clear();
                        self.model.thumbnails.clear();
                        self.preview_cache.clear();
                    }
                }
                features::FeatureId::Downloads => {
                    if now {
                        self.activity_center.set_provider_enabled(
                            enzui_core::operation_activity::downloads::DOWNLOADS,
                            true,
                        );
                        self.publish_downloads();
                    } else {
                        self.model.downloads.cancel_all();
                        self.model.downloads.clear_all_torrent_telemetry();
                        self.model.downloads.add = None;
                        self.activity_center.set_provider_enabled(
                            enzui_core::operation_activity::downloads::DOWNLOADS,
                            false,
                        );
                        self.model.downloads_settings_open = false;
                        if self.model.screen == Screen::Downloads {
                            needs_reload = true;
                        }
                    }
                }
                features::FeatureId::Network => {
                    if !now {
                        if let Some(cancel) = self.network_cancel.take() {
                            cancel.cancel();
                        }
                        if let Some(handle) = self.network_operation.take() {
                            handle.abort();
                        }
                        self.model.network.pending = false;
                        self.model.network.pending_mount = false;
                        self.model.attach = None;
                        if self.model.screen == Screen::Network {
                            needs_reload = true;
                        }
                    }
                }
                features::FeatureId::Converters => {}
            }
        }
        if needs_reload {
            return self.nav_navigate_to(Msg::NavigateTo(self.model.home.clone()));
        }
        Task::none()
    }

    pub(crate) fn publish_downloads(&mut self) {
        let now = self.activity_now_ms();
        let downloads = &self.model.downloads;
        let _ = enzui_core::operation_activity::downloads::publish(
            &mut self.activity_center,
            downloads
                .downloads()
                .map(|download| (download, downloads.name_of(download.id))),
            now,
        );
    }

    pub(crate) fn publish_download(&mut self, id: enzui_core::download::DownloadId) {
        let now = self.activity_now_ms();
        let downloads = &self.model.downloads;
        if let Some(download) = downloads.queue.get(id) {
            let _ = enzui_core::operation_activity::downloads::publish_one(
                &mut self.activity_center,
                download,
                downloads.name_of(id),
                now,
            );
        }
    }

    // Starts every transfer the queue's concurrency cap just released.
    //
    // Call wherever a slot can open; `DownloadQueue::promote` decides order.
    pub(crate) fn start_promoted(&mut self) -> Task<Msg> {
        if !self.model.features.enabled(features::FeatureId::Downloads) {
            return Task::none();
        }
        let jobs = self.model.downloads.promote();
        // One provider reconcile, so every released slot shows at once.
        self.publish_downloads();
        if jobs.is_empty() {
            return Task::none();
        }
        Task::batch(jobs.into_iter().map(downloads::transfer_task))
    }

    // Validates a drag or keyboard drop and routes it through `BeginPaste`. A
    // refusal never touches disk; it only posts a status.
    //
    // The checks stat every item and the target, so they run on a worker: a
    // drop onto a slow network folder must not freeze the window.
    pub(crate) fn route_item_drop(&mut self, req: enzui_core::drag::ItemDropRequest) -> Task<Msg> {
        let context = drag_drop::DropContext::of(&self.model);
        Task::perform(
            crate::shell_work::run(move || drag_drop::validate_item_drop_in(&context, &req)),
            |validated| Msg::ItemDropValidated(Box::new(validated)),
        )
    }

    pub(crate) fn finish_item_drop(
        &mut self,
        validated: Result<drag_drop::PlannedDrop, drag_drop::DropRefusal>,
    ) -> Task<Msg> {
        match validated {
            Ok(plan) => self.update(Msg::BeginPaste {
                sources: plan.sources,
                dest_dir: plan.dest_dir,
                copy: plan.copy,
            }),
            Err(refusal) => {
                self.post_status(refusal.status().to_owned(), ToastTone::Error);
                Task::none()
            }
        }
    }

    // The keyboard "Move to…" / "Copy to…" invoker: opens the portal directory
    // chooser.
    //
    // The chosen folder returns through [`Msg::MoveSelectionToFolder`] or
    // [`Msg::CopySelectionToFolder`], the same validation as a pointer drop. An
    // empty selection or a missing portal reports a visible status.
    pub(crate) fn begin_pick_destination(&mut self, copy: bool) -> Task<Msg> {
        if drag_drop::selected_ids_in_order(&self.model).is_empty() {
            self.post_status(
                if copy {
                    crate::i18n::tr("Nothing selected to copy")
                } else {
                    crate::i18n::tr("Nothing selected to move")
                },
                ToastTone::Warning,
            );
            return Task::none();
        }
        let target = if copy {
            picker::PickerTarget::CopySelection
        } else {
            picker::PickerTarget::MoveSelection
        };
        self.picker.start(target, &self.model, self.main_window)
    }

    pub(crate) fn main_window_scope(&self) -> Task<Msg> {
        self.main_window
            .map_or_else(Task::none, iced::window::invalidate_only)
    }

    // The one `Msg::DialogEvent` presentation currently visible, in the same
    // precedence order as [`Self::view`]. The delete confirmation is excluded:
    // its typed `ConfirmDialogEvent` owns a separate focus lifecycle.
    pub(crate) fn active_generic_dialog(&self) -> Option<ActiveGenericDialog> {
        if self.model.conflicts.is_some() {
            Some(ActiveGenericDialog::Conflict)
        } else if self.model.pending_permanent_delete.is_some() {
            None
        } else if self.model.mount_question.is_some() {
            Some(ActiveGenericDialog::MountQuestion)
        } else if self.model.unlock.is_some() {
            Some(ActiveGenericDialog::Unlock)
        } else if self.model.location_dialog.is_some() {
            Some(ActiveGenericDialog::Location)
        } else if self.model.format.is_some() {
            Some(ActiveGenericDialog::Format)
        } else if self.model.cloud.is_some() {
            Some(ActiveGenericDialog::Cloud)
        } else if self.model.bulk_rename.is_some() {
            Some(ActiveGenericDialog::BulkRename)
        } else if self.model.pending_device_confirm.is_some() {
            Some(ActiveGenericDialog::DeviceConfirm)
        } else if self.model.properties.is_some() {
            Some(ActiveGenericDialog::Properties)
        } else if self.model.open_with.is_some() {
            Some(ActiveGenericDialog::OpenWith)
        } else if self.model.attach.is_some() {
            Some(ActiveGenericDialog::Attach)
        } else if self.model.downloads.add.is_some() {
            Some(ActiveGenericDialog::AddDownload)
        } else if self.model.view_settings_open {
            Some(ActiveGenericDialog::ViewSettings)
        } else if self.model.select_matching_open {
            Some(ActiveGenericDialog::SelectMatching)
        } else if self.model.shortcuts_open {
            Some(ActiveGenericDialog::Shortcuts)
        } else if self.model.history.is_some() {
            Some(ActiveGenericDialog::History)
        } else if self.model.folder_color_dialog.is_some() {
            Some(ActiveGenericDialog::FolderColor)
        } else if self.model.all_activity_open {
            Some(ActiveGenericDialog::AllActivity)
        } else {
            None
        }
    }

    // The control that caused a dialog's open edge: a file action's row or tile
    // id, or the `Action` id for page actions.
    pub(crate) fn generic_dialog_invoker(
        &self,
        message: &Msg,
    ) -> Option<iced::advanced::widget::Id> {
        let file_row = |path: &Path| match self.model.view_mode {
            model::ViewMode::List => context_menu::list_row_id(path),
            model::ViewMode::Grid => context_menu::grid_tile_id(path),
        };
        match message {
            Msg::ToggleViewSettings if !self.model.view_settings_open => {
                Some(view_settings_invoker_id())
            }
            Msg::ShowProperties(path) | Msg::ShowOpenWith(path) => Some(file_row(path)),
            Msg::BeginBulkRename | Msg::OpenSelectMatching => self
                .model
                .selection
                .active()
                .map(|path| file_row(path.as_path())),
            Msg::ShowAddDownload => Some(iced::advanced::widget::Id::new("files-app-add-download")),
            Msg::ShowAddCloud => Some(iced::advanced::widget::Id::new(
                view::network::ADD_CLOUD_ACTION_ID,
            )),
            Msg::ShowAttach => Some(match self.model.screen {
                Screen::Network => view::network::connect_invoker_id(),
                _ => iced::advanced::widget::Id::new("attach"),
            }),
            _ => None,
        }
    }

    // Fallback invoker for dialogs opened by an asynchronous completion, such as
    // conflicts.
    pub(crate) fn generic_dialog_fallback_invoker(
        &self,
        dialog: ActiveGenericDialog,
    ) -> Option<iced::advanced::widget::Id> {
        let active_row = || {
            self.model
                .selection
                .active()
                .map(|path| match self.model.view_mode {
                    model::ViewMode::List => context_menu::list_row_id(path),
                    model::ViewMode::Grid => context_menu::grid_tile_id(path),
                })
        };
        match dialog {
            ActiveGenericDialog::Conflict | ActiveGenericDialog::BulkRename => active_row(),
            ActiveGenericDialog::Properties => {
                self.model
                    .properties
                    .as_ref()
                    .map(|info| match self.model.view_mode {
                        model::ViewMode::List => context_menu::list_row_id(&info.path),
                        model::ViewMode::Grid => context_menu::grid_tile_id(&info.path),
                    })
            }
            ActiveGenericDialog::OpenWith => self
                .model
                .open_with
                .as_ref()
                .and_then(|state| state.target_path.as_deref())
                .map(|path| match self.model.view_mode {
                    model::ViewMode::List => context_menu::list_row_id(path),
                    model::ViewMode::Grid => context_menu::grid_tile_id(path),
                }),
            ActiveGenericDialog::Attach => Some(if self.model.screen == Screen::Network {
                view::network::connect_invoker_id()
            } else {
                iced::advanced::widget::Id::new("attach")
            }),
            ActiveGenericDialog::AddDownload => {
                Some(iced::advanced::widget::Id::new("files-app-add-download"))
            }
            // Opened by the activity surface's pinned `+N more` button.
            ActiveGenericDialog::AllActivity => Some(iced::advanced::widget::Id::new(
                view::shell::ACTIVITY_OVERFLOW_ID,
            )),
            ActiveGenericDialog::ViewSettings => Some(view_settings_invoker_id()),
            ActiveGenericDialog::SelectMatching => active_row(),
            ActiveGenericDialog::Shortcuts => None,
            ActiveGenericDialog::DeviceConfirm => None,
            // Opened by a mount in progress, not by a control.
            ActiveGenericDialog::MountQuestion => None,
            ActiveGenericDialog::Unlock
            | ActiveGenericDialog::Location
            | ActiveGenericDialog::Format => None,
            ActiveGenericDialog::Cloud => Some(iced::advanced::widget::Id::new(
                view::network::ADD_CLOUD_ACTION_ID,
            )),
            // Opened from the row's menu; focus returns to the row.
            ActiveGenericDialog::History => match &self.model.history {
                Some(crate::history::Dialog::Versions(d)) => Some(match self.model.view_mode {
                    model::ViewMode::List => context_menu::list_row_id(&d.path),
                    model::ViewMode::Grid => context_menu::grid_tile_id(&d.path),
                }),
                _ => None,
            },
            ActiveGenericDialog::FolderColor => {
                self.model
                    .folder_color_dialog
                    .as_deref()
                    .map(|path| match self.model.view_mode {
                        model::ViewMode::List => context_menu::list_row_id(path),
                        model::ViewMode::Grid => context_menu::grid_tile_id(path),
                    })
            }
        }
    }
}
