// Update dispatch and window views.

use super::*;

impl App {
    pub(crate) fn update(&mut self, message: Msg) -> Task<Msg> {
        // Recorded first, so a message an early return refuses is still logged
        // (payload-free kind only).
        enzui_core::crash_report::observe(&message);

        // AT requests arriving between turns must not wait for another wake receipt.
        let a11y_pending =
            enzui_widgets::a11y_actions::apply_pending_accessibility_actions::<Msg>();
        let mut _acceptance_receipt = None;
        let message = match message {
            Msg::Acceptance(event) => {
                use enzui_widgets::app_shell::acceptance_probe::{Dispatch, dispatch};
                match dispatch(event) {
                    Dispatch::Toggle(receipt) => {
                        _acceptance_receipt = Some(receipt);
                        Msg::ToggleSidebar
                    }
                    Dispatch::Continue(task) => return Task::batch([a11y_pending, task]),
                    Dispatch::Finish => return Task::batch([a11y_pending, iced::exit()]),
                }
            }
            other => other,
        };
        let dialog_before = self.active_generic_dialog();
        let opening_invoker = self.generic_dialog_invoker(&message);
        let dismissal_event = match &message {
            Msg::DialogEvent(event @ ModalHostEvent::Dismissed(_)) => Some(*event),
            _ => None,
        };
        let refresh_projection = matches!(
            &message,
            Msg::ToggleHidden
                | Msg::ToggleFoldersFirst
                | Msg::ToggleSearch
                | Msg::BeginPathEdit
                | Msg::SearchChanged(_)
                | Msg::SetSearchScope(_)
                | Msg::SearchResultsReady { .. }
                | Msg::SetFilterType(_)
                | Msg::SetFilterDate(_)
                | Msg::NavigateTo(_)
                | Msg::NavigateScreen(_, _)
                | Msg::GoBack
                | Msg::GoForward
                | Msg::GoUp
        );
        // Scroll offsets are app state: re-parenting by a modal or sidebar toggle
        // discards the scrollable's offset, and the virtual views re-seed from it,
        // since `operate` skips the modal's inert background. `DialogEvent` focus
        // events become traversal operations; only dismissal reaches the close
        // policy.
        let task = match message {
            Msg::StackDialog(event) => {
                let (closed, focus) = self.dialogs.update(event);
                let cleanup = if closed.is_empty() {
                    Task::none()
                } else {
                    self.tail_dialog_event(Msg::DialogEvent(ModalHostEvent::Dismissed(
                        enzui_core::modal::DismissCause::CancelAction,
                    )))
                };
                Task::batch([focus, cleanup])
            }
            Msg::DialogEvent(ModalHostEvent::Dismissed(_))
                if self.model.conflicts.is_some()
                    || (self.model.bulk_rename.is_some() && self.model.bulk_rename_busy) =>
            {
                Task::none()
            }
            Msg::DialogEvent(ModalHostEvent::Dismissed(cause)) if !self.dialogs.is_empty() => {
                let key = self.dialogs.topmost().cloned().expect("open stack");
                let (closed, focus) = self.dialogs.dismiss(&key, cause);
                let cleanup = if closed.is_empty() {
                    Task::none()
                } else {
                    self.tail_dialog_event(Msg::DialogEvent(ModalHostEvent::Dismissed(cause)))
                };
                Task::batch([focus, cleanup])
            }
            Msg::DialogEvent(
                event @ (ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious),
            ) if dialog_before == Some(ActiveGenericDialog::ViewSettings) => {
                dialog::focus_task(event)
            }
            other => self.update_inner(other),
        };
        // Runs after every message: a peer overlay opened during the settings exit
        // would otherwise stay painted behind the outgoing surface until a stale
        // `ViewSettingsExitSettled`.
        settings_surface::sync(self);
        let dialog_after = self.active_generic_dialog();
        let stacked = dialog_after
            .clone()
            .filter(|key| *key != ActiveGenericDialog::ViewSettings);
        let current = self.dialogs.topmost().cloned();
        let stack_focus = match (current, stacked.clone()) {
            (None, Some(key)) => {
                let invoker = opening_invoker
                    .clone()
                    .or_else(|| self.generic_dialog_fallback_invoker(key.clone()));
                self.dialogs.open(key, invoker)
            }
            (Some(key), None) => {
                self.dialogs
                    .dismiss(&key, enzui_core::modal::DismissCause::DefaultAction)
                    .1
            }
            (Some(from), Some(to)) if from != to => self.dialogs.hand_off(&from, to),
            _ => Task::none(),
        };
        if let Some(key) = stacked {
            let busy = key == ActiveGenericDialog::Conflict
                || (key == ActiveGenericDialog::BulkRename && self.model.bulk_rename_busy);
            let policy = if busy {
                enzui_widgets::modal_host::alert_dismiss_policy()
            } else {
                enzui_widgets::modal_host::modal_dismiss_policy()
            };
            self.dialogs.set_policy(&key, policy);
        }
        let opened_attach = dialog_before != Some(ActiveGenericDialog::Attach)
            && dialog_after == Some(ActiveGenericDialog::Attach);
        let opened_unlock = dialog_before != Some(ActiveGenericDialog::Unlock)
            && dialog_after == Some(ActiveGenericDialog::Unlock);
        let settings_focus = match (dialog_before, dialog_after) {
            (before, Some(ActiveGenericDialog::ViewSettings))
                if before != Some(ActiveGenericDialog::ViewSettings) =>
            {
                self.settings_invoker = opening_invoker.or_else(|| {
                    self.generic_dialog_fallback_invoker(ActiveGenericDialog::ViewSettings)
                });
                dialog::initial_focus_task()
            }
            (Some(ActiveGenericDialog::ViewSettings), None) => {
                let invoker = self.settings_invoker.take();
                dialog::restore_focus_task(
                    dismissal_event.unwrap_or(ModalHostEvent::Dismissed(
                        enzui_core::modal::DismissCause::CloseButton,
                    )),
                    invoker.as_ref(),
                )
            }
            _ => Task::none(),
        };
        // Modal entry otherwise overrides a credential field's initial focus.
        let stack_focus = if opened_unlock {
            stack_focus.chain(iced::widget::operation::focus(
                crate::dialog::unlock_passphrase_id(),
            ))
        } else if opened_attach
            && self.model.attach.as_ref().is_some_and(|attach| {
                attach.service_target.is_some() && !attach.username.is_empty()
            })
        {
            stack_focus.chain(iced::widget::operation::focus(
                crate::dialog::sign_in_password_id(),
            ))
        } else {
            stack_focus
        };
        let focus_task = Task::batch([stack_focus, settings_focus]);
        if refresh_projection {
            self.model.refresh_visible_provider();
        }
        self.reconcile_open_context_menu();
        self.reconcile_folder_sizes();
        self.shell_activity.sync_downloads(&self.activity_center);
        self.reconcile_transfer_failure_toast();
        // Arms only the earliest terminal expiry, so idle activity never polls.
        let now = self.activity_now_ms();
        let _ = self.activity_schedule.sync(&self.activity_center, now);
        // An unchanged earliest deadline keeps its armed identity, so unrelated
        // messages never restart or duplicate the wake.
        self.toasts
            .sync_expiry(&mut self.toast_expiry, std::time::Instant::now());
        // Thumbnail scheduling runs as a post-pass so every turn applies the same
        // bounded visible-window policy. Listing calls waiting on this turn's
        // folder load are answered here too.
        #[cfg(feature = "center")]
        let waiting_calls = self.answer_waiting_center_calls();
        #[cfg(not(feature = "center"))]
        let waiting_calls = Task::none();
        #[cfg(feature = "center")]
        let graceful_stop = self.reconcile_graceful_stop();
        #[cfg(not(feature = "center"))]
        let graceful_stop = Task::none();
        Task::batch([
            a11y_pending,
            task,
            focus_task,
            self.schedule_thumbnails(),
            self.schedule_inspector(),
            waiting_calls,
            graceful_stop,
        ])
    }

    pub(crate) fn update_inner(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::Acceptance(..) => self.nav_acceptance(message),
            Msg::Theme(..) => self.nav_theme(message),
            Msg::A11yWake(..) => self.nav_a11y_wake(message),
            Msg::DirLoaded { .. } => self.nav_dir_loaded(message),
            Msg::DirPrepared { .. } => self.nav_dir_prepared(message),
            Msg::TypedPathChecked { .. } => self.reduce_typed_path_checked(message),
            Msg::WatchRescanDue { generation } => self.consume_watch_rescan_due(generation),
            Msg::ListingVerified { path, mtime } => self.consume_listing_verified(path, mtime),
            Msg::ItemDropValidated(validated) => self.finish_item_drop(*validated),
            Msg::ListingSlow { generation } => {
                if generation == self.model.load_generation && self.model.listing_pending {
                    self.model.listing_slow = true;
                }
                Task::none()
            }
            Msg::InspectorPropertiesReady { path, info } => {
                // The selected path is the guard: a reload of the same
                // folder in between does not make these details stale.
                if self.model.inspector_pending.as_ref() == Some(&path) {
                    self.model.inspector_properties = info;
                }
                Task::none()
            }
            Msg::DrivesLoaded(..) => self.nav_drives_loaded(message),
            Msg::ListScrolled(..) => self.nav_list_scrolled(message),
            Msg::GridScrolled(..) => self.nav_grid_scrolled(message),
            Msg::DownloadsScrolled(..) => self.nav_downloads_scrolled(message),
            Msg::ThumbnailLoaded(..) => self.nav_thumbnail_loaded(message),
            Msg::CapturePrepared { .. } => self.nav_capture_prepared(message),
            Msg::NavigateTo(..) => self.nav_navigate_to(message),
            #[cfg(feature = "center")]
            Msg::Center(event) => self.center_event(event),
            #[cfg(feature = "center")]
            Msg::ApplicationWake => self.application_wake(),
            #[cfg(feature = "center")]
            Msg::BackgroundTaskRefused(error) => {
                eprintln!("files-app: background task refused: {error}");
                Task::none()
            }

            Msg::NavigateScreen(..) => self.nav_navigate_screen(message),
            Msg::GoBack => self.nav_go_back(message),
            Msg::GoForward => self.nav_go_forward(message),
            Msg::GoUp => self.nav_go_up(message),
            Msg::OpenEntry(..) => self.nav_open_entry(message),
            Msg::TrashSelection => self.reduce_trash_selection(message),
            Msg::TrashFinished { .. } => self.reduce_trash_finished(message),
            Msg::TrashLoaded { .. } => self.reduce_trash_loaded(message),
            Msg::SelectTrashEntry { .. } => self.reduce_select_trash_entry(message),
            Msg::RestoreFromTrash(..) => self.reduce_restore_from_trash(message),
            Msg::TrashRestorePlanned { .. } => self.reduce_trash_restore_planned(message),
            Msg::TrashRestoreFinished { .. } => self.reduce_trash_restore_finished(message),
            Msg::AskDeleteTrashEntry(..) => self.reduce_ask_delete_trash_entry(message),
            Msg::TrashContextRowActivated(..) => self.reduce_trash_context_row_activated(message),
            Msg::EmptyTrash => self.reduce_empty_trash(message),
            Msg::TrashEmptied { .. } => self.reduce_trash_emptied(message),
            Msg::AskPermanentDelete => self.reduce_ask_permanent_delete(message),
            Msg::CancelPermanentDelete => self.reduce_cancel_permanent_delete(message),
            Msg::ConfirmPermanentDelete => self.reduce_confirm_permanent_delete(message),
            Msg::DeleteConfirmEvent(..) => self.reduce_delete_confirm_event(message),
            Msg::PermanentDeleteFinished { .. } => self.reduce_permanent_delete_finished(message),
            Msg::NewFolder => self.reduce_new_folder(message),
            Msg::NewFile => self.reduce_new_file(message),
            Msg::CreationFinished(..) => self.reduce_creation_finished(message),
            Msg::OpenLaunched { .. } => self.reduce_open_launched(message),
            Msg::SetViewMode(..) => {
                self.model.settings_menu = None;
                self.view_set_view_mode(message)
            }
            Msg::OpenSettingsMenu(menu) => {
                self.model.settings_menu = Some(menu);
                Task::none()
            }
            Msg::CloseSettingsMenus => {
                self.model.settings_menu = None;
                Task::none()
            }
            Msg::SortBy(..) => self.view_sort_by(message),
            Msg::SetSortKey(..) => {
                self.model.settings_menu = None;
                self.view_set_sort_key(message)
            }
            Msg::SetSortAscending(..) => {
                self.model.settings_menu = None;
                self.view_set_sort_ascending(message)
            }
            Msg::ToggleHidden => self.view_toggle_hidden(message),
            Msg::ToggleFoldersFirst => self.view_toggle_folders_first(message),
            Msg::ToggleSingleClick => self.view_toggle_single_click(message),
            Msg::ToggleColoredFileIcons => self.view_toggle_colored_file_icons(message),
            Msg::ToggleColumn(..) => self.view_toggle_column(message),
            Msg::ToggleGridInfo(..) => self.view_toggle_grid_info(message),
            Msg::ViewSettingsSaved(..) => self.view_view_settings_saved(message),
            Msg::ViewSettingsLoaded(..) => self.view_view_settings_loaded(message),
            Msg::ToggleDetails => self.view_toggle_details(message),
            Msg::ToggleViewSettings => self.view_toggle_view_settings(message),
            Msg::ViewSettingsExitSettled(..) => self.view_view_settings_exit_settled(message),
            Msg::ToggleToolbarOverflow => self.view_toggle_toolbar_overflow(message),
            Msg::CloseToolbarOverflow => self.view_close_toolbar_overflow(message),
            Msg::SelectSettingsPage(..) => self.view_select_settings_page(message),
            Msg::SettingsStackedBack => {
                self.model.settings_page_open = false;
                Task::none()
            }
            Msg::ShowProperties(..) => self.view_show_properties(message),
            Msg::PropertiesShortcut => {
                let path = self
                    .model
                    .selection
                    .active()
                    .filter(|path| self.model.entry_for_path(path).is_some())
                    .cloned();
                path.map_or_else(Task::none, |path| {
                    self.view_show_properties(Msg::ShowProperties(path))
                })
            }

            Msg::TogglePermissionsSection(..) => self.view_toggle_permissions_section(message),
            Msg::TogglePermissionBit { .. } => self.view_toggle_permission_bit(message),
            Msg::ApplyPermissions => self.view_apply_permissions(message),
            Msg::PropertiesRecursiveSizeReady { .. } => {
                self.view_properties_recursive_size_ready(message)
            }
            Msg::BeginBulkRename => self.view_begin_bulk_rename(message),
            Msg::RemovableDevicesLoaded(..) => self.reduce_removable_devices_loaded(message),
            Msg::DeviceAction(..) => self.reduce_device_action(message),
            Msg::ConfirmDeviceAction => self.reduce_confirm_device_action(message),
            Msg::CancelDeviceAction => self.reduce_cancel_device_action(message),
            Msg::DeviceActionFinished { .. } => self.reduce_device_action_finished(message),
            Msg::StorageLoaded(..) => self.reduce_storage_loaded(message),
            Msg::MountDiskImage(path) => self.reduce_mount_disk_image(path),
            Msg::OpenStorage(..) | Msg::UnmountStorage(..) => {
                self.reduce_storage_operation(message)
            }
            Msg::StorageOperationFinished { .. } => self.reduce_storage_finished(message),
            Msg::MountServiceFinished { .. } => self.reduce_mount_service_finished(message),
            Msg::OpenLocationContextMenu(..)
            | Msg::ShowLocationMenu(..)
            | Msg::DeviceRowPressed { .. }
            | Msg::LocationAction(..)
            | Msg::LocationUpdated { .. }
            | Msg::LocationRenameChanged(..)
            | Msg::LocationShareChosen(..)
            | Msg::ConfirmLocationDialog
            | Msg::CloseLocationDialog => self.reduce_location(message),
            Msg::UnlockPassphraseChanged(..)
            | Msg::ToggleUnlockRevealed
            | Msg::ConfirmUnlock
            | Msg::CloseUnlock => self.reduce_unlock(message),
            Msg::FormatLabelChanged(..)
            | Msg::FormatFileSystemChosen(..)
            | Msg::SetFormatWholeDrive(..)
            | Msg::ConfirmFormat
            | Msg::CloseFormat
            | Msg::FormatFinished { .. }
            | Msg::OpenFormatted => self.reduce_format(message),
            Msg::ShowAddCloud
            | Msg::CloudProviderChosen(..)
            | Msg::CloudServerChanged(..)
            | Msg::ConfirmCloudServer
            | Msg::CloudBack
            | Msg::CloudAdded { .. }
            | Msg::CancelCloudSignIn
            | Msg::OpenCloudDrive
            | Msg::CloseCloud => self.reduce_cloud(message),

            Msg::PhonesLoaded(..) => self.reduce_phones_loaded(message),
            Msg::OpenPhone(..) => self.reduce_open_phone(message),
            Msg::PhoneOpened { .. } => self.reduce_phone_opened(message),
            Msg::InlineRenameChanged(..) => self.view_inline_rename_changed(message),
            Msg::CommitInlineRename => self.view_commit_inline_rename(message),
            Msg::InlineRenameBlurred => self.view_inline_rename_blurred(message),
            Msg::CancelInlineRename => self.view_cancel_inline_rename(message),
            Msg::CancelBulkRename => self.view_cancel_bulk_rename(message),
            Msg::SetBulkRenameMode(..) => self.view_set_bulk_rename_mode(message),
            Msg::BulkRenameDirectChanged(..) => self.view_bulk_rename_direct_changed(message),
            Msg::BulkRenameFindChanged(..) => self.view_bulk_rename_find_changed(message),
            Msg::BulkRenameReplaceChanged(..) => self.view_bulk_rename_replace_changed(message),
            Msg::BulkRenameTemplateChanged(..) => self.view_bulk_rename_template_changed(message),
            Msg::BulkRenameStartChanged(..) => self.view_bulk_rename_start_changed(message),
            Msg::BulkRenameStepChanged(..) => self.view_bulk_rename_step_changed(message),
            Msg::BulkRenamePadChanged(..) => self.view_bulk_rename_pad_changed(message),
            Msg::ResetBulkRename => self.view_reset_bulk_rename(message),
            Msg::ApplyBulkRename => self.view_apply_bulk_rename(message),
            Msg::BulkRenameFinished { .. } => self.view_bulk_rename_finished(message),

            Msg::CutSelection | Msg::CopySelection => self.reduce_cut_selection(message),
            Msg::PasteShortcut => self.reduce_paste_shortcut(message),
            Msg::DownloadLinkPasted(..) => self.reduce_download_link_pasted(message),
            Msg::PasteHere => self.reduce_paste_here(message),
            Msg::FileClipboardFinished(..) => self.reduce_file_clipboard_finished(message),
            Msg::CopyPreviewSelection(..) => self.reduce_copy_preview_selection(message),
            Msg::FileItemDropped(..) => self.reduce_file_item_dropped(message),
            Msg::BeginMoveSelectionToFolder => self.reduce_begin_move_selection_to_folder(message),
            Msg::BeginCopySelectionToFolder => self.reduce_begin_copy_selection_to_folder(message),
            Msg::MoveSelectionToFolder(..) => self.reduce_move_selection_to_folder(message),
            Msg::CopySelectionToFolder(..) => self.reduce_copy_selection_to_folder(message),
            Msg::ExtractArchive { .. } => self.reduce_extract_archive(message),
            Msg::BeginExtractArchiveTo(..) => self.reduce_begin_extract_archive_to(message),
            Msg::CompressSelection(..) => self.reduce_compress_selection(message),
            Msg::OpenArchiveExternally(path) => self.start_extract_archive(path, None, true),
            Msg::ArchiveHandlerResolved { .. } => self.nav_archive_handler_resolved(message),
            Msg::ArchiveSettled { .. } => self.reduce_archive_settled(message),
            Msg::MeasureFolder(..) => self.reduce_measure_folder(message),
            Msg::CancelFolderMeasure(..) => self.reduce_cancel_folder_measure(message),
            Msg::FolderMeasure { .. } => self.reduce_folder_measure(message),
            // A right click elsewhere settles an in-place rename, as a left
            // click does (a right click leaves the field focused).
            Msg::OpenContextMenu(..) => {
                let settle = self.settle_inline_rename();
                Task::batch([settle, self.reduce_open_context_menu(message)])
            }
            Msg::OpenSidebarContextMenu(..) => {
                let settle = self.settle_inline_rename();
                Task::batch([settle, self.reduce_open_sidebar_context_menu(message)])
            }
            Msg::OpenBackgroundContextMenu(..) => {
                let settle = self.settle_inline_rename();
                Task::batch([settle, self.reduce_open_background_context_menu(message)])
            }

            Msg::ContextMenuEvent(..) => self.reduce_context_menu_event(message),
            Msg::BackgroundContextRowActivated(..) => {
                self.reduce_background_context_row_activated(message)
            }
            Msg::ContextRowActivated(..) => self.reduce_context_row_activated(message),
            Msg::BeginPaste { .. } => self.reduce_begin_paste(message),
            Msg::PastePlanned { .. } => self.reduce_paste_planned(message),
            Msg::PasteExecuted { .. } => self.reduce_paste_executed(message),
            Msg::SetApplyToAllConflicts(..) => self.reduce_set_apply_to_all_conflicts(message),
            Msg::PauseConflicts => self.reduce_pause_conflicts(message),
            Msg::ResumeConflicts => self.reduce_resume_conflicts(message),
            Msg::CancelConflicts => self.reduce_cancel_conflicts(message),
            Msg::ResolveConflict(..) => self.reduce_resolve_conflict(message),
            Msg::ConflictBatchFinished { .. } => self.reduce_conflict_batch_finished(message),

            Msg::ToggleSearch => self.reduce_toggle_search(message),
            Msg::SearchChanged(..) => self.reduce_search_changed(message),
            Msg::SetSearchScope(..) => self.reduce_set_search_scope(message),
            Msg::SearchResultsReady { .. } => self.reduce_search_results_ready(message),
            Msg::ToggleFilters => self.reduce_toggle_filters(message),
            Msg::SearchFiltersOpenChanged(..) => self.reduce_search_filters_open_changed(message),
            Msg::SetFilterType(..) => self.reduce_set_filter_type(message),
            Msg::SetFilterDate(..) => self.reduce_set_filter_date(message),
            Msg::BeginPathEdit => self.reduce_begin_path_edit(message),
            Msg::PathDraftChanged(..) => self.reduce_path_draft_changed(message),
            Msg::PathSuggestionsReady { .. } | Msg::PathSuggestionAccepted(..) => {
                self.reduce_path_suggestion(message)
            }
            Msg::CommitPathEdit => self.reduce_commit_path_edit(message),
            Msg::CancelPathEdit => self.reduce_cancel_path_edit(message),
            Msg::DismissPathEdit => self.reduce_dismiss_path_edit(message),
            Msg::CopyCwdPath => self.reduce_copy_cwd_path(message),
            Msg::CopyRowPath(..) => self.reduce_copy_row_path(message),

            Msg::RowClickPrimary { .. } => self.reduce_row_click_primary(message),
            Msg::SelectOnly(..) => self.reduce_select_only(message),
            Msg::SpaceOnRow(..) => self.reduce_space_on_row(message),
            Msg::ToggleSelect(..) => self.reduce_toggle_select(message),
            Msg::SelectRange(..) => self.reduce_select_range(message),
            Msg::SelectAll => self.reduce_select_all(message),
            Msg::ClearSelection => self.reduce_clear_selection(message),
            Msg::MoveSelection { .. } => self.reduce_move_selection(message),
            Msg::TypeAhead(..) => self.reduce_type_ahead(message),
            Msg::Zoom(..) => self.view_zoom(message),
            Msg::OpenInTerminal(..) => self.view_open_in_terminal(message),
            Msg::Project(message) => self.project_message(message),
            Msg::History(message) => self.history_message(message),
            Msg::TemplatesScanned(..) => self.reduce_templates_scanned(message),
            Msg::NewTab
            | Msg::CloseTab(..)
            | Msg::SwitchTab(..)
            | Msg::NextTab
            | Msg::PreviousTab
            | Msg::ToggleTabList => self.reduce_tabs(message),
            Msg::FolderIdentitiesScanned { .. }
            | Msg::ShowFolderColor(..)
            | Msg::CloseFolderColor
            | Msg::SetFolderColor(..)
            | Msg::FolderColorIdentified { .. } => self.reduce_folder_color(message),
            Msg::DuplicateSelection | Msg::LinkSelection | Msg::LinkSources { .. } => {
                self.reduce_create_batch(message)
            }
            Msg::PasteAsLink => self.begin_file_paste(true),
            Msg::CreatedBatch { .. } => self.reduce_created_batch(message),
            Msg::NewFromTemplate(..) => self.reduce_new_from_template(message),
            Msg::TerminalLaunched(..) => self.view_terminal_launched(message),
            Msg::SetTerminalCommand(..) => self.view_set_terminal_command(message),
            Msg::OpenSelectMatching => self.reduce_open_select_matching(message),
            Msg::CloseSelectMatching => self.reduce_close_select_matching(message),
            Msg::SelectMatchingDraftChanged(..) => {
                self.reduce_select_matching_draft_changed(message)
            }
            Msg::SelectMatchingSubmit => self.reduce_select_matching_submit(message),

            Msg::MarqueeResolved(..) | Msg::MarqueePreview(..) => {
                self.reduce_marquee_resolved(message)
            }
            Msg::SidebarState(state) => {
                self.model.nav = state;
                Task::none()
            }
            Msg::ToggleSidebar => self.reduce_toggle_sidebar(message),
            Msg::FocusNext => self.reduce_focus_next(message),
            Msg::FocusPrevious => self.reduce_focus_previous(message),
            Msg::ClearFocus(window) if self.main_window == Some(window) => {
                self.reduce_clear_focus(message)
            }
            Msg::ClearFocus(..) => self.reduce_clear_focus_2(message),
            Msg::WindowResized(..) => self.reduce_window_resized(message),
            Msg::GridLayoutSettled => self.reduce_grid_layout_settled(message),
            Msg::Window(..) => self.reduce_window(message),
            Msg::MainWindowOpened(..) => self.reduce_main_window_opened(message),
            Msg::ActivityWindowTagged(id, tag, queued) => {
                if self.main_window == Some(id) {
                    self.shell_activity.confirm_tag(tag, queued);
                }
                Task::none()
            }
            Msg::OpenPreview(..) => self.reduce_open_preview(message),
            Msg::PreviewSelection(..) => self.reduce_preview_selection(message),
            Msg::PreviewPrepared(..) => self.reduce_preview_prepared(message),
            Msg::PreviewMediaWake(..)
            | Msg::PreviewMediaControl(..)
            | Msg::PreviewPdfPage(..)
            | Msg::PreviewPdfLoaded(..) => self.reduce_preview_content(message),
            Msg::PreviewWindowOpened(..) => self.reduce_preview_window_opened(message),
            Msg::CloseRequested(..) => self.reduce_close_requested(message),
            Msg::PreviewWindow(id, command) => {
                if command == enzui_widgets::title_bar_gesture::WindowCommand::Close {
                    self.reduce_close_requested(Msg::CloseRequested(id))
                } else {
                    command.into_task(id)
                }
            }
            Msg::CleanupDrainComplete => self.reduce_cleanup_drain_complete(message),
            Msg::ShutdownHelperDone(..) => self.reduce_shutdown_helper_done(message),
            Msg::WindowClosed(..) => self.reduce_window_closed(message),

            Msg::BeginInstallExtension
            | Msg::ReviewExtension(..)
            | Msg::ExtensionReviewed(..)
            | Msg::CancelExtensionReview
            | Msg::ConfirmInstallExtension
            | Msg::ExtensionsLoaded(..)
            | Msg::ChangeExtension(..)
            | Msg::ExtensionsChanged(..)
            | Msg::ExtensionDetails(..)
            | Msg::ShowDownloadsSettings(..)
            | Msg::ExtensionSettingEdited { .. }
            | Msg::SaveExtensionSettings(..)
            | Msg::AskRemoveExtension(..)
            | Msg::ConfirmRemoveExtension
            | Msg::RunExtension { .. }
            | Msg::ExtensionFinished(..)
            | Msg::CancelExtensionAction => self.reduce_extensions(message),
            Msg::Shortcut(..) | Msg::ShortcutsSaved(..) => self.reduce_shortcuts(message),
            Msg::ToggleFeature(..) => self.aux_toggle_feature(message),
            Msg::RetryFeatureSettings => self.aux_retry_feature_settings(message),
            Msg::ReplaceFeatureSettings => self.aux_replace_feature_settings(message),
            Msg::TorrentDownloadLimitChanged(..) => {
                self.aux_torrent_download_limit_changed(message)
            }
            Msg::TorrentUploadLimitChanged(..) => self.aux_torrent_upload_limit_changed(message),
            Msg::DownloadConcurrencyChanged(..) => self.aux_download_concurrency_changed(message),
            Msg::ApplyDownloadSettings => self.aux_apply_download_settings(message),
            Msg::RetryDownloadSettings => self.aux_retry_download_settings(message),
            Msg::RetireDownloadSettings => self.aux_retire_download_settings(message),
            Msg::Aria2Probed(..) => self.aux_aria2_probed(message),
            Msg::ShowAddDownload => self.aux_show_add_download(message),
            Msg::CloseAddDownload => self.aux_close_add_download(message),
            Msg::AddDownloadUrlChanged(..) => self.aux_add_download_url_changed(message),
            Msg::ToggleDownloadAdvanced => self.aux_toggle_download_advanced(message),
            Msg::AddDownloadLimitChanged(..) => self.aux_add_download_limit_changed(message),
            Msg::EnqueueDownload => self.aux_enqueue_download(message),
            Msg::EnqueueTorrent(..) => self.aux_enqueue_torrent(message),
            Msg::DownloadAction(..) => self.aux_download_action(message),
            Msg::TorrentTelemetry(..) => self.aux_torrent_telemetry(message),
            Msg::TransferEvent(..) => self.aux_transfer_event(message),
            Msg::TransferOutcome(..) => self.aux_transfer_outcome(message),
            Msg::PublicationReconciled(..) => self.aux_publication_reconciled(message),
            Msg::PausedDownloadCleaned(..) => self.aux_paused_download_cleaned(message),
            Msg::CleanupRetryTick => self.aux_cleanup_retry_tick(message),
            Msg::ActivityControl(..) => self.aux_activity_control(message),
            Msg::ShowActivityOverflow(..) => self.aux_show_activity_overflow(message),
            Msg::CloseAllActivity => self.aux_close_all_activity(message),
            Msg::ActivityPrune => self.aux_activity_prune(message),
            Msg::UnmountDrive(..) => self.aux_unmount_drive(message),
            Msg::DriveUnmounted { .. } => self.aux_drive_unmounted(message),
            Msg::PinDragHover(..) => self.aux_pin_drag_hover(message),
            Msg::PinDragLeave => self.aux_pin_drag_leave(message),
            Msg::PinDrop(..) => self.aux_pin_drop(message),
            Msg::ReorderSidebarPins(..) => self.aux_reorder_sidebar_pins(message),
            Msg::PinToSidebar(..) => self.aux_pin_to_sidebar(message),
            Msg::UnpinFromSidebar(..) => self.aux_unpin_from_sidebar(message),
            Msg::RetryPins => self.aux_retry_pins(message),
            Msg::RetirePins => self.aux_retire_pins(message),
            Msg::PickerParentReady(request, reply) => {
                self.picker.parent_ready(request, reply, self.main_window)
            }
            Msg::PickerFallback(event) => self.picker.handle(event),
            Msg::DownloadDestinationFallback(event) => {
                let (answered, focus) = self.model.downloads.handle_destination_fallback(event);
                if answered {
                    Task::batch([focus, self.finish_download_destinations()])
                } else {
                    focus
                }
            }
            Msg::NativeFilesDropped { drop, target } => self.native_files_dropped(drop, target),
            Msg::NativeDropReceived(delivery) => self.native_drop_received(delivery),
            Msg::NativeContentDropped { drop, target } => self.native_content_dropped(drop, target),
            Msg::NativeContentSaved(delivery) => self.native_content_saved(delivery),
            Msg::StackDialog(_) => Task::none(),
            Msg::DesktopLaunchWake => self.desktop_launch_wake(),
            Msg::DesktopLaunchActivated => Task::none(),
            Msg::PortalTick => self.aux_portal_tick(message),
            Msg::PortalOutcome(..) => self.aux_portal_outcome(message),

            Msg::ShowOpenWith(..) => self.remote_show_open_with(message),
            Msg::OpenWithQueried { .. } => self.remote_open_with_queried(message),
            Msg::CloseOpenWith => self.remote_close_open_with(message),
            Msg::SelectOpenCandidate(..) => self.remote_select_open_candidate(message),
            Msg::SetOpenBehavior(..) => self.remote_set_open_behavior(message),
            Msg::OpenWithCommandChanged(..) => self.remote_open_with_command_changed(message),
            Msg::ConfirmOpenWith => self.remote_confirm_open_with(message),
            Msg::OpenWithLaunched { .. } => self.remote_open_with_launched(message),
            Msg::MimeDefaultPersisted { .. } => self.remote_mime_default_persisted(message),
            Msg::ShowAttach => self.remote_show_attach(message),
            Msg::CloseAttach => self.remote_close_attach(message),
            Msg::SetAttachProtocol(..) => self.remote_set_attach_protocol(message),
            Msg::AttachServerChanged(..) => self.remote_attach_server_changed(message),
            Msg::AttachShareChanged(..) => self.remote_attach_share_changed(message),
            Msg::AttachDomainChanged(..) => self.remote_attach_domain_changed(message),
            Msg::AttachUsernameChanged(..) => self.remote_attach_username_changed(message),
            Msg::AttachPasswordChanged(..) => self.remote_attach_password_changed(message),
            Msg::ToggleAttachPasswordRevealed => {
                self.remote_toggle_attach_password_revealed(message)
            }
            Msg::SetAttachAutoConnect(enabled) => {
                if let Some(attach) = &mut self.model.attach {
                    attach.auto_connect = enabled;
                }
                Task::none()
            }
            Msg::SetAttachMountAtBoot(enabled) => {
                if let Some(attach) = &mut self.model.attach {
                    attach.mount_at_boot = enabled;
                }
                Task::none()
            }
            Msg::SetAttachRememberPassword(remember) => {
                if let Some(attach) = &mut self.model.attach {
                    attach.remember_password = remember;
                }
                Task::none()
            }
            Msg::DiscoverSmb => self.remote_discover_smb(message),
            Msg::AttachDiscovered(..) => self.remote_attach_discovered(message),
            Msg::AdoptDiscovered(..) => self.remote_adopt_discovered(message),
            Msg::ConfirmAttach => self.remote_confirm_attach(message),
            Msg::AttachMounted(..) => self.remote_attach_mounted(message),
            Msg::MountQuestionAsked(..) | Msg::MountQuestionAnswered(..) => {
                self.remote_mount_question(message)
            }
            Msg::DiscoverNetwork => self.remote_discover_network(message),
            Msg::NetworkDiscovered(..) => self.remote_network_discovered(message),
            Msg::MountDiscovered(..) => self.remote_mount_discovered(message),
            Msg::CancelNetworkOperation => self.remote_cancel_network_operation(message),

            Msg::DialogEvent(ModalHostEvent::Dismissed(_)) => self.tail_dialog_event(message),
            Msg::DialogEvent(ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious) => {
                self.tail_dialog_event_2(message)
            }
            Msg::OpenShortcuts => self.tail_open_shortcuts(message),
            Msg::CloseShortcuts => self.tail_close_shortcuts(message),
            Msg::Toast(..) => self.tail_toast(message),
            Msg::Refresh => self.tail_refresh(message),
            Msg::TreeWatch(..) => self.tail_tree_watch(message),
            Msg::WatchArmed { .. } => self.tail_watch_armed(message),
            Msg::ShellShortcut(..) => self.tail_shell_shortcut(message),
            Msg::UndoLastOperation => self.tail_undo_last_operation(message),
            Msg::RedoLastUndone => self.tail_redo_last_undone(message),
            Msg::UndoOperationFinished { .. } => self.tail_undo_operation_finished(message),
            Msg::RedoOperationFinished { .. } => self.tail_redo_operation_finished(message),
            Msg::ResolveJournalConflict { .. } => self.tail_resolve_journal_conflict(message),

            Msg::Noop => Task::none(),
        }
    }

    // The daemon's `window::Id`-keyed view: a preview or the browser.
    //
    // `iced::application`'s `view` has no window id, which is why multi-window
    // needs a daemon. `main_view` stays separate so tests call what the runtime
    // calls.
    pub(crate) fn view_window(&self, id: iced::window::Id) -> Element<'_, Msg> {
        match self.previews.get(&id) {
            Some(p) => view::preview::view(p, self.themes.theme(), id),
            None => self.view(),
        }
    }
}
