// Dialog, shortcuts, watch, and journal reducer arms.

use super::*;

impl App {
    pub(crate) fn tail_dialog_event(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DialogEvent(ModalHostEvent::Dismissed(_)) = message else {
            unreachable!("misrouted message in tail_dialog_event");
        };
        // Dismiss exactly the emitting presentation. Conflict resolution consumes
        // Escape and backdrop presses; only its Cancel abandons the operation.
        match self.active_generic_dialog() {
            Some(ActiveGenericDialog::Conflict) | None => {}
            Some(ActiveGenericDialog::BulkRename) => {
                if !self.model.bulk_rename_busy {
                    self.model.bulk_rename = None;
                }
            }
            Some(ActiveGenericDialog::Properties) => self.model.close_properties(),
            Some(ActiveGenericDialog::OpenWith) => {
                self.cancel_mime_query();
                self.model.open_with = None;
            }
            Some(ActiveGenericDialog::Attach) => {
                if let Some(cancel) = self.network_cancel.take() {
                    cancel.cancel();
                }
                if let Some(handle) = self.network_operation.take() {
                    handle.abort();
                }
                self.model.attach = None;
            }
            Some(ActiveGenericDialog::AddDownload) => {
                if let Some(id) = self.add_download_submitting() {
                    self.model.downloads.cancel_destination(id);
                }
                self.model.downloads.add = None;
            }
            Some(ActiveGenericDialog::ViewSettings) => {
                self.model.view_settings_open = false;
                self.model.downloads.reset_settings_draft();
                // The surface stays mounted until `ViewSettingsExitSettled` arrives
                // with the matching generation.
                settings_surface::sync(self);
            }
            Some(ActiveGenericDialog::SelectMatching) => {
                self.model.select_matching_open = false;
            }
            Some(ActiveGenericDialog::Shortcuts) => {
                self.model.shortcuts_open = false;
            }
            Some(ActiveGenericDialog::AllActivity) => {
                self.model.all_activity_open = false;
            }
            Some(ActiveGenericDialog::History) => {
                let busy = matches!(&self.model.history, Some(crate::history::Dialog::Versions(d)) if d.busy)
                    || matches!(&self.model.history, Some(crate::history::Dialog::Browse(d)) if d.busy);
                if !busy {
                    self.model.history = None;
                }
            }
            Some(ActiveGenericDialog::DeviceConfirm) => {
                self.model.pending_device_confirm = None;
            }
            Some(ActiveGenericDialog::MountQuestion) => {
                // Escape or the scrim: no choice is made for the user.
                if let Some(asked) = self.model.mount_question.take() {
                    asked.answer(network::MountAnswer::Decline);
                }
            }
            // An unlock under way finishes in the dialog.
            Some(ActiveGenericDialog::Unlock) => {
                if !self.model.unlock.as_ref().is_some_and(|state| state.busy) {
                    self.model.unlock = None;
                }
            }
            Some(ActiveGenericDialog::Location) => return self.close_location_dialog(),
            // A format or sign-in in progress continues; its outcome arrives
            // as a notice.
            Some(ActiveGenericDialog::Format) => {
                self.model.format = None;
            }
            Some(ActiveGenericDialog::Cloud) => {
                self.model.cloud = None;
            }
            Some(ActiveGenericDialog::FolderColor) => {
                self.model.folder_color_dialog = None;
            }
        }
        Task::none()
    }

    pub(crate) fn tail_dialog_event_2(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DialogEvent(ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious) = message
        else {
            unreachable!("misrouted message in tail_dialog_event_2");
        };
        Task::none()
    }

    pub(crate) fn tail_open_shortcuts(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenShortcuts = message else {
            unreachable!("misrouted message in tail_open_shortcuts");
        };
        // F1 under another dialog would queue this one invisibly behind it.
        if self.modal_owns_keyboard() {
            return Task::none();
        }
        self.model.shortcuts_open = true;
        Task::none()
    }

    pub(crate) fn tail_close_shortcuts(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseShortcuts = message else {
            unreachable!("misrouted message in tail_close_shortcuts");
        };
        self.model.shortcuts_open = false;
        Task::none()
    }

    pub(crate) fn tail_toast(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Toast(event) = message else {
            unreachable!("misrouted message in tail_toast");
        };
        if matches!(event, ToastEvent::Tick(_)) {
            // Disarm so the tail reconcile re-arms only the remaining dwell.
            self.toast_expiry = ToastExpirySchedule::default();
        }
        self.toasts.update(event);
        Task::none()
    }

    pub(crate) fn tail_refresh(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Refresh = message else {
            unreachable!("misrouted message in tail_refresh");
        };
        self.arm_dir_watch();
        Task::batch([self.reload(), self.main_window_scope()])
    }

    pub(crate) fn tail_tree_watch(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TreeWatch(deliveries) = message else {
            unreachable!("misrouted message in tail_tree_watch");
        };
        self.consume_watch_deliveries(deliveries)
    }

    pub(crate) fn tail_watch_armed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::WatchArmed {
            generation,
            outcome,
        } = message
        else {
            unreachable!("misrouted message in tail_watch_armed");
        };
        self.consume_watch_armed(generation, outcome)
    }

    pub(crate) fn tail_shell_shortcut(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShellShortcut(inner) = message else {
            unreachable!("misrouted message in tail_shell_shortcut");
        };
        // Modal state is unavailable to the stateless event classifier.
        let scope = enzui_core::action::KeyRouteScope::new(false, self.modal_owns_keyboard());
        if enzui_core::action::KeyRouteScope::is_global(&scope) {
            self.update(*inner)
        } else {
            Task::none()
        }
    }

    pub(crate) fn tail_undo_last_operation(&mut self, message: Msg) -> Task<Msg> {
        let Msg::UndoLastOperation = message else {
            unreachable!("misrouted message in tail_undo_last_operation");
        };
        self.begin_undo()
    }

    pub(crate) fn tail_redo_last_undone(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RedoLastUndone = message else {
            unreachable!("misrouted message in tail_redo_last_undone");
        };
        self.begin_redo()
    }

    pub(crate) fn tail_undo_operation_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::UndoOperationFinished {
            attempt,
            entry,
            activity,
            result,
        } = message
        else {
            unreachable!("misrouted message in tail_undo_operation_finished");
        };
        self.finish_undo(attempt, *entry, activity, result)
    }

    pub(crate) fn tail_redo_operation_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RedoOperationFinished {
            attempt,
            entry,
            activity,
            result,
        } = message
        else {
            unreachable!("misrouted message in tail_redo_operation_finished");
        };
        self.finish_redo(attempt, *entry, activity, result)
    }

    pub(crate) fn tail_resolve_journal_conflict(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ResolveJournalConflict {
            entry_id,
            entry_generation,
            choice,
        } = message
        else {
            unreachable!("misrouted message in tail_resolve_journal_conflict");
        };
        self.resolve_journal_conflict(entry_id, entry_generation, choice)
    }
}
