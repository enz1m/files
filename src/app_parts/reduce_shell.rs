// Sidebar, window, preview, and close reducer arms.

use super::*;

impl App {
    pub(crate) fn reduce_toggle_sidebar(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleSidebar = message else {
            unreachable!("misrouted message in reduce_toggle_sidebar");
        };
        let presentation = self.sidebar_presentation();
        self.model.nav.toggle_for_presentation(presentation);
        Task::none()
    }

    pub(crate) fn reduce_focus_next(&mut self, message: Msg) -> Task<Msg> {
        let Msg::FocusNext = message else {
            unreachable!("misrouted message in reduce_focus_next");
        };
        operate(traverse::<Msg>(Traversal::Next))
    }

    pub(crate) fn reduce_focus_previous(&mut self, message: Msg) -> Task<Msg> {
        let Msg::FocusPrevious = message else {
            unreachable!("misrouted message in reduce_focus_previous");
        };
        operate(traverse::<Msg>(Traversal::Previous))
    }

    pub(crate) fn reduce_clear_focus(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ClearFocus(_) = message else {
            unreachable!("misrouted message in reduce_clear_focus");
        };
        operate(iced::advanced::widget::operation::focusable::unfocus())
    }

    pub(crate) fn reduce_clear_focus_2(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ClearFocus(_) = message else {
            unreachable!("misrouted message in reduce_clear_focus_2");
        };
        Task::none()
    }

    pub(crate) fn reduce_window_resized(&mut self, message: Msg) -> Task<Msg> {
        let Msg::WindowResized(id, size) = message else {
            unreachable!("misrouted message in reduce_window_resized");
        };
        let width = size.width;
        let refresh_grid_after_layout = self.main_window == Some(id)
            && self.model.screen == Screen::Directory
            && self.model.view_mode == model::ViewMode::Grid;
        // `resize_events()` reports every window; a preview's width must not
        // refold the main window.
        if self.main_window == Some(id) {
            self.window_width = width;
            self.window_height = size.height;
        } else if let Some(p) = self.previews.get_mut(&id) {
            p.width = width;
        }

        // The tracker coalesces a drag's events into one eventual write.
        if let Some(role) = self.role_of(id) {
            self.placement
                .record_resize_for(&role, (size.width, size.height));
        }

        // The collapse decision belongs to `nav_split::resolve_for_width`;
        // `effective_overlay_open` gates a stale `overlay_open` on presentation.
        // Grid defers a turn so thumbnail scheduling sees this resize's allocation.
        if refresh_grid_after_layout {
            Task::done(Msg::GridLayoutSettled)
        } else {
            Task::none()
        }
    }

    pub(crate) fn reduce_grid_layout_settled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::GridLayoutSettled = message else {
            unreachable!("misrouted message in reduce_grid_layout_settled");
        };
        Task::none()
    }

    pub(crate) fn reduce_window(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Window(command) = message else {
            unreachable!("misrouted message in reduce_window");
        };
        // Closing chrome must settle the same workers and pending authority as a native close.
        if command == enzui_widgets::title_bar_gesture::WindowCommand::Close
            && let Some(id) = self.main_window
        {
            return self.reduce_close_requested(Msg::CloseRequested(id));
        }
        match self.main_window {
            Some(id) => command.into_task(id),
            None => Task::none(),
        }
    }

    pub(crate) fn reduce_main_window_opened(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MainWindowOpened(id) = message else {
            unreachable!("misrouted message in reduce_main_window_opened");
        };
        if self.close_pending.is_some() {
            return iced::window::close(id);
        }
        self.main_window = Some(id);
        // `opened_primary`: closing the main window closes its previews
        // (`PrimaryClosePolicy::CloseEverything`).
        self.ledger.opened_primary(id);
        // Reads the real first window size; a compositor may never send a resize.
        let tag_task = self
            .shell_activity
            .pending_tag()
            .map_or_else(Task::none, |tag| {
                let tag = tag.to_owned();
                iced::window::set_toplevel_tag(id, tag.clone())
                    .map(move |queued| Msg::ActivityWindowTagged(id, tag.clone(), queued))
            });
        let size = Task::batch([
            self.pending_desktop_launches(),
            iced::window::size(id).map(move |size| Msg::WindowResized(id, size)),
            tag_task,
        ]);
        // Startup dialog fixtures bypass the open message, so focus once the native
        // window exists.
        let initial_dialog_focus =
            if self.active_generic_dialog() == Some(ActiveGenericDialog::ViewSettings) {
                dialog::initial_focus_task()
            } else {
                Task::none()
            };

        // Capture-only seed: opens a preview at boot through the real
        // `Msg::OpenPreview` path. Unset in production.

        if let Ok(rel) = std::env::var("FILES_APP_PREVIEW")
            && !rel.is_empty()
        {
            let path = self.model.cwd.join(rel);
            return Task::batch([
                size,
                initial_dialog_focus,
                Task::done(Msg::OpenPreview(path)),
            ]);
        }
        Task::batch([size, initial_dialog_focus])
    }

    pub(crate) fn reduce_preview_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PreviewSelection(window) = message else {
            unreachable!("misrouted message in reduce_preview_selection");
        };
        match self.space_preview(window) {
            SpacePreview::Close => iced::window::close(window),
            SpacePreview::Open(path) => self.update(Msg::OpenPreview(path)),
            SpacePreview::Nothing => Task::none(),
        }
    }

    pub(crate) fn reduce_space_on_row(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SpaceOnRow(path) = message else {
            unreachable!("misrouted message in reduce_space_on_row");
        };
        let select = self.update(Msg::SelectOnly(path));
        let Some(main) = self.main_window else {
            return select;
        };
        Task::batch([select, self.update(Msg::PreviewSelection(main))])
    }

    // What Space does in `window`: close it when it is a preview, as Quick
    // Look does; otherwise preview the selected file when the context menu's
    // Preview would.
    pub(crate) fn space_preview(&self, window: iced::window::Id) -> SpacePreview {
        if self.previews.contains_key(&window) {
            return SpacePreview::Close;
        }
        let selection = &self.model.selection;
        let path = selection.active().cloned().or_else(|| {
            (selection.len() == 1)
                .then(|| selection.iter().next().cloned())
                .flatten()
        });
        let Some(path) = path else {
            return SpacePreview::Nothing;
        };
        let is_dir = self
            .model
            .projected_entries()
            .into_iter()
            .any(|entry| entry.path == path && matches!(entry.kind, model::EntryKind::Dir));
        if crate::context_menu::RowKind::OpenPreview.enabled(&self.model, &path, is_dir) {
            SpacePreview::Open(path)
        } else {
            SpacePreview::Nothing
        }
    }

    pub(crate) fn reduce_open_preview(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenPreview(path) = message else {
            unreachable!("misrouted message in reduce_open_preview");
        };
        // Looked up in the current listing, so a deleted or filtered entry cannot
        // open.
        let Some(entry) = self
            .model
            .projected_entries()
            .into_iter()
            .find(|e| e.path == path)
            .cloned()
        else {
            return Task::none();
        };
        // Also guarded here, so a synthesised message cannot preview a folder.
        if matches!(entry.kind, model::EntryKind::Dir) {
            return Task::none();
        }

        // Focus an existing preview of this file instead of opening a second.
        if let Some((&id, _)) = self.previews.iter().find(|(_, p)| p.path == path) {
            return iced::window::gain_focus(id);
        }

        // File access, which may stall on a network mount, never runs in the
        // reducer; `PreviewPrepared` opens the window afterwards.
        Task::perform(
            crate::shell_work::run(move || {
                preview::Preview {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    // Shared with the list cells.
                    type_label: entry.kind.label(&entry.mime),
                    size_label: services::format_size(&entry),
                    content: services::read_preview_content(&entry.path, &entry.mime),
                    width: preview::DEFAULT_PREVIEW_SIZE.0,
                }
            }),
            |item| Msg::PreviewPrepared(Box::new(item)),
        )
    }

    pub(crate) fn reduce_preview_prepared(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PreviewPrepared(item) = message else {
            unreachable!("misrouted message in reduce_preview_prepared");
        };
        // Two clicks can race the read; the later result focuses the first window.
        if let Some((&id, _)) = self
            .previews
            .iter()
            .find(|(_, preview)| preview.path == item.path)
        {
            return iced::window::gain_focus(id);
        }

        // Restores stored placement, clamped so it cannot open off-screen.
        let restored = self.placement.initial_for(
            &item.role(),
            enzui_core::window_placement::Placement {
                normal_size: preview::DEFAULT_PREVIEW_SIZE,
                maximized: false,
            },
            self.output_bounds(),
        );

        // The framework's secondary-window settings, not the main window's, which
        // suppress the compositor frame.
        let (_id, open) =
            iced::window::open(preview::preview_window_settings(restored.normal_size));
        open.map(move |id| Msg::PreviewWindowOpened(id, item.clone()))
    }

    pub(crate) fn reduce_preview_window_opened(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PreviewWindowOpened(id, item) = message else {
            unreachable!("misrouted message in reduce_preview_window_opened");
        };
        if self.close_pending.is_some() {
            return iced::window::close(id);
        }
        if let preview::PreviewContent::Media { player, .. } = &item.content {
            player.service.play();
        }
        self.previews.insert(id, *item);
        // `opened`: closing a preview does not end the app.
        self.ledger.opened(id);
        Task::none()
    }

    pub(crate) fn reduce_close_requested(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseRequested(id) = message else {
            unreachable!("misrouted message in reduce_close_requested");
        };
        if self.main_window == Some(id) {
            self.model.unlock = None;
            self.picker.cancel();
            self.pending_extract = None;
            self.native_drop.cancel();
            if let Some(cancel) = &self.extension_cancel {
                cancel.cancel(enzui_core::cancellation::CancelCause::Shutdown);
            }
            self.file_clipboard.cancel();
            self.model.clipboard_pending = false;
        }
        if self.main_window == Some(id) && self.close_pending.is_none() {
            #[cfg(feature = "center")]
            {
                self.shutdown_center();
                self.agent.release_application();
            }
            self.desktop_launch = None;
            self.pending_launches.clear();
            self.model.downloads.cancel_all();
            self.model.downloads.close_cleanup_admission();
            self.close_pending = Some(id);
            self.close_deadline =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(8));
            let mut debt_tasks = self.model.downloads.retry_publication_debts();
            debt_tasks.extend(self.model.downloads.retry_cleanup_debts());
            if self.extension_cancel.is_none()
                && !self.model.downloads.has_cleanup_debt()
                && self.model.downloads.cleanup_idle()
            {
                debt_tasks.push(self.update(Msg::CleanupDrainComplete));
            }
            debt_tasks.push(iced::window::close(id));
            return Task::batch(debt_tasks);
        }
        if self.main_window != Some(id) {
            return iced::window::close(id);
        }
        Task::none()
    }

    pub(crate) fn reduce_cleanup_drain_complete(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CleanupDrainComplete = message else {
            unreachable!("misrouted message in reduce_cleanup_drain_complete");
        };
        if self.close_pending.is_some()
            && self.shutdown_helper.is_none()
            && self.extension_cancel.is_none()
            && self.model.downloads.cleanup_owner_state == downloads::CleanupOwnerState::Ready
        {
            self.close_deadline = None;
            match self.model.downloads.spawn_shutdown_helper() {
                Ok(handle) => {
                    self.shutdown_helper = Some(handle);
                }
                Err(()) => {
                    self.model.downloads.cleanup_error =
                        Some(crate::i18n::tr("shutdown helper could not be started").into());
                    self.close_deadline =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
                }
            }
        }
        Task::none()
    }

    pub(crate) fn reduce_shutdown_helper_done(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShutdownHelperDone(outcome) = message else {
            unreachable!("misrouted message in reduce_shutdown_helper_done");
        };
        self.shutdown_helper = None;
        self.model.downloads.record_helper_outcome(&outcome);
        match outcome {
            downloads::ShutdownOutcome::Clean => {
                if let Some(id) = self.close_pending.take() {
                    self.close_deadline = None;
                    return if self.main_window == Some(id) {
                        iced::window::close(id)
                    } else if self.ledger.open_count() == 0 {
                        iced::exit()
                    } else {
                        Task::none()
                    };
                }
            }
            downloads::ShutdownOutcome::Panicked { count } => {
                self.close_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(4));
                self.model.downloads.cleanup_error = Some(crate::i18n::format(
                    "files-count-cleanup-worker-panicked-during-shutdown-a7c56373",
                    &[("count", (count).to_string())],
                ));
            }
            downloads::ShutdownOutcome::HelperPanicked => {
                // Worker custody unknown; permanently degraded.
                self.close_deadline = None;
                self.model.downloads.cleanup_error = Some(
                    crate::i18n::tr("cleanup shutdown helper panicked; worker state unknown")
                        .into(),
                );
            }
            downloads::ShutdownOutcome::Unconfirmed => {
                self.close_deadline = None;
                self.model.downloads.cleanup_error = Some(
                    crate::i18n::tr("cleanup shutdown did not confirm worker termination").into(),
                );
            }
        }
        Task::none()
    }

    pub(crate) fn reduce_window_closed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::WindowClosed(id) = message else {
            unreachable!("misrouted message in reduce_window_closed");
        };
        if self.main_window == Some(id) {
            self.model.downloads.cancel_all();
        }
        self.previews.remove(&id);
        if self.main_window == Some(id) {
            self.main_window = None;
        }

        let action = self.ledger.closed(id);

        // Flushed synchronously before `iced::exit()`, which tears down the runtime
        // and would drop a batched write.
        self.flush_placement();
        // Same for view settings changed in the closing frame; a no-op when
        // unchanged.
        if action.exits() {
            self.flush_view_settings_blocking();
        }

        if self.close_pending.is_some() {
            // Keep the runtime alive for owned retirement after the native windows close.
            return match action {
                enzui_core::window_lifecycle::LifecycleAction::CloseThenExit(ids) => {
                    Task::batch(ids.into_iter().map(iced::window::close))
                }
                _ => Task::none(),
            };
        }
        lifecycle_task(action)
    }
}

// What Space does; see [`App::space_preview`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SpacePreview {
    Close,
    Open(std::path::PathBuf),
    Nothing,
}

impl App {
    pub(crate) fn reduce_preview_content(&mut self, message: Msg) -> Task<Msg> {
        use crate::preview::PreviewContent;
        use enzui_services::media::{PlaybackState, SeekMode};
        use enzui_widgets::media_controls::MediaControlsMessage as Control;
        match message {
            Msg::PreviewMediaWake(id) => {
                if let Some(item) = self.previews.get_mut(&id)
                    && let PreviewContent::Media {
                        player,
                        status,
                        frame,
                    } = &mut item.content
                {
                    let update = player.service.poll();
                    if let Some(next) = update.status {
                        **status = next;
                    }
                    if let Some(next) = update.frame
                        && next.generation() == status.generation
                    {
                        let (w, h) = (next.width(), next.height());
                        *frame = Some(iced::widget::image::Handle::from_rgba(
                            w,
                            h,
                            next.into_pixels(),
                        ));
                    }
                }
            }
            Msg::PreviewMediaControl(id, control) => {
                if let Some(item) = self.previews.get(&id)
                    && let PreviewContent::Media { player, status, .. } = &item.content
                {
                    match control {
                        Control::PlayPause if status.playback == PlaybackState::Playing => {
                            player.service.pause();
                        }
                        Control::PlayPause => player.service.play(),
                        Control::Seek(at) => player.service.seek(at, SeekMode::Accurate),
                        Control::SetVolume(volume) => player.service.set_volume(volume),
                        Control::ToggleMute => player.service.set_mute(!status.muted),
                        _ => {}
                    }
                }
            }
            Msg::PreviewPdfPage(id, index) => {
                if let Some(item) = self.previews.get_mut(&id)
                    && let PreviewContent::Pdf {
                        page,
                        pending,
                        error,
                    } = &mut item.content
                    && pending.is_none()
                    && index < page.count
                    && index != page.index
                {
                    *pending = Some(index);
                    *error = None;
                    let path = item.path.clone();
                    return Task::perform(
                        crate::shell_work::run(move || crate::preview_pdf::render(&path, index)),
                        move |result| Msg::PreviewPdfLoaded(id, index, result),
                    );
                }
            }
            Msg::PreviewPdfLoaded(id, index, result) => {
                if let Some(item) = self.previews.get_mut(&id)
                    && let PreviewContent::Pdf {
                        page,
                        pending,
                        error,
                    } = &mut item.content
                    && *pending == Some(index)
                {
                    *pending = None;
                    match result {
                        Ok(next) => *page = next,
                        Err(reason) => *error = Some(reason),
                    }
                }
            }
            _ => unreachable!("misrouted preview content message"),
        }
        Task::none()
    }
}
