// Clipboard, paste, and conflict reducer arms.

use super::*;
use enzui_core::operation_activity::ActivityProgress;
use iced::futures::{SinkExt, StreamExt};

impl App {
    pub(crate) fn reduce_cut_selection(&mut self, message: Msg) -> Task<Msg> {
        let (Msg::CutSelection | Msg::CopySelection) = message else {
            unreachable!("misrouted message in reduce_cut_selection");
        };
        self.begin_file_copy(matches!(message, Msg::CopySelection))
    }

    pub(crate) fn reduce_paste_shortcut(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PasteShortcut = message else {
            unreachable!("misrouted message in reduce_paste_shortcut");
        };
        if self.model.screen == Screen::Downloads
            && self.model.features.enabled(features::FeatureId::Downloads)
        {
            iced::clipboard::read_text().map(Msg::DownloadLinkPasted)
        } else {
            self.update(Msg::PasteHere)
        }
    }

    pub(crate) fn reduce_download_link_pasted(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadLinkPasted(result) = message else {
            unreachable!("misrouted message in reduce_download_link_pasted");
        };
        let Ok(text) = result else {
            return Task::none();
        };
        let url = text.trim();
        if downloads::is_enqueueable(url) && self.model.downloads.can_add() {
            let mut add = downloads::AddDownloadState::new(
                self.model.home.join("Downloads"),
                self.model.downloads.default_download_limit_kib(),
            );
            url.clone_into(&mut add.url);
            self.model.downloads.add = Some(add);
        }
        Task::none()
    }

    pub(crate) fn reduce_paste_here(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PasteHere = message else {
            unreachable!("misrouted message in reduce_paste_here");
        };
        self.begin_file_paste(false)
    }

    pub(crate) fn reduce_file_clipboard_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::FileClipboardFinished(receipt) = message else {
            unreachable!("misrouted message in reduce_file_clipboard_finished");
        };
        self.file_clipboard_finished(receipt)
    }

    pub(crate) fn reduce_copy_preview_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CopyPreviewSelection(text) = message else {
            unreachable!("misrouted message in reduce_copy_preview_selection");
        };
        iced::clipboard::write(text).discard()
    }

    pub(crate) fn reduce_file_item_dropped(&mut self, message: Msg) -> Task<Msg> {
        let Msg::FileItemDropped(req) = message else {
            unreachable!("misrouted message in reduce_file_item_dropped");
        };
        self.route_item_drop(req)
    }

    pub(crate) fn reduce_begin_move_selection_to_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginMoveSelectionToFolder = message else {
            unreachable!("misrouted message in reduce_begin_move_selection_to_folder");
        };
        self.begin_pick_destination(false)
    }

    pub(crate) fn reduce_begin_copy_selection_to_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginCopySelectionToFolder = message else {
            unreachable!("misrouted message in reduce_begin_copy_selection_to_folder");
        };
        self.begin_pick_destination(true)
    }

    pub(crate) fn reduce_move_selection_to_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MoveSelectionToFolder(dest) = message else {
            unreachable!("misrouted message in reduce_move_selection_to_folder");
        };
        match drag_drop::keyboard_item_drop(&self.model, &dest, false) {
            Some(req) => self.route_item_drop(req),
            None => {
                self.post_status(
                    crate::i18n::tr("Nothing selected to move"),
                    ToastTone::Warning,
                );
                Task::none()
            }
        }
    }

    pub(crate) fn reduce_copy_selection_to_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CopySelectionToFolder(dest) = message else {
            unreachable!("misrouted message in reduce_copy_selection_to_folder");
        };
        match drag_drop::keyboard_item_drop(&self.model, &dest, true) {
            Some(req) => self.route_item_drop(req),
            None => {
                self.post_status(
                    crate::i18n::tr("Nothing selected to copy"),
                    ToastTone::Warning,
                );
                Task::none()
            }
        }
    }

    pub(crate) fn reduce_open_context_menu(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenContextMenu(path, at) = message else {
            unreachable!("misrouted message in reduce_open_context_menu");
        };
        let target = self
            .model
            .projected_entries()
            .into_iter()
            .find(|entry| entry.path == path)
            .map(|entry| {
                (
                    entry.path.clone(),
                    matches!(entry.kind, model::EntryKind::Dir),
                )
            });
        self.model.context_menu = if let Some((target_path, is_dir)) = target {
            // A press inside a multi-selection keeps it; a press on an outsider
            // selects just that row, as `effective_selection_count` assumes.
            if !self.model.is_selected(&target_path) {
                self.model.select_only(target_path.clone());
            }
            let rows = context_menu::menu_model(&self.model, &target_path, is_dir);
            let invoker = (self.model.view_mode == model::ViewMode::List)
                .then(|| context_menu::list_row_id(&target_path));
            context_menu::OpenContextMenu::new_with_restore(path, at, rows, invoker)
        } else if self
            .model
            .trash_entries
            .iter()
            .any(|entry| entry.trashed_path == path)
        {
            context_menu::OpenContextMenu::new_with_restore(
                path.clone(),
                at,
                context_menu::trash_menu_model(),
                Some(context_menu::trash_row_id(&path)),
            )
        } else {
            None
        };
        Task::perform(crate::history::refresh(), |r| {
            Msg::History(crate::history::HistoryMsg::Refreshed(r))
        })
    }

    pub(crate) fn reduce_open_sidebar_context_menu(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenSidebarContextMenu(path, at) = message else {
            unreachable!("misrouted message in reduce_open_sidebar_context_menu");
        };
        self.model.context_menu = self
            .model
            .is_pinned(&path)
            .then(|| {
                context_menu::OpenContextMenu::sidebar(path, at, context_menu::sidebar_menu_model())
            })
            .flatten();
        Task::none()
    }

    pub(crate) fn reduce_open_background_context_menu(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenBackgroundContextMenu(at) = message else {
            unreachable!("misrouted message in reduce_open_background_context_menu");
        };
        let rows = context_menu::background_menu_model(&self.model);
        self.model.context_menu =
            context_menu::OpenContextMenu::background(self.model.cwd.clone(), at, rows);
        // Rescan the Templates folder; the open menu picks it up.
        Task::batch([
            Task::perform(
                crate::templates::scan_task(self.model.home.clone()),
                Msg::TemplatesScanned,
            ),
            Task::perform(crate::history::refresh(), |r| {
                Msg::History(crate::history::HistoryMsg::Refreshed(r))
            }),
        ])
    }

    pub(crate) fn reduce_context_menu_event(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ContextMenuEvent(generation, event) = message else {
            unreachable!("misrouted message in reduce_context_menu_event");
        };
        let Some(open) = self.model.context_menu.as_mut() else {
            return Task::none();
        };
        if open.generation != generation {
            return Task::none();
        }
        let origin = open.origin;
        let (chosen, focus) = open.menu.update::<Msg>(&event);
        if !open.menu.is_open() {
            self.model.context_menu = None;
        }
        let Some((path, command)) = chosen else {
            return focus;
        };
        let Some(items) = context_menu::items_for_target(&self.model, &path, origin) else {
            return focus;
        };
        if !context_menu::command_allowed(&items, &command) {
            return focus;
        }
        use context_menu::{Command, ContextMenuOrigin};
        let message = match command {
            Command::Backup(action) => {
                use crate::history::HistoryMsg;
                use context_menu::BackupAction;
                Some(Msg::History(match action {
                    BackupAction::Versions => HistoryMsg::ShowVersions(path),
                    BackupAction::Browse => HistoryMsg::Browse(path),
                    BackupAction::Exclude => HistoryMsg::Exclude(path, true),
                    BackupAction::Include => HistoryMsg::Exclude(path, false),
                    BackupAction::BackUpNow => HistoryMsg::BackUpNow,
                }))
            }
            Command::Background(row) => Some(Msg::BackgroundContextRowActivated(row)),
            Command::Template(template) => Some(Msg::NewFromTemplate(template)),
            Command::Extension { extension, action } => Some(Msg::RunExtension {
                extension,
                action,
                path,
                background: origin == ContextMenuOrigin::Background,
            }),
            Command::Location(action) => path
                .to_str()
                .map(|id| Msg::LocationAction(id.to_owned(), action)),
            Command::Row(_) if origin == ContextMenuOrigin::Sidebar => {
                Some(Msg::UnpinFromSidebar(path))
            }
            Command::Row(row) => self
                .model
                .entry_for_path(&path)
                .map(|entry| Msg::ContextRowActivated(path.clone(), entry.kind, row))
                .or_else(|| {
                    self.model
                        .trash_entries
                        .iter()
                        .any(|entry| entry.trashed_path == path)
                        .then(|| Msg::TrashContextRowActivated(path.clone(), row))
                }),
        };
        match message {
            Some(message) => Task::batch([focus, self.update(message)]),
            None => focus,
        }
    }

    pub(crate) fn reduce_background_context_row_activated(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BackgroundContextRowActivated(row) = message else {
            unreachable!("misrouted message in reduce_background_context_row_activated");
        };
        self.model.context_menu = None;
        self.update(row.message())
    }

    pub(crate) fn reduce_context_row_activated(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ContextRowActivated(path, kind, row) = message else {
            unreachable!("misrouted message in reduce_context_row_activated");
        };
        // Closed first, so the popup does not cover the action's result.
        self.model.context_menu = None;

        // Clipboard verbs read `model.selection`, so an unselected right-clicked
        // row becomes the selection; a row inside the selection keeps the group.
        if !self.model.is_selected(&path) {
            self.model.select_only(path.clone());
        }

        self.update(context_menu::message_for(row, &path, kind))
    }

    pub(crate) fn reduce_begin_paste(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginPaste {
            sources,
            dest_dir,
            copy,
        } = message
        else {
            unreachable!("misrouted message in reduce_begin_paste");
        };
        let count = sources.len();
        let kind = if copy {
            ActivityKind::Copy
        } else {
            ActivityKind::Move
        };
        let activity = self.start_file_activity(
            kind,
            crate::i18n::count(
                if copy {
                    "files-copying-items"
                } else {
                    "files-moving-items"
                },
                count as u64,
            ),
            count,
        );
        if self.native_drop.lease.is_some() && self.native_drop.activity.is_none() {
            self.native_drop.activity = Some((activity, count));
        }
        if copy {
            let _ = self.activity_center.progress(
                activity.key,
                activity.generation,
                ActivityProgress::Indeterminate,
            );
        }
        self.shell_activity.start(activity, kind, count);
        Task::perform(plan_paste(sources, dest_dir.clone(), copy), move |plan| {
            Msg::PastePlanned {
                activity,
                dest_dir: dest_dir.clone(),
                copy,
                plan: Box::new(plan),
            }
        })
    }

    pub(crate) fn reduce_paste_planned(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PastePlanned {
            activity,
            dest_dir,
            copy,
            plan,
        } = message
        else {
            unreachable!("misrouted message in reduce_paste_planned");
        };
        if plan.is_empty() {
            self.finish_file_activity(
                activity,
                0,
                TerminalOutcome::Completed,
                crate::i18n::tr("Nothing to paste"),
            );
            self.post_status(crate::i18n::tr("Nothing to paste"), ToastTone::Warning);
            return Task::none();
        }
        let mode = if copy {
            conflict::ClipMode::Copy
        } else {
            conflict::ClipMode::Cut
        };
        if plan.copy_folders.is_empty() {
            if copy {
                let total = (plan.clear.len() + plan.conflicts.len() + plan.refused.len()) as u64;
                let _ = self.activity_center.progress(
                    activity.key,
                    activity.generation,
                    ActivityProgress::Items {
                        done: 0,
                        total: Some(total),
                    },
                );
            }
            return Task::perform(
                open_paste_queue(
                    dest_dir,
                    mode,
                    *plan,
                    self.shell_activity.transfer(activity),
                    |_| {},
                ),
                move |queue| Msg::PasteExecuted { activity, queue },
            );
        }

        self.model
            .pending_copy_folders
            .extend(
                plan.copy_folders
                    .iter()
                    .map(|path| model::PendingCopyFolder {
                        path: path.clone(),
                        activity: Some(activity),
                    }),
            );
        let transfer = self.shell_activity.transfer(activity);
        Task::run(
            iced::stream::channel(2, async move |mut output| {
                let (mut progress, mut snapshots) = iced::futures::channel::mpsc::channel(2);
                let work = open_paste_queue(dest_dir, mode, *plan, transfer, move |fraction| {
                    let _ = progress.try_send(fraction);
                });
                let publish = async {
                    while let Some(fraction) = snapshots.next().await {
                        if output
                            .send(Msg::PasteProgress(activity, fraction))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                };
                let (queue, ()) = iced::futures::future::join(work, publish).await;
                let _ = output.send(Msg::PasteExecuted { activity, queue }).await;
            }),
            |message| message,
        )
    }

    pub(crate) fn reduce_paste_executed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PasteExecuted { activity, queue } = message else {
            unreachable!("misrouted message in reduce_paste_executed");
        };
        let mut refresh_published_folders = false;
        self.model.pending_copy_folders.retain_mut(|folder| {
            if folder.activity != Some(activity) {
                return true;
            }
            let published = self.model.cwd.as_path() == queue.dest_dir
                && queue
                    .resolved
                    .iter()
                    .any(|item| item.dst == folder.path && item.outcome.changed_destination());
            if published {
                folder.activity = None;
                refresh_published_folders = true;
            }
            published
        });
        self.publish_file_activity_items(activity, &queue);
        if queue.is_open() {
            self.set_file_activity_phase(activity, LivePhase::Waiting);
            self.conflict_activity = Some(activity);
            self.model.conflicts = Some(*queue);
            return if refresh_published_folders {
                self.reload()
            } else {
                Task::none()
            };
        }
        // No decision was needed: the whole paste is already done.
        let summary = queue.summary();
        let terminal = if queue
            .resolved
            .iter()
            .any(|item| matches!(item.outcome, conflict::ItemOutcome::Failed(_)))
        {
            TerminalOutcome::Failed
        } else {
            TerminalOutcome::Completed
        };
        self.finish_file_activity(activity, queue.completed_items(), terminal, summary.clone());
        // Summary first, admission last so any warning trails.
        self.post_outcome(summary, terminal);
        self.admit_moves_from_queue(&queue);
        if queue.changed_anything() {
            self.focus_pasted_item(&queue);
            self.reload()
        } else {
            // A declined Copy wrote nothing, so nothing is reloaded.
            Task::none()
        }
    }

    fn focus_pasted_item(&mut self, queue: &conflict::ConflictQueue) {
        self.model.focus_after_load = queue
            .resolved
            .iter()
            .find(|item| {
                item.outcome.changed_destination()
                    && item.dst.parent() == Some(self.model.cwd.as_path())
            })
            .map(|item| item.dst.clone());
    }

    pub(crate) fn reduce_set_apply_to_all_conflicts(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetApplyToAllConflicts(on) = message else {
            unreachable!("misrouted message in reduce_set_apply_to_all_conflicts");
        };
        if let Some(q) = self.model.conflicts.as_mut() {
            q.set_apply_to_all(on);
        }
        Task::none()
    }

    pub(crate) fn reduce_pause_conflicts(&mut self, message: Msg) -> Task<Msg> {
        if self.model.conflicts_busy {
            return Task::none();
        }
        let Msg::PauseConflicts = message else {
            unreachable!("misrouted message in reduce_pause_conflicts");
        };
        if let Some(q) = self.model.conflicts.as_mut() {
            q.pause();
        }
        if let Some(activity) = self.conflict_activity {
            self.set_file_activity_phase(activity, LivePhase::Paused);
        }
        Task::none()
    }

    pub(crate) fn reduce_resume_conflicts(&mut self, message: Msg) -> Task<Msg> {
        if self.model.conflicts_busy {
            return Task::none();
        }
        let Msg::ResumeConflicts = message else {
            unreachable!("misrouted message in reduce_resume_conflicts");
        };
        if let Some(q) = self.model.conflicts.as_mut() {
            q.resume();
        }
        if let Some(activity) = self.conflict_activity {
            self.set_file_activity_phase(activity, LivePhase::Waiting);
        }
        Task::none()
    }

    pub(crate) fn reduce_cancel_conflicts(&mut self, message: Msg) -> Task<Msg> {
        if self.model.conflicts_busy {
            return Task::none();
        }
        let Msg::CancelConflicts = message else {
            unreachable!("misrouted message in reduce_cancel_conflicts");
        };
        let restoring = self.model.pending_trash_restore.take().is_some();
        let summary = self.model.conflicts.as_mut().map(|q| {
            q.cancel();
            if restoring {
                crate::i18n::tr("Restore cancelled").to_owned()
            } else {
                q.summary()
            }
        });
        if let Some(activity) = self.conflict_activity.take() {
            let completed = self
                .model
                .conflicts
                .as_ref()
                .map_or(0, super::conflict::ConflictQueue::completed_items);
            self.finish_file_activity(
                activity,
                completed,
                TerminalOutcome::Cancelled,
                summary
                    .clone()
                    .unwrap_or_else(|| crate::i18n::tr("Operation cancelled").into()),
            );
        }
        self.model.conflicts = None;
        if let Some(summary) = summary {
            self.post_status(summary, ToastTone::Info);
        }
        Task::none()
    }

    pub(crate) fn reduce_resolve_conflict(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ResolveConflict(choice) = message else {
            unreachable!("misrouted message in reduce_resolve_conflict");
        };
        let Some(queue) = self.model.conflicts.clone() else {
            return Task::none();
        };
        if queue.paused || self.model.conflicts_busy {
            return Task::none();
        }
        let Some(activity) = self.conflict_activity else {
            return Task::none();
        };
        self.model.conflicts_busy = true;
        self.set_file_activity_phase(activity, LivePhase::Running);
        // A `Merge` walks a tree, so the decision is applied off-thread.
        Task::perform(
            resolve_conflict(queue, choice, self.shell_activity.transfer(activity)),
            move |queue| Msg::ConflictBatchFinished { activity, queue },
        )
    }

    pub(crate) fn reduce_conflict_batch_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ConflictBatchFinished { activity, queue } = message else {
            unreachable!("misrouted message in reduce_conflict_batch_finished");
        };
        self.model.conflicts_busy = false;
        self.publish_file_activity_items(activity, &queue);
        if queue.is_open() {
            self.set_file_activity_phase(activity, LivePhase::Waiting);
            self.conflict_activity = Some(activity);
            self.model.conflicts = Some(*queue);
            Task::none()
        } else if let Some(entry) = self.model.pending_trash_restore.take() {
            let changed = queue.changed_anything();
            self.model.conflicts = None;
            self.conflict_activity = None;
            if changed {
                Task::perform(finalize_conflict_restore(entry.clone()), move |result| {
                    Msg::TrashRestoreFinished {
                        activity,
                        entry,
                        result,
                    }
                })
            } else {
                let (terminal, summary) = if queue
                    .resolved
                    .iter()
                    .any(|item| matches!(item.outcome, conflict::ItemOutcome::Failed(_)))
                {
                    (
                        TerminalOutcome::Failed,
                        crate::i18n::format(
                            "files-could-not-restore-c161af19",
                            &[("arg0", (entry.name))],
                        ),
                    )
                } else {
                    (
                        TerminalOutcome::Cancelled,
                        crate::i18n::tr("Restore cancelled").to_owned(),
                    )
                };
                self.finish_file_activity(
                    activity,
                    queue.completed_items(),
                    terminal,
                    summary.clone(),
                );
                self.post_outcome(summary, terminal);
                Task::none()
            }
        } else {
            let summary = queue.summary();
            let terminal = if queue.cancelled {
                TerminalOutcome::Cancelled
            } else if queue
                .resolved
                .iter()
                .any(|item| matches!(item.outcome, conflict::ItemOutcome::Failed(_)))
            {
                TerminalOutcome::Failed
            } else {
                TerminalOutcome::Completed
            };
            self.finish_file_activity(activity, queue.completed_items(), terminal, summary.clone());
            self.conflict_activity = None;
            self.post_outcome(summary, terminal);
            self.admit_moves_from_queue(&queue);
            let changed = queue.changed_anything();
            self.model.conflicts = None;
            if changed {
                self.focus_pasted_item(&queue);
                self.reload()
            } else {
                Task::none()
            }
        }
    }
}
