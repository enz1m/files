// Trash and create reducer arms.

use super::*;

impl App {
    fn open_delete_confirm_focus(
        &mut self,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> Task<Msg> {
        let (focus, operation) = enzui_widgets::modal_host::ModalFocusLifecycle::open(
            crate::dialog::DELETE_CONFIRM_SCOPE,
            invoker,
        );
        self.delete_confirm_focus = Some(focus);
        iced::advanced::widget::operate(operation)
    }

    pub(crate) fn reduce_trash_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashSelection = message else {
            unreachable!("misrouted message in reduce_trash_selection");
        };
        let sources = self.model.clipboard_sources();
        if sources.is_empty() {
            self.post_status(
                crate::i18n::tr("Nothing selected to trash"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        if crate::services::is_virtual(&self.model.cwd) {
            self.post_status(
                crate::i18n::tr("Cannot trash from a virtual folder"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        // Same-device sources need the home-Trash authority; other devices
        // use mount-local `.Trash-<uid>`, admitted per source at task time,
        // so a refused home authority does not refuse the whole batch.
        let count = sources.len();
        let activity = self.start_file_activity(
            ActivityKind::Trash,
            crate::i18n::format(
                "files-moving-count-to-trash-150e0511",
                &[("count", (count).to_string())],
            ),
            count,
        );
        Task::perform(
            trash_selection(sources, self.home_data_dir.clone()),
            move |outcome| Msg::TrashFinished { activity, outcome },
        )
    }

    pub(crate) fn reduce_trash_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashFinished { activity, outcome } = message else {
            unreachable!("misrouted message in reduce_trash_finished");
        };
        let summary = outcome.summary();
        let terminal = if outcome.failed.is_empty() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        };
        self.finish_file_activity(activity, outcome.trashed.len(), terminal, summary.clone());
        let now_ms = journal::now_ms();
        let mut tally = JournalAdmissionTally::default();
        for (origin, trashed_path) in &outcome.trashed {
            let info_path = trashinfo_path_of(trashed_path);
            // A capture failure counts as unavailable Undo rather than
            // synthesising an entry.
            let Ok(trashed_inode) = journal::InodeIdent::of(trashed_path) else {
                tally.note_capture_unavailable();
                continue;
            };
            let Ok((info_inode, info_bytes)) =
                crate::trash::capture_admitted_metadata_twin(&info_path)
            else {
                tally.note_capture_unavailable();
                continue;
            };
            tally.observe(self.model.journal.record(
                journal::OpRecord::Trash {
                    origin: origin.clone(),
                    trashed_path: trashed_path.clone(),
                    info_path,
                    trashed_inode,
                    info_inode,
                    info_bytes,
                },
                now_ms,
            ));
        }
        self.sweep_journal_prompts();
        // The Undo-availability warning goes last so the trailing status names
        // the gap.
        self.post_outcome(summary, terminal);
        if let Some(msg) = tally.warning_for(crate::i18n::tr("Trash")) {
            self.post_status(msg, ToastTone::Warning);
        }
        for (origin, _) in &outcome.trashed {
            self.model.selection.remove(origin);
        }
        if !outcome.trashed.is_empty() {
            return self.reload();
        }
        Task::none()
    }

    pub(crate) fn reduce_trash_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashLoaded {
            generation,
            listing,
        } = message
        else {
            unreachable!("misrouted message in reduce_trash_loaded");
        };
        // A scan superseded by a leave / refresh / restore /
        // delete / empty cannot land over the freshest listing.
        if generation != self.trash_load_generation {
            return Task::none();
        }
        // Classified before `listing.entries` moves out.
        let incomplete = classify_trash_incomplete(&listing);
        self.model.trash_entries = listing.entries;
        self.model.trash_payloads = listing.payloads;
        self.model.trash_listing_incomplete = incomplete;
        if let Some(msg) = trash_listing_warning(self.model.trash_listing_incomplete.as_ref()) {
            self.post_status(msg, ToastTone::Warning);
        }
        Task::none()
    }

    pub(crate) fn reduce_select_trash_entry(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectTrashEntry {
            path,
            command,
            shift,
        } = message
        else {
            unreachable!("misrouted message in reduce_select_trash_entry");
        };
        let modifiers = if command {
            enzui_core::list_selection::ClickModifiers::TOGGLE
        } else if shift {
            enzui_core::list_selection::ClickModifiers::RANGE
        } else {
            enzui_core::list_selection::ClickModifiers::PLAIN
        };
        self.model.click_trash_selection(path, modifiers);
        Task::none()
    }

    pub(crate) fn reduce_restore_from_trash(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RestoreFromTrash(trashed_path) = message else {
            unreachable!("misrouted message in reduce_restore_from_trash");
        };
        let Some(entry) = self
            .model
            .trash_entries
            .iter()
            .find(|entry| entry.trashed_path == trashed_path)
            .cloned()
        else {
            self.post_status(
                crate::i18n::tr("Could not restore: the Trash entry no longer exists"),
                ToastTone::Error,
            );
            return Task::none();
        };
        // The retained root fd authorizes this row only; rows of a mixed
        // listing without authority are refused individually.
        if !App::authority_ok_for(&entry) {
            self.post_status(
                crate::i18n::tr("Refused: this Trash entry's root is not admitted"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        let activity = self.start_file_activity(
            ActivityKind::Other("Restoring"),
            crate::i18n::format(
                "files-restoring-be79eef8",
                &[("arg0", (entry.name).clone())],
            ),
            1,
        );
        Task::perform(plan_trash_restore(entry.clone()), move |plan| {
            Msg::TrashRestorePlanned {
                activity,
                entry,
                plan: plan.map(Box::new),
            }
        })
    }

    pub(crate) fn reduce_trash_restore_planned(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashRestorePlanned {
            activity,
            entry,
            plan,
        } = message
        else {
            unreachable!("misrouted message in reduce_trash_restore_planned");
        };
        let plan = match plan {
            Ok(plan) => *plan,
            Err(message) => {
                self.finish_file_activity(activity, 0, TerminalOutcome::Failed, message.clone());
                self.post_status(message, ToastTone::Error);
                return Task::none();
            }
        };
        if !plan.conflicts.is_empty() {
            let parent = entry
                .original_path
                .parent()
                .map_or_else(|| self.model.home.clone(), Path::to_path_buf);
            self.model.pending_trash_restore = Some(entry);
            self.set_file_activity_phase(activity, LivePhase::Waiting);
            self.conflict_activity = Some(activity);
            self.model.conflicts = Some(conflict::ConflictQueue::new(
                parent,
                conflict::ClipMode::Cut,
                plan.conflicts,
            ));
            return Task::none();
        }
        Task::perform(restore_trash_entry(entry.clone()), move |result| {
            Msg::TrashRestoreFinished {
                activity,
                entry,
                result,
            }
        })
    }

    pub(crate) fn reduce_trash_restore_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashRestoreFinished {
            activity,
            entry,
            result,
        } = message
        else {
            unreachable!("misrouted message in reduce_trash_restore_finished");
        };
        let (terminal, summary) = match result {
            Ok(()) => (
                TerminalOutcome::Completed,
                crate::i18n::format("files-restored-342a1f32", &[("arg0", (entry.name))]),
            ),
            Err(message) => (TerminalOutcome::Failed, message),
        };
        self.finish_file_activity(
            activity,
            usize::from(terminal == TerminalOutcome::Completed),
            terminal,
            summary.clone(),
        );
        self.post_outcome(summary, terminal);
        self.dispatch_trash_load()
    }

    pub(crate) fn reduce_ask_delete_trash_entry(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AskDeleteTrashEntry(trashed_path) = message else {
            unreachable!("misrouted message in reduce_ask_delete_trash_entry");
        };
        let entries: Vec<_> = self
            .model
            .trash_entries
            .iter()
            .filter(|entry| entry.trashed_path == trashed_path)
            .cloned()
            .collect();
        let entries: Vec<_> = entries.into_iter().filter(App::authority_ok_for).collect();
        if entries.is_empty() {
            self.post_status(
                crate::i18n::tr("Could not delete: the Trash entry no longer exists"),
                ToastTone::Error,
            );
            return Task::none();
        }
        self.model.delete_confirm_state = enzui_core::confirm::ConfirmState::new(None);
        self.model.pending_permanent_delete = Some(trash::DeleteRequest::Trash {
            entries,
            empty_all: false,
        });
        self.open_delete_confirm_focus(Some(context_menu::trash_row_id(&trashed_path)))
    }

    pub(crate) fn reduce_trash_context_row_activated(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashContextRowActivated(path, row) = message else {
            unreachable!("misrouted message in reduce_trash_context_row_activated");
        };
        self.model.context_menu = None;
        match context_menu::message_for_trash(row, &path) {
            Some(message) => self.update(message),
            None => Task::none(),
        }
    }

    pub(crate) fn reduce_empty_trash(&mut self, message: Msg) -> Task<Msg> {
        let Msg::EmptyTrash = message else {
            unreachable!("misrouted message in reduce_empty_trash");
        };
        if self.model.trash_entries.is_empty() {
            self.post_status(
                crate::i18n::tr("Trash is already empty"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        // If every row lacks authority, one toast replaces an empty dialog.
        let admitted: Vec<_> = self
            .model
            .trash_entries
            .iter()
            .filter(|entry| App::authority_ok_for(entry))
            .cloned()
            .collect();
        if admitted.is_empty() {
            self.post_status(
                crate::i18n::tr("Files couldn't safely open the Trash folder for these items."),
                ToastTone::Warning,
            );
            return Task::none();
        }
        self.model.delete_confirm_state = enzui_core::confirm::ConfirmState::new(None);
        self.model.pending_permanent_delete = Some(trash::DeleteRequest::Trash {
            entries: admitted,
            empty_all: true,
        });
        self.open_delete_confirm_focus(Some(iced::advanced::widget::Id::new("files-empty-trash")))
    }

    pub(crate) fn reduce_trash_emptied(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TrashEmptied { activity, outcome } = message else {
            unreachable!("misrouted message in reduce_trash_emptied");
        };
        let summary = outcome.summary();
        let terminal = if outcome.failed.is_empty() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        };
        self.finish_file_activity(activity, outcome.deleted.len(), terminal, summary.clone());
        self.post_outcome(summary, terminal);
        // Reload the listing after delete-one or Empty Trash so no
        // stale row survives either confirmed destructive action.
        self.dispatch_trash_load()
    }

    pub(crate) fn reduce_ask_permanent_delete(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AskPermanentDelete = message else {
            unreachable!("misrouted message in reduce_ask_permanent_delete");
        };
        // Snapshotted now so later selection changes cannot alter what is
        // confirmed or deleted.
        let items = self.model.delete_items();
        if items.is_empty() {
            self.post_status(
                crate::i18n::tr("Nothing selected to delete"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        if crate::services::is_virtual(&self.model.cwd) {
            self.post_status(
                crate::i18n::tr("Cannot delete from a virtual folder"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        // Files in the home can also be removed from backups and earlier
        // versions (it asks for the person's password).
        // The service takes at most 256 paths in one request.
        let in_history = self.model.history_available
            && items.len() <= crate::history::FORGET_LIMIT
            && items
                .iter()
                .all(|item| crate::history::covers(&self.model.home, &item.path));
        self.model.delete_confirm_state =
            enzui_core::confirm::ConfirmState::new(in_history.then(|| {
                enzui_core::confirm::ConfirmCheckbox::new(crate::i18n::tr(
                    "Also remove from backups and earlier versions",
                ))
            }));
        self.model.pending_permanent_delete = Some(trash::DeleteRequest::Files(items));
        let invoker = self
            .model
            .selection
            .active()
            .map(|path| match self.model.view_mode {
                model::ViewMode::List => context_menu::list_row_id(path),
                model::ViewMode::Grid => context_menu::grid_tile_id(path),
            });
        self.open_delete_confirm_focus(invoker)
    }

    pub(crate) fn reduce_cancel_permanent_delete(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelPermanentDelete = message else {
            unreachable!("misrouted message in reduce_cancel_permanent_delete");
        };
        self.model.pending_permanent_delete = None;
        let focus = self.delete_confirm_focus.take().map(|focus| {
            focus
                .reduce(enzui_widgets::modal_host::ModalHostEvent::Dismissed(
                    enzui_core::modal::DismissCause::CancelAction,
                ))
                .into_task()
        });
        focus.unwrap_or_else(Task::none)
    }

    pub(crate) fn reduce_confirm_permanent_delete(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ConfirmPermanentDelete = message else {
            unreachable!("misrouted message in reduce_confirm_permanent_delete");
        };
        let forget_too = self.model.delete_confirm_state.is_checked();
        let Some(request) = self.model.pending_permanent_delete.take() else {
            return Task::none();
        };
        self.delete_confirm_focus = None;
        match request {
            trash::DeleteRequest::Files(items) => {
                let count = items.len();
                let forget = if forget_too {
                    let paths: Vec<PathBuf> = items.iter().map(|item| item.path.clone()).collect();
                    Task::perform(crate::history::forget(paths), |r| {
                        Msg::History(crate::history::HistoryMsg::Forgotten(r))
                    })
                } else {
                    Task::none()
                };
                let activity = self.start_file_activity(
                    ActivityKind::Delete,
                    crate::i18n::format(
                        "files-deleting-count-8422f846",
                        &[("count", (count).to_string())],
                    ),
                    count,
                );
                Task::batch([
                    Task::perform(
                        // Deletion consumes only the captured paths, in the
                        // captured order — the confirmed set, never a rescan.
                        permanent_delete_selection(
                            items.into_iter().map(|item| item.path).collect(),
                        ),
                        move |outcome| Msg::PermanentDeleteFinished { activity, outcome },
                    ),
                    forget,
                ])
            }
            trash::DeleteRequest::Trash { entries, .. } => {
                // Revalidate each row between confirmation and dispatch; the
                // unlink then runs `unlinkat` on the pinned inode.
                let entries: Vec<_> = entries.into_iter().filter(App::authority_ok_for).collect();
                if entries.is_empty() {
                    self.post_status(
                        crate::i18n::tr("Refused: no admitted Trash roots to delete"),
                        ToastTone::Warning,
                    );
                    return Task::none();
                }
                let count = entries.len();
                let activity = self.start_file_activity(
                    ActivityKind::Delete,
                    crate::i18n::format(
                        "files-deleting-count-trash-dca38a5d",
                        &[("count", (count).to_string())],
                    ),
                    count,
                );
                Task::perform(delete_trash_selection(entries), move |outcome| {
                    Msg::TrashEmptied { activity, outcome }
                })
            }
        }
    }

    pub(crate) fn reduce_delete_confirm_event(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DeleteConfirmEvent(event) = message else {
            unreachable!("misrouted message in reduce_delete_confirm_event");
        };
        use enzui_widgets::confirm_dialog::ConfirmDialogEvent;
        if self.model.pending_permanent_delete.is_none() {
            return Task::none();
        }
        match event {
            // Dropping Tab would trap keyboard users at the first control.
            ConfirmDialogEvent::FocusNext | ConfirmDialogEvent::FocusPrevious => {
                if let Some(op) = event.scoped_focus_operation::<Msg>(
                    enzui_widgets::modal_host::presentation_scope_id(
                        crate::dialog::DELETE_CONFIRM_SCOPE,
                    ),
                ) {
                    return iced::advanced::widget::operate(op);
                }
                Task::none()
            }
            ConfirmDialogEvent::CheckboxToggled(next) => {
                self.model.delete_confirm_state.toggle(next);
                Task::none()
            }
            ConfirmDialogEvent::Resolved(cause) => {
                let confirmed =
                    enzui_core::confirm::ConfirmDecision::from_cause(cause).is_confirmed();
                if confirmed {
                    self.update(Msg::ConfirmPermanentDelete)
                } else {
                    self.update(Msg::CancelPermanentDelete)
                }
            }
        }
    }

    pub(crate) fn reduce_permanent_delete_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PermanentDeleteFinished { activity, outcome } = message else {
            unreachable!("misrouted message in reduce_permanent_delete_finished");
        };
        let summary = outcome.summary();
        let terminal = if outcome.failed.is_empty() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        };
        self.finish_file_activity(activity, outcome.deleted.len(), terminal, summary.clone());
        self.post_outcome(summary, terminal);
        for path in &outcome.deleted {
            self.model.selection.remove(path);
        }
        if !outcome.deleted.is_empty() {
            return self.reload();
        }
        Task::none()
    }

    pub(crate) fn reduce_new_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NewFolder = message else {
            unreachable!("misrouted message in reduce_new_folder");
        };
        let dir = self.model.cwd.clone();
        if crate::services::is_virtual(&dir) {
            self.post_status(
                crate::i18n::tr("Cannot create a folder here"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        // Off-thread: `read_dir` + `mkdir` on a busy filesystem.
        Task::perform(new_folder(dir), Msg::CreationFinished)
    }

    pub(crate) fn reduce_new_file(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NewFile = message else {
            unreachable!("misrouted message in reduce_new_file");
        };
        let dir = self.model.cwd.clone();
        if crate::services::is_virtual(&dir) {
            self.post_status(
                crate::i18n::tr("Cannot create a file here"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        Task::perform(new_file(dir), Msg::CreationFinished)
    }

    pub(crate) fn reduce_templates_scanned(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TemplatesScanned(templates) = message else {
            unreachable!("misrouted message in reduce_templates_scanned");
        };
        self.model.templates = templates;
        let items = self.model.context_menu.as_ref().and_then(|open| {
            (open.origin == context_menu::ContextMenuOrigin::Background)
                .then(|| context_menu::background_menu_model(&self.model))
        });
        if let (Some(open), Some(items)) = (self.model.context_menu.as_mut(), items) {
            open.refresh(items);
        }
        Task::none()
    }

    // Duplicate, Create Link and Paste as Link, off the UI thread.
    // Each item is its own attempt; one failure does not stop the rest.
    pub(crate) fn reduce_create_batch(&mut self, message: Msg) -> Task<Msg> {
        if matches!(message, Msg::LinkSelection) {
            if self.model.selection.is_empty() {
                return Task::none();
            }
            return self.picker.start(
                picker::PickerTarget::LinkSelection,
                &self.model,
                self.main_window,
            );
        }
        let (verb, jobs): (CreateVerb, Vec<(PathBuf, PathBuf)>) = match message {
            Msg::DuplicateSelection => (CreateVerb::Duplicate, self.selection_beside_itself()),
            Msg::LinkSources { sources, dest_dir } => (
                CreateVerb::Link,
                sources
                    .into_iter()
                    .map(|src| (src, dest_dir.clone()))
                    .collect(),
            ),
            _ => unreachable!("misrouted message in reduce_create_batch"),
        };
        if jobs.is_empty() {
            self.post_status(crate::i18n::tr("Nothing selected"), ToastTone::Warning);
            return Task::none();
        }
        if jobs
            .iter()
            .any(|(src, dir)| crate::services::is_virtual(src) || crate::services::is_virtual(dir))
        {
            self.post_status(
                crate::i18n::tr("That cannot be done in this location"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        Task::perform(
            crate::shell_work::run(move || {
                let mut created = Vec::new();
                let mut failed = Vec::new();
                for (src, dir) in &jobs {
                    let result = match verb {
                        CreateVerb::Duplicate => services::duplicate_entry(src),
                        CreateVerb::Link => services::create_link(dir, src),
                    };
                    match result {
                        Ok(path) => created.push(path),
                        Err(reason) => failed.push(reason),
                    }
                }
                (created, failed)
            }),
            move |(created, failed)| Msg::CreatedBatch {
                verb,
                created,
                failed,
            },
        )
    }

    fn selection_beside_itself(&self) -> Vec<(PathBuf, PathBuf)> {
        self.model
            .clipboard_sources()
            .into_iter()
            .filter_map(|src| Some((src.clone(), src.parent()?.to_path_buf())))
            .collect()
    }

    pub(crate) fn reduce_created_batch(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CreatedBatch {
            verb,
            created,
            failed,
        } = message
        else {
            unreachable!("misrouted message in reduce_created_batch");
        };
        let done = match (verb, created.as_slice()) {
            (_, []) => None,
            (CreateVerb::Duplicate, [one]) => Some(crate::i18n::format(
                "files-duplicated-as-037c8350",
                &[(
                    "arg0",
                    (one.file_name().unwrap_or_default().to_string_lossy()).to_string(),
                )],
            )),
            (CreateVerb::Duplicate, many) => Some(crate::i18n::format(
                "files-duplicated-items-c2f2ac02",
                &[("arg0", (many.len()).to_string())],
            )),
            (CreateVerb::Link, [one]) => Some(crate::i18n::format(
                "files-created-4a73f3b0",
                &[(
                    "arg0",
                    (one.file_name().unwrap_or_default().to_string_lossy()).to_string(),
                )],
            )),
            (CreateVerb::Link, many) => Some(crate::i18n::format(
                "files-created-links-d0f258fe",
                &[("arg0", (many.len()).to_string())],
            )),
        };
        match (done, failed.first()) {
            (Some(done), None) => self.post_status(done, ToastTone::Success),
            (None, Some(first)) if failed.len() == 1 => {
                self.post_status(first.clone(), ToastTone::Error);
            }
            (None, Some(first)) => self.post_status(
                crate::i18n::format(
                    "files-first-more-failed-9f0f6158",
                    &[
                        ("first", (first).clone()),
                        ("arg0", (failed.len() - 1).to_string()),
                    ],
                ),
                ToastTone::Error,
            ),
            (Some(done), Some(first)) => self.post_status(
                crate::i18n::format(
                    "files-done-failed-first-7fd86c3e",
                    &[
                        ("done", (done)),
                        ("arg0", (failed.len()).to_string()),
                        ("first", (first).clone()),
                    ],
                ),
                ToastTone::Warning,
            ),
            (None, None) => {}
        }
        if created
            .iter()
            .any(|path| path.parent() == Some(self.model.cwd.as_path()))
        {
            self.reload()
        } else {
            Task::none()
        }
    }

    // Tabs. Every change reloads the visible listing and saves the session.
    pub(crate) fn reduce_tabs(&mut self, message: Msg) -> Task<Msg> {
        let len = self.model.tabs.len();
        let active = self.model.tabs.active;
        let changed = match message {
            Msg::NewTab => {
                if !self.model.new_tab() {
                    self.post_status(
                        crate::i18n::format(
                            "files-a-window-holds-at-most-tabs-047ea8de",
                            &[("arg0", (crate::tabs::MAX_TABS).to_string())],
                        ),
                        ToastTone::Warning,
                    );
                    return Task::none();
                }
                true
            }
            Msg::CloseTab(id) => {
                let index = id.map_or(Some(active), |id| self.model.tabs.index_of(id));
                index.is_some_and(|index| self.model.close_tab(index))
            }
            Msg::SwitchTab(id) => {
                self.model.tab_list_open = false;
                self.model
                    .tabs
                    .index_of(id)
                    .is_some_and(|index| self.model.switch_tab(index))
            }
            Msg::NextTab => self.model.switch_tab((active + 1) % len),
            Msg::PreviousTab => self.model.switch_tab((active + len - 1) % len),
            Msg::ToggleTabList => {
                self.model.tab_list_open = !self.model.tab_list_open;
                return Task::none();
            }
            _ => unreachable!("misrouted message in reduce_tabs"),
        };
        if !changed {
            return Task::none();
        }
        // The same load a navigation runs for the screen now on show, which
        // also moves the folder watch to it.
        let screen = self.model.screen.clone();
        // A tab left mid-search dropped its walk; resume it on return.
        let search = if self.model.search_active
            && self.model.search_scope == model::SearchScope::Everywhere
            && !self.model.search_query.is_empty()
        {
            self.recursive_search_task()
        } else {
            Task::none()
        };
        Task::batch([
            self.after_screen_change(screen),
            search,
            self.persist_view_settings(),
        ])
    }

    pub(crate) fn reduce_folder_color(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::FolderIdentitiesScanned { dir, identities } => {
                if dir != self.model.cwd {
                    return Task::none();
                }
                self.model.folder_identities = identities.iter().cloned().collect();
                // Mutate a copy: `Tracked` bumps its revision on any mutable
                // borrow, and an unchanged record must not look changed.
                let mut colors = self.model.folder_colors.get().clone();
                if colors.reconcile(&identities) {
                    self.model.folder_colors.set(colors);
                    return self.persist_view_settings();
                }
                Task::none()
            }
            Msg::ShowFolderColor(path) => {
                if crate::services::is_virtual(&path) {
                    return Task::none();
                }
                self.model.folder_color_dialog = Some(path);
                Task::none()
            }
            Msg::CloseFolderColor => {
                self.model.folder_color_dialog = None;
                Task::none()
            }
            Msg::SetFolderColor(path, color) => {
                if let Some(task) = self.guard_view_settings_mutation() {
                    return task;
                }
                // Read the identity now rather than trusting the listing's,
                // which may predate a rename.
                let stat = path.clone();
                Task::perform(
                    crate::shell_work::run(move || {
                        crate::folder_colors::FolderIdentity::of(&stat).map_err(|e| e.to_string())
                    }),
                    move |identity| Msg::FolderColorIdentified {
                        path: path.clone(),
                        color,
                        identity,
                    },
                )
            }
            Msg::FolderColorIdentified {
                path,
                color,
                identity,
            } => match identity {
                Ok(identity) => {
                    if path.parent() == Some(self.model.cwd.as_path()) {
                        self.model.folder_identities.insert(path.clone(), identity);
                    }
                    let mut colors = self.model.folder_colors.get().clone();
                    if colors.set(&path, identity, color) {
                        self.model.folder_colors.set(colors);
                        self.persist_view_settings()
                    } else {
                        Task::none()
                    }
                }
                Err(reason) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.post_status(
                        crate::i18n::format(
                            "files-could-not-change-the-colour-of-38daa979",
                            &[("name", (name).to_string()), ("reason", (reason))],
                        ),
                        ToastTone::Error,
                    );
                    Task::none()
                }
            },
            _ => unreachable!("misrouted message in reduce_folder_color"),
        }
    }

    pub(crate) fn reduce_new_from_template(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NewFromTemplate(template) = message else {
            unreachable!("misrouted message in reduce_new_from_template");
        };
        let dir = self.model.cwd.clone();
        if crate::services::is_virtual(&dir) {
            self.post_status(
                crate::i18n::tr("Cannot create a document here"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        Task::perform(
            crate::shell_work::run(move || services::create_from_template(&dir, &template)),
            Msg::CreationFinished,
        )
    }

    pub(crate) fn reduce_creation_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CreationFinished(result) = message else {
            unreachable!("misrouted message in reduce_creation_finished");
        };
        match result {
            Ok(path) => {
                // Reload only if the user is still in the parent folder; the
                // new entry then opens in place for naming.
                if path.parent() == Some(self.model.cwd.as_path()) {
                    self.model.focus_after_load = Some(path.clone());
                    self.model.rename_after_load = Some(path);
                    self.reload()
                } else {
                    // Elsewhere by now: say where the new entry went.
                    let name = path.file_name().map_or_else(
                        || path.to_string_lossy().into_owned(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    self.post_status(
                        crate::i18n::format("files-created-name-0fb1c5b6", &[("name", (name))]),
                        ToastTone::Success,
                    );
                    Task::none()
                }
            }
            Err(msg) => {
                self.post_status(msg, ToastTone::Error);
                Task::none()
            }
        }
    }

    pub(crate) fn reduce_open_launched(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenLaunched { generation, result } = message else {
            unreachable!("misrouted message in reduce_open_launched");
        };
        if !self.finish_launch(generation) {
            return Task::none();
        }
        // Success is silent; only a failure raises a toast.
        if let Err(message) = result {
            self.post_status(message, ToastTone::Error);
        }
        Task::none()
    }
}
