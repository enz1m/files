// App view and subscriptions.

use super::*;

impl App {
    pub(crate) fn view(&self) -> Element<'_, Msg> {
        let shell = view::shell::view(
            &self.model,
            &self.activity_center,
            self.themes.theme(),
            self.window_height,
            self.native_drop_allowed(),
        );

        // A non-modal popover mounted below the dialog chain, so it never floats
        // over a dialog. Point-anchored at the `enz_context_target` hit, since
        // virtualised rows cannot be measured; the dismissing press is consumed.
        // Mounted every frame so opening it does not rebuild the scroller tree.
        let mut host = match &self.model.context_menu {
            Some(open) => {
                let generation = open.generation;
                open.menu
                    .host(shell, move |event| Msg::ContextMenuEvent(generation, event))
            }
            None => enzui_widgets::context_menu_host::enz_optional_point_context_menu_host(
                shell,
                None,
                |event| Msg::ContextMenuEvent(0, event),
            ),
        };
        let prefix = self.model.context_menu.as_ref().map_or_else(
            || "files-app-context-menu".to_owned(),
            |open| format!("files-app-context-menu-{}", open.generation),
        );
        host = host.id_prefix(prefix);
        if let Some(label) = context_menu::menu_label(&self.model) {
            host = host.label(label);
        }
        let shell: Element<'_, Msg> = host.into_element();
        // Every dialog gets the live viewport, so the presenter can cap only the
        // scrolling body at narrow rungs.
        let window = iced::Size::new(
            if self.window_width > 0.0 {
                self.window_width
            } else {
                1024.0
            },
            if self.window_height > 0.0 {
                self.window_height
            } else {
                900.0
            },
        );

        let shell = settings_surface::view_settings(
            self.themes.theme(),
            &self.model,
            &self.shortcuts,
            shell,
            window,
            self.model.view_settings_open,
            settings_surface::presentation_key(self),
        );
        let shell = self.dialogs.view_with(
            shell,
            |_| self.generic_dialog_surface(window),
            Msg::StackDialog,
            |key, host| {
                use ActiveGenericDialog as Dialog;
                let label = match key {
                    Dialog::Conflict => crate::i18n::tr("File conflict"),
                    Dialog::BulkRename => self
                        .model
                        .bulk_rename
                        .as_ref()
                        .map_or_else(|| crate::i18n::tr("Rename"), |state| state.title()),
                    Dialog::Properties => crate::i18n::tr("Properties"),
                    Dialog::OpenWith => crate::i18n::tr("Open With"),
                    Dialog::Attach => match self.model.attach.as_ref() {
                        Some(state) if state.service_target.is_some() => {
                            crate::i18n::tr(crate::dialog::SIGN_IN_TITLE)
                        }
                        Some(state) if state.service => {
                            crate::i18n::tr(crate::dialog::CONNECT_TITLE)
                        }
                        _ => crate::i18n::tr("Attach network location"),
                    },
                    Dialog::AddDownload => crate::i18n::tr(crate::downloads::ADD_DOWNLOAD_LABEL),
                    Dialog::SelectMatching => {
                        crate::i18n::tr(crate::view::shell::SELECT_MATCHING_TITLE)
                    }
                    Dialog::Shortcuts => crate::i18n::tr("Keyboard Shortcuts"),
                    Dialog::AllActivity => crate::i18n::tr("All activity"),
                    Dialog::FolderColor => crate::i18n::tr("Folder Color"),
                    Dialog::DeviceConfirm => {
                        if self
                            .model
                            .pending_device_confirm
                            .as_ref()
                            .is_some_and(|(_, op)| *op == crate::devices::DeviceOp::PowerOff)
                        {
                            crate::i18n::tr("Safely Remove")
                        } else {
                            crate::i18n::tr("Eject")
                        }
                    }
                    Dialog::MountQuestion => crate::i18n::tr("Verify the server"),
                    Dialog::Unlock => crate::i18n::tr(crate::dialog::UNLOCK_TITLE),
                    Dialog::Location => self
                        .model
                        .location_dialog
                        .as_ref()
                        .map_or("", crate::dialog::location_title),
                    Dialog::Format => crate::i18n::tr(crate::dialog::FORMAT_TITLE),
                    Dialog::Cloud => crate::i18n::tr(crate::dialog::CLOUD_TITLE),
                    Dialog::History => match self.model.history {
                        Some(crate::history::Dialog::Browse(_)) => {
                            crate::i18n::tr(crate::dialog::BROWSE_TITLE)
                        }
                        _ => crate::i18n::tr(crate::dialog::VERSIONS_TITLE),
                    },
                    Dialog::ViewSettings => crate::i18n::tr("Settings"),
                };
                host.label(label).alert(*key == Dialog::Conflict)
            },
        );
        let shell = dialog::delete_confirm(
            self.model.pending_permanent_delete.as_ref(),
            &self.model.delete_confirm_state,
            shell,
        );
        let presentation = shell;

        // The toast layer floats over the whole presentation, dialogs included: a
        // modal makes its background inert, which would withdraw an in-shell toast
        // from the accessibility tree. Both layers are always mounted, so a toast
        // arriving does not re-parent the browser.
        let presentation: Element<'_, Msg> = iced::widget::Stack::with_children(
            std::iter::once(presentation).chain(view::journal_prompt::overlay_if_any(&self.model)),
        )
        .into();
        enz_toast_layer(
            self.model.downloads.destination_view(
                self.picker.view(presentation),
                Msg::DownloadDestinationFallback,
            ),
            (!self.toasts.is_empty()).then(|| self.toasts.view().map(Msg::Toast)),
            ToastOverlayPlacement::default(),
        )
    }
    fn generic_dialog_surface(&self, window: iced::Size) -> Element<'_, Msg> {
        if let Some(queue) = &self.model.conflicts {
            // First: a paused paste hidden behind another dialog could not finish.
            dialog::conflicts(
                self.themes.theme(),
                queue,
                self.model.conflicts_busy,
                window,
            )
        } else if let Some(asked) = &self.model.mount_question {
            dialog::mount_question(self.themes.theme(), &asked.question, window)
        } else if let Some(state) = &self.model.unlock {
            let detail = self
                .model
                .storage_locations
                .iter()
                .find(|location| location.id == state.id)
                .and_then(|location| location.service.as_ref()?.detail.clone());
            dialog::unlock(self.themes.theme(), state, detail, window)
        } else if let Some(state) = &self.model.location_dialog {
            dialog::location(self.themes.theme(), state, &self.model, window)
        } else if let Some(state) = &self.model.format {
            dialog::format(self.themes.theme(), state, window)
        } else if let Some(state) = &self.model.cloud {
            dialog::cloud(
                self.themes.theme(),
                state,
                &self.model.cloud_providers,
                window,
            )
        } else if let Some(state) = &self.model.bulk_rename {
            dialog::bulk_rename(
                self.themes.theme(),
                state,
                self.model.bulk_rename_busy,
                window,
            )
        } else if let Some((id, op)) = &self.model.pending_device_confirm {
            let label = self
                .model
                .removable_devices
                .iter()
                .find(|device| &device.id == id)
                .map_or_else(
                    || crate::i18n::tr("this drive"),
                    |device| device.label.as_str(),
                );
            dialog::device_confirm(self.themes.theme(), label, *op, window)
        } else if let Some(info) = &self.model.properties {
            // Viewport-sized so the metadata can reflow to one column at 375.
            dialog::properties(
                self.themes.theme(),
                info,
                crate::icon::properties_art(&self.model, info),
                self.model.properties_perms_expanded,
                // `Apply Permissions` stays inert until a bit is edited.
                self.model.permissions_dirty(),
                (self.model.history_available
                    && crate::history::covers(&self.model.home, &info.path))
                .then(|| crate::dialog::BackupInfo {
                    path: info.path.clone(),
                    // Directories show an em dash for their own size.
                    dir: info.size_text == "\u{2014}",
                    exclusion: crate::history::exclusion(&self.model.history_excluded, &info.path),
                }),
                window,
            )
        } else if let Some(ow) = &self.model.open_with {
            dialog::open_with(self.themes.theme(), ow, window)
        } else if let Some(attach) = &self.model.attach {
            dialog::attach(self.themes.theme(), attach, window)
        } else if let Some(add) = &self.model.downloads.add {
            dialog::add_download(
                self.themes.theme(),
                add,
                self.model.downloads.backend,
                window,
            )
        } else if self.model.select_matching_open {
            dialog::select_matching(self.themes.theme(), &self.model, window)
        } else if self.model.shortcuts_open {
            dialog::shortcuts()
        } else if let Some(history) = &self.model.history {
            match history {
                crate::history::Dialog::Versions(d) => {
                    dialog::previous_versions(self.themes.theme(), d, window)
                }
                crate::history::Dialog::Browse(d) => {
                    dialog::browse_history(self.themes.theme(), d, window)
                }
            }
        } else if let Some(path) = &self.model.folder_color_dialog {
            dialog::folder_color(self.themes.theme(), &self.model, path, window)
        } else if self.model.all_activity_open {
            // Fed the same centre as the surface, so it lists exactly its rows.
            dialog::all_activity(self.themes.theme(), &self.activity_center, window)
        } else {
            iced::widget::Space::new().into()
        }
    }
    pub(crate) fn subscription(&self) -> Subscription<Msg> {
        // `Opened` is ignored: after a compositor restart it can carry the previous
        // output's width for a frame. Escape is decoded by the modal host. The
        // window id is kept, because a daemon delivers every window's events here.
        let events = iced::event::listen_with(|event, status, id| {
            classify_event_with_status(&event, status, id)
        });

        // Without `close_events()` the ledger never sees a close and the process
        // outlives its last window.
        let closes = iced::window::close_events().map(Msg::WindowClosed);
        let close_requests = iced::window::close_requests().map(Msg::CloseRequested);

        let retirement_retry = self.close_pending.is_some()
            && self
                .close_deadline
                .is_some_and(|deadline| std::time::Instant::now() < deadline);
        let cleanup_debt = self.model.downloads.has_cleanup_debt()
            && !self.model.downloads.is_owner_uncertain()
            && (self.close_pending.is_none() || retirement_retry);
        let cleanup_tick = if cleanup_debt || self.shutdown_helper.is_some() || retirement_retry {
            let interval = if self.shutdown_helper.is_some() {
                std::time::Duration::from_millis(100)
            } else {
                std::time::Duration::from_secs(2)
            };
            iced::time::every(interval).map(|_| Msg::CleanupRetryTick)
        } else {
            Subscription::none()
        };

        let mut subscriptions = vec![
            events,
            closes,
            close_requests,
            cleanup_tick,
            self.themes
                .subscription()
                .map(|event| Msg::Theme(Box::new(event))),
        ];
        for (&id, preview) in &self.previews {
            if let crate::preview::PreviewContent::Media { player, .. } = &preview.content {
                subscriptions.push(
                    player
                        .service
                        .subscription()
                        .with(id)
                        .map(|(id, _)| Msg::PreviewMediaWake(id)),
                );
            }
        }
        if let Some(owner) = &self.desktop_launch {
            subscriptions.push(owner.subscription(|_| Msg::DesktopLaunchWake));
        }
        let folder_wake = self.picker.completion_subscription();
        let save_wake = self.model.downloads.destination_completion_subscription();
        let open_wake = self.file_opener.completion_subscription();
        subscriptions.push(folder_wake.map(|_| Msg::PortalTick));
        subscriptions.push(save_wake.map(|_| Msg::PortalTick));
        subscriptions.push(open_wake.map(|_| Msg::PortalTick));
        subscriptions.push(self.dir_watch.delivery_subscription().map(Msg::TreeWatch));
        subscriptions.push(self.dir_watch.arm_subscription());
        if !self.model.storage_capture {
            subscriptions.push(crate::storage::subscription().map(Msg::StorageLoaded));
        }
        if self.archive_work.has_active() {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(150))
                    .map(|_| Msg::ArchiveProgress),
            );
        }
        if let Some(deadline) = self.activity_schedule.armed_deadline() {
            let remaining = deadline.saturating_sub(self.activity_now_ms()).max(1);
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(remaining))
                    .map(|_| Msg::ActivityPrune),
            );
        }
        // One deadline-keyed wake while a toast can expire; nothing when idle.
        if let Some((deadline, wait)) = self.toast_expiry.armed() {
            subscriptions.push(
                iced::time::every(wait)
                    .with(deadline)
                    .map(|(_deadline, now)| Msg::Toast(ToastEvent::Tick(now))),
            );
        }
        use enzui_widgets::app_shell::acceptance_probe::{self, Arm};
        let rows = self.model.visible_provider.len() as usize;
        let ready = match acceptance_probe::selected_arm("files") {
            Some(Arm::Busy) => rows >= 1000,
            Some(Arm::Cheap) => rows <= 1,
            None => false,
        };
        subscriptions
            .push(acceptance_probe::subscription("files", ready, rows).map(Msg::Acceptance));
        subscriptions.push(enzui_core::app_runtime::accessibility_wake_subscription(
            Msg::A11yWake,
        ));
        #[cfg(feature = "center")]
        subscriptions.push(self.center_subscription());
        #[cfg(feature = "center")]
        subscriptions.push(self.agent_subscription());

        Subscription::batch(subscriptions)
    }
}
