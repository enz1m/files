// View, sort, dialogs, properties, and bulk-rename reducer arms.

use super::*;

// How the current view stacks rows, for keeping a row in place
// across a zoom: row `i` starts `lead + (i / columns) * pitch` down.
struct ZoomGeometry {
    columns: usize,
    pitch: f32,
    lead: f32,
}

impl ZoomGeometry {
    fn top(&self, index: usize) -> f32 {
        self.lead + (index / self.columns.max(1)) as f32 * self.pitch
    }
}

impl App {
    // Steps the current view's zoom level, persists it, and keeps the anchor row
    // (the active row when visible, else the top row) at the same viewport height.
    pub(crate) fn view_zoom(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Zoom(step) = message else {
            unreachable!("misrouted message in view_zoom");
        };
        if self.model.screen != Screen::Directory {
            return Task::none();
        }
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        let before = self.zoom_geometry();
        let scroll = match self.model.view_mode {
            model::ViewMode::List => self.model.list_scroll_y,
            model::ViewMode::Grid => self.model.grid_scroll_y,
        };
        let viewport =
            (self.window_height - enzui_widgets::title_bar::DEFAULT_TITLEBAR_HEIGHT).max(120.0);
        let active = self.model.selection.active().and_then(|path| {
            self.model
                .projected_entries()
                .iter()
                .position(|entry| &entry.path == path)
        });
        let first_visible = (((scroll - before.lead).max(0.0) / before.pitch.max(1.0)) as usize)
            * before.columns.max(1);
        let anchor = active
            .filter(|&index| {
                let top = before.top(index);
                top + before.pitch > scroll && top < scroll + viewport
            })
            .unwrap_or(first_visible);
        let offset = before.top(anchor) - scroll;

        if !self.model.zoom(step) {
            return Task::none();
        }
        let after = self.zoom_geometry();
        let restored = (after.top(anchor) - offset).max(0.0);
        match self.model.view_mode {
            model::ViewMode::List => self.model.list_scroll_y = restored,
            model::ViewMode::Grid => self.model.grid_scroll_y = restored,
        }
        self.persist_view_settings()
    }

    // The current view's row geometry at the model's zoom level. For the grid it
    // also publishes the next tile stretch, so keyboard columns agree before the
    // next frame.
    fn zoom_geometry(&self) -> ZoomGeometry {
        let theme = self.themes.theme();
        let shell_width =
            view::shell_geometry(&self.model, self.themes.theme(), self.window_width).content_width;
        let content_width = view::shell::directory_view_width(&self.model, shell_width);
        match self.model.view_mode {
            model::ViewMode::List => ZoomGeometry {
                columns: 1,
                pitch: view::list::row_height_for(&self.model, theme, content_width),
                lead: 0.0,
            },
            model::ViewMode::Grid => {
                let gap = view::widgets::GRID_GAP;
                let tile_width = view::grid::grid_tile_width(&self.model, theme);
                let tile_height = view::grid::grid_tile_height(&self.model, theme);
                let resolved = enzui_core::virtual_grid::TileGrid::new(tile_width, tile_height)
                    .with_spacing(gap, gap)
                    .stretch_for_width((content_width - 2.0 * gap).max(tile_width));
                self.model.grid_layout.set(resolved);
                ZoomGeometry {
                    columns: resolved.columns.max(1),
                    pitch: tile_height + gap,
                    lead: gap,
                }
            }
        }
    }

    // Opens a terminal in `folder` or the current folder; virtual locations and
    // launch failures are reported.
    pub(crate) fn view_open_in_terminal(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenInTerminal(folder) = message else {
            unreachable!("misrouted message in view_open_in_terminal");
        };
        let folder = folder.unwrap_or_else(|| self.model.cwd.clone());
        // A missing folder is refused by the launch itself, off this thread.
        if crate::services::is_virtual(&folder) {
            self.post_status(
                crate::i18n::tr("Open in Terminal needs a folder on this computer"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        match crate::terminal::plan(&self.model.terminal_command) {
            Ok(plan) => Task::perform(
                crate::shell_work::run(move || crate::terminal::launch(&plan, &folder)),
                Msg::TerminalLaunched,
            ),
            Err(reason) => {
                self.post_status(reason, ToastTone::Error);
                Task::none()
            }
        }
    }

    pub(crate) fn view_terminal_launched(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TerminalLaunched(result) = message else {
            unreachable!("misrouted message in view_terminal_launched");
        };
        // Success is the terminal window itself; only a failure needs words.
        if let Err(reason) = result {
            self.post_status(reason, ToastTone::Error);
        }
        Task::none()
    }

    // The Settings field, kept within one persisted field line.
    pub(crate) fn view_set_terminal_command(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetTerminalCommand(command) = message else {
            unreachable!("misrouted message in view_set_terminal_command");
        };
        if !crate::view_settings_store::is_terminal_command(&command) {
            return Task::none();
        }
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.terminal_command = command;
        self.persist_view_settings()
    }

    pub(crate) fn view_set_view_mode(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetViewMode(mode) = message else {
            unreachable!("misrouted message in view_set_view_mode");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.view_mode = mode;
        self.model.default_view_mode = mode;
        self.persist_view_settings()
    }

    pub(crate) fn view_sort_by(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SortBy(key) = message else {
            unreachable!("misrouted message in view_sort_by");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.sort_by(key);
        // Sort state is visible in the controls; no toast.
        self.persist_view_settings()
    }

    pub(crate) fn view_set_sort_key(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetSortKey(key) = message else {
            unreachable!("misrouted message in view_set_sort_key");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.set_sort_key(key);
        self.persist_view_settings()
    }

    pub(crate) fn view_set_sort_ascending(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetSortAscending(ascending) = message else {
            unreachable!("misrouted message in view_set_sort_ascending");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.set_sort_ascending(ascending);
        // Preview windows show pinned files, so sorting invalidates only the browser.
        Task::batch([self.persist_view_settings(), self.main_window_scope()])
    }

    pub(crate) fn view_toggle_hidden(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleHidden = message else {
            unreachable!("misrouted message in view_toggle_hidden");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.show_hidden = !self.model.show_hidden;
        self.persist_view_settings()
    }

    pub(crate) fn view_toggle_folders_first(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleFoldersFirst = message else {
            unreachable!("misrouted message in view_toggle_folders_first");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.sort_folders_first = !self.model.sort_folders_first;
        self.persist_view_settings()
    }

    pub(crate) fn view_toggle_single_click(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleSingleClick = message else {
            unreachable!("misrouted message in view_toggle_single_click");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.single_click_open = !self.model.single_click_open;
        self.persist_view_settings()
    }

    pub(crate) fn view_toggle_colored_file_icons(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleColoredFileIcons = message else {
            unreachable!("misrouted message in view_toggle_colored_file_icons");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        self.model.colored_file_icons = !self.model.colored_file_icons;
        self.persist_view_settings()
    }

    pub(crate) fn view_toggle_column(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleColumn(column) = message else {
            unreachable!("misrouted message in view_toggle_column");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        let i = column.index();
        self.model.visible_columns[i] = !self.model.visible_columns[i];
        self.persist_view_settings()
    }

    pub(crate) fn view_toggle_grid_info(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleGridInfo(column) = message else {
            unreachable!("misrouted message in view_toggle_grid_info");
        };
        if let Some(task) = self.guard_view_settings_mutation() {
            return task;
        }
        let i = column.index();
        self.model.grid_info[i] = !self.model.grid_info[i];
        self.persist_view_settings()
    }

    pub(crate) fn view_view_settings_saved(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ViewSettingsSaved(result) = message else {
            unreachable!("misrouted message in view_view_settings_saved");
        };
        self.view_settings_save_error = result.err().map(|error| error.to_string());
        self.main_window_scope()
    }

    pub(crate) fn view_view_settings_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ViewSettingsLoaded(handoff) = message else {
            unreachable!("misrouted message in view_view_settings_loaded");
        };
        use enzui_core::persist::LoadSettlement;
        // Take-once: clones share this slot, so only the first observer settles.
        let Some(completion) = handoff.take() else {
            return Task::none();
        };
        let Some(controller) = self.view_settings_controller.as_mut() else {
            // No controller armed, or a prior completion retired it.
            return Task::none();
        };
        match controller.settle(completion) {
            LoadSettlement::Applied(outcome) => {
                let summary = outcome.user_summary();
                // A changed model token means the user edited during the load;
                // keep their state and rebaseline instead of applying.
                let current_token = view_settings_store::ViewSettings::baseline_token(&self.model);
                let mut moved = false;
                let applied_loaded = match &self.view_settings_load_baseline {
                    Some(baseline) if *baseline == current_token => {
                        self.model.apply_view_settings(outcome.value());
                        // Saved tabs restore only into the untouched boot tab, and
                        // not when this launch names its own folder.
                        if !self.startup_directory_requested
                            && self.model.tabs.len() == 1
                            && self.model.back.is_empty()
                            && self.model.forward.is_empty()
                        {
                            let before = self.model.cwd.clone();
                            self.model
                                .restore_session(&outcome.value().tabs, outcome.value().active_tab);
                            moved = self.model.cwd != before;
                        }
                        true
                    }
                    Some(_) => {
                        // Edited during the load: the loaded record with the
                        // edits kept on top. Tabs are left as the user has them.
                        if let Some(boot) = &self.view_settings_boot_snapshot {
                            let current = view_settings_store::ViewSettings::snapshot(&self.model);
                            let merged = view_settings_store::ViewSettings::keep_edits(
                                outcome.value(),
                                boot,
                                &current,
                            );
                            self.model.apply_view_settings(&merged);
                        }
                        false
                    }
                    None => {
                        // No baseline captured; fail closed.
                        false
                    }
                };
                // Rebaseline against whichever state now holds.
                let new_token = view_settings_store::ViewSettings::token(&self.model);
                self.view_settings_gate.rebaseline(new_token);
                // Production issues no second boot load.
                self.view_settings_controller = None;
                self.view_settings_load_baseline = None;
                self.view_settings_boot_snapshot = None;
                if applied_loaded && let Some(text) = summary {
                    self.post_status(text, ToastTone::Error);
                }
                // Superseded by user edits, not refused, so no toast.
                if moved {
                    let screen = self.model.screen.clone();
                    return self.after_screen_change(screen);
                }
            }
            LoadSettlement::WorkerFailed(err) => {
                self.view_settings_controller = None;
                self.view_settings_load_baseline = None;
                self.post_status(
                    crate::i18n::format(
                        "files-view-settings-load-worker-stopped-err-e3b40485",
                        &[("err", (err).to_string())],
                    ),
                    ToastTone::Error,
                );
            }
            LoadSettlement::DiscardedStale { .. } => {
                // A superseded worker returned; the current one keeps its slot.
            }
        }
        Task::none()
    }

    pub(crate) fn view_toggle_details(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleDetails = message else {
            unreachable!("misrouted message in view_toggle_details");
        };
        self.model.details_open = !self.model.details_open;
        Task::none()
    }

    pub(crate) fn view_toggle_view_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleViewSettings = message else {
            unreachable!("misrouted message in view_toggle_view_settings");
        };
        let was_open = self.model.view_settings_open;
        self.model.view_settings_open = !was_open;
        // Settings open on the page list in a narrow window.
        self.model.settings_page_open = false;
        self.close_settings_subpages();
        if was_open {
            // Closing without Apply discards plugin-setting edits.
            self.model.downloads.reset_settings_draft();
        }
        // The dialog stays hosted for the exit motion while
        // `presentation_open(false)` withdraws input, focus and AT.
        settings_surface::sync(self);
        // The dialog lifecycle post-pass moves focus in and restores the invoker.
        Task::none()
    }

    pub(crate) fn view_view_settings_exit_settled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ViewSettingsExitSettled(key) = message else {
            unreachable!("misrouted message in view_view_settings_exit_settled");
        };
        settings_surface::finish_exit(self, key);
        Task::none()
    }

    pub(crate) fn view_toggle_toolbar_overflow(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleToolbarOverflow = message else {
            unreachable!("misrouted message in view_toggle_toolbar_overflow");
        };
        self.model.toolbar_overflow_open = !self.model.toolbar_overflow_open;
        Task::none()
    }

    pub(crate) fn view_close_toolbar_overflow(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseToolbarOverflow = message else {
            unreachable!("misrouted message in view_close_toolbar_overflow");
        };
        // Idempotent close: the framework batches this with a folded row's command
        // and Escape/outside presses, so it sets rather than toggles.
        self.model.toolbar_overflow_open = false;
        Task::none()
    }

    // Leaves any extension or Downloads settings subpage and any shortcut
    // recording.
    fn close_settings_subpages(&mut self) {
        self.model.extensions.expanded = None;
        self.model.downloads_settings_open = false;
        self.model.shortcut_capture = None;
        self.model.shortcut_feedback = None;
    }

    pub(crate) fn view_select_settings_page(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectSettingsPage(id) = message else {
            unreachable!("misrouted message in view_select_settings_page");
        };
        // Unknown page ids are ignored.
        if let Some(section) = model::SettingsSection::from_id(&id) {
            self.model.settings_page = section;
            self.model.settings_page_open = true;
            // A page opens at its top level, not inside a subpage left open.
            self.close_settings_subpages();
        }
        Task::none()
    }

    pub(crate) fn view_show_properties(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowProperties(path) = message else {
            unreachable!("misrouted message in view_show_properties");
        };
        // The snapshot is cheap; a directory's recursive size is computed off-thread
        // and arrives as `PropertiesRecursiveSizeReady`.
        match services::build_properties(&path) {
            Ok(info) => {
                let needs_recursive = info.recursive_size_pending();
                // Permissions start folded: most visits want the size and
                // dates. An Apply re-read keeps whatever the user opened.
                self.model.properties_perms_expanded = false;
                self.model.open_properties(info);
                if needs_recursive {
                    let walk_path = path.clone();
                    Task::perform(recursive_size(walk_path.clone()), move |(total, count)| {
                        Msg::PropertiesRecursiveSizeReady {
                            path: walk_path,
                            total,
                            count,
                        }
                    })
                } else {
                    Task::none()
                }
            }
            Err(_) => Task::none(),
        }
    }

    pub(crate) fn view_toggle_permissions_section(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TogglePermissionsSection(open) = message else {
            unreachable!("misrouted message in view_toggle_permissions_section");
        };
        self.model.properties_perms_expanded = open;
        Task::none()
    }

    pub(crate) fn view_toggle_permission_bit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TogglePermissionBit { row, col } = message else {
            unreachable!("misrouted message in view_toggle_permission_bit");
        };
        self.model.toggle_permission_bit(row, col);
        Task::none()
    }

    pub(crate) fn view_apply_permissions(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ApplyPermissions = message else {
            unreachable!("misrouted message in view_apply_permissions");
        };
        // Re-read after writing, since a chmod can be refused or clamped.
        if let Some(info) = self.model.properties.clone() {
            let _ = services::apply_permissions(&info);
            if let Ok(reread) = services::build_properties(&info.path) {
                // Keep a computed recursive size so the re-read does not restart it.
                let prior = info.recursive_size_text.clone();
                self.model.open_properties(reread);
                if !prior.is_empty() {
                    self.model.set_recursive_size(&info.path, prior);
                }
            }
        }
        Task::none()
    }

    pub(crate) fn view_properties_recursive_size_ready(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PropertiesRecursiveSizeReady { path, total, count } = message else {
            unreachable!("misrouted message in view_properties_recursive_size_ready");
        };
        let text = crate::i18n::format(
            "files-items-417b3c8f",
            &[
                ("arg0", (services::format_bytes(total))),
                ("arg1", (count).to_string()),
            ],
        );
        self.model.set_recursive_size(&path, text);
        Task::none()
    }

    pub(crate) fn view_begin_bulk_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginBulkRename = message else {
            unreachable!("misrouted message in view_begin_bulk_rename");
        };
        let targets = self.model.bulk_rename_targets();
        if targets.is_empty() {
            // Nothing selected: never rename the whole folder by accident.
            return Task::none();
        }
        let occupied = services::occupied_siblings(&targets);
        let state = bulk_rename::BulkRename::new(targets, occupied);
        // One row renames in place; several keep the rule dialog.
        if state.is_single() && !crate::services::is_virtual(&self.model.cwd) {
            // The name is selected up to its extension, so typing replaces
            // the name and keeps the type.
            let is_dir = state.targets.first().is_some_and(|path| {
                self.model
                    .entries
                    .iter()
                    .any(|entry| entry.path == *path && entry.kind == model::EntryKind::Dir)
            });
            let end = name_stem_end(&state.direct, is_dir);
            self.model.inline_rename = Some(state);
            self.model.inline_rename_blurred = false;
            let id = crate::view::widgets::inline_rename_id();
            return iced::widget::operation::focus(id.clone()).chain(
                iced::widget::operation::select_range(
                    id,
                    iced::advanced::text::Position { line: 0, index: 0 },
                    iced::advanced::text::Position {
                        line: 0,
                        index: end,
                    },
                ),
            );
        }
        self.model.bulk_rename = Some(state);
        Task::none()
    }

    pub(crate) fn view_inline_rename_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::InlineRenameChanged(name) = message else {
            unreachable!("misrouted message in view_inline_rename_changed");
        };
        if let Some(state) = self.model.inline_rename.as_mut() {
            state.direct = name;
        }
        Task::none()
    }

    pub(crate) fn view_commit_inline_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CommitInlineRename = message else {
            unreachable!("misrouted message in view_commit_inline_rename");
        };
        let Some(state) = self.model.inline_rename.as_ref() else {
            return Task::none();
        };
        if self.model.bulk_rename_busy {
            return Task::none();
        }
        let plan = state.plan();
        if plan.rename_count() == 0 && !plan.is_blocked() {
            // Enter on an unchanged name ends the edit like Escape.
            return self.update(Msg::CancelInlineRename);
        }
        if !state.can_apply() {
            // Blocked: the field shows why and stays open for a fix.
            return Task::none();
        }
        let operations = plan.operations();
        self.model.bulk_rename_busy = true;
        let activity =
            self.start_file_activity(ActivityKind::Rename, crate::i18n::tr("Renaming 1 item"), 1);
        Task::perform(apply_renames(operations), move |outcome| {
            Msg::BulkRenameFinished { activity, outcome }
        })
    }

    // A click elsewhere settles the edit with the typed name, as Enter does,
    // without taking focus back from what was clicked. A blocked name keeps
    // the field open with its reason.
    pub(crate) fn view_inline_rename_blurred(&mut self, message: Msg) -> Task<Msg> {
        let Msg::InlineRenameBlurred = message else {
            unreachable!("misrouted message in view_inline_rename_blurred");
        };
        let Some(state) = self.model.inline_rename.as_ref() else {
            return Task::none();
        };
        if self.model.bulk_rename_busy {
            return Task::none();
        }
        let plan = state.plan();
        if plan.rename_count() == 0 && !plan.is_blocked() {
            self.model.inline_rename = None;
            return Task::none();
        }
        self.model.inline_rename_blurred = true;
        self.update(Msg::CommitInlineRename)
    }

    pub(crate) fn settle_inline_rename(&mut self) -> Task<Msg> {
        if self.model.inline_rename.is_none() {
            return Task::none();
        }
        self.view_inline_rename_blurred(Msg::InlineRenameBlurred)
    }

    pub(crate) fn view_cancel_inline_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelInlineRename = message else {
            unreachable!("misrouted message in view_cancel_inline_rename");
        };
        if self.model.bulk_rename_busy {
            return Task::none();
        }
        let Some(state) = self.model.inline_rename.take() else {
            return Task::none();
        };
        // Return focus to the row the edit replaced.
        match state.targets.first() {
            Some(path) => iced::widget::operation::focus(match self.model.view_mode {
                model::ViewMode::List => context_menu::list_row_id(path),
                model::ViewMode::Grid => context_menu::grid_tile_id(path),
            }),
            None => Task::none(),
        }
    }

    pub(crate) fn view_cancel_bulk_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelBulkRename = message else {
            unreachable!("misrouted message in view_cancel_bulk_rename");
        };
        self.model.close_bulk_rename();
        Task::none()
    }

    pub(crate) fn view_set_bulk_rename_mode(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetBulkRenameMode(mode) = message else {
            unreachable!("misrouted message in view_set_bulk_rename_mode");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.mode = mode;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_direct_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameDirectChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_direct_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            // Stored verbatim; `plan()` trims, so whitespace is blocked, not edited away.
            b.direct = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_find_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameFindChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_find_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.find = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_replace_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameReplaceChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_replace_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.replace = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_template_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameTemplateChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_template_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.template = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_start_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameStartChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_start_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            // Stored verbatim; `0` is honoured literally.
            b.start = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_step_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameStepChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_step_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.step = v;
        }
        Task::none()
    }

    pub(crate) fn view_bulk_rename_pad_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenamePadChanged(v) = message else {
            unreachable!("misrouted message in view_bulk_rename_pad_changed");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.pad = v.min(bulk_rename::MAX_PAD);
        }
        Task::none()
    }

    pub(crate) fn view_reset_bulk_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ResetBulkRename = message else {
            unreachable!("misrouted message in view_reset_bulk_rename");
        };
        if let Some(b) = self.model.bulk_rename.as_mut() {
            b.reset();
        }
        Task::none()
    }

    pub(crate) fn view_apply_bulk_rename(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ApplyBulkRename = message else {
            unreachable!("misrouted message in view_apply_bulk_rename");
        };
        match self.planned_rename_operations() {
            None => Task::none(),
            Some(operations) => {
                self.model.bulk_rename_busy = true;
                // Published to the shared activity center with a key threaded
                // through the reply. Renames report no bytes, so the row stays
                // indeterminate and has no controls.
                let count = operations.len();
                let activity = self.start_file_activity(
                    ActivityKind::Rename,
                    crate::i18n::format(
                        "files-renaming-count-774bf76c",
                        &[("count", (count).to_string())],
                    ),
                    count,
                );
                // Off the update thread: a batch walks the filesystem.
                Task::perform(apply_renames(operations), move |outcome| {
                    Msg::BulkRenameFinished { activity, outcome }
                })
            }
        }
    }

    pub(crate) fn view_bulk_rename_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BulkRenameFinished { activity, outcome } = message else {
            unreachable!("misrouted message in view_bulk_rename_finished");
        };
        self.model.bulk_rename_busy = false;
        let summary = outcome.summary();
        // Any failed pair fails the row, keeping the landed count; `finish`
        // refuses replies for retired attempts.
        let terminal = if outcome.failed.is_empty() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        };
        self.finish_file_activity(activity, outcome.renamed.len(), terminal, summary.clone());
        // `dst_inode` captured after the rename, so Undo rejects a replaced
        // destination.
        let now_ms = journal::now_ms();
        let mut tally = JournalAdmissionTally::default();
        for (src, dst) in &outcome.renamed {
            match journal::InodeIdent::of(dst) {
                Ok(dst_inode) => {
                    tally.observe(self.model.journal.record(
                        journal::OpRecord::Rename {
                            src: src.clone(),
                            dst: dst.clone(),
                            dst_inode,
                        },
                        now_ms,
                    ));
                }
                Err(_) => tally.note_capture_unavailable(),
            }
        }
        self.sweep_journal_prompts();
        self.post_outcome(summary, terminal);
        if let Some(msg) = tally.warning_for(crate::i18n::tr("Rename")) {
            self.post_status(msg, ToastTone::Warning);
        }
        self.model.close_bulk_rename();
        let blurred = std::mem::take(&mut self.model.inline_rename_blurred);
        if blurred {
            // The click that settled the edit chose the next selection.
            for (src, _) in &outcome.renamed {
                self.model.selection.remove(src);
            }
        } else {
            self.model.clear_selection();
        }
        // A successful inline rename focuses the renamed row after reload.
        if self.model.inline_rename.take().is_some() && !blurred {
            self.model.focus_after_load = match outcome.renamed.as_slice() {
                [(_, renamed)] => Some(renamed.clone()),
                _ => outcome.failed.first().map(|(src, _)| src.clone()),
            };
        }
        self.reload()
    }
}

// The byte length of the part of `name` a rename selects: a folder's whole
// name, a file's name before its extension. A leading dot (`.bashrc`) starts
// the name, not an extension.
fn name_stem_end(name: &str, is_dir: bool) -> usize {
    if is_dir {
        return name.len();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => dot,
        _ => name.len(),
    }
}
