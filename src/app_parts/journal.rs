// Journal undo/redo and prompt helpers.

use super::*;

impl App {
    pub(crate) fn begin_undo(&mut self) -> Task<Msg> {
        if self.model.journal.in_flight_direction().is_some() {
            self.post_status(
                crate::i18n::tr("Undo or Redo is already in progress"),
                ToastTone::Info,
            );
            return Task::none();
        }
        let Some((attempt, entry)) = self.model.journal.begin_undo_attempt() else {
            self.post_status(crate::i18n::tr("Nothing to undo"), ToastTone::Info);
            return Task::none();
        };
        let label = crate::i18n::format(
            "files-undo-4e35bbf0",
            &[("arg0", (journal::describe(&entry)))],
        );
        let activity = self.start_file_activity(ActivityKind::Move, label, 1);
        let inverse = journal::compute_inverse(&entry);
        Task::perform(
            apply_inverse_async(inverse, entry),
            move |(result, entry)| Msg::UndoOperationFinished {
                attempt,
                entry: Box::new(entry),
                activity,
                result,
            },
        )
    }

    pub(crate) fn begin_redo(&mut self) -> Task<Msg> {
        if self.model.journal.in_flight_direction().is_some() {
            self.post_status(
                crate::i18n::tr("Undo or Redo is already in progress"),
                ToastTone::Info,
            );
            return Task::none();
        }
        let Some((attempt, entry)) = self.model.journal.begin_redo_attempt() else {
            self.post_status(crate::i18n::tr("Nothing to redo"), ToastTone::Info);
            return Task::none();
        };
        let label = crate::i18n::format(
            "files-redo-6869ef67",
            &[("arg0", (journal::describe(&entry)))],
        );
        let activity = self.start_file_activity(ActivityKind::Move, label, 1);
        let forward_inverse = journal::compute_redo(&entry);
        Task::perform(
            apply_inverse_async(forward_inverse, entry),
            move |(result, entry)| Msg::RedoOperationFinished {
                attempt,
                entry: Box::new(entry),
                activity,
                result,
            },
        )
    }

    pub(crate) fn finish_undo(
        &mut self,
        attempt: journal::AttemptId,
        entry: journal::JournalEntry,
        activity: activity::FileActivity,
        result: journal::InverseResult,
    ) -> Task<Msg> {
        let entry_id = entry.id;
        let entry_generation = entry.generation;
        let describe = journal::describe(&entry);
        let (placement, is_applied, summary_for_prompt) = match &result {
            journal::InverseResult::Applied => {
                (journal::FinishPlacement::AppliedToOpposite, true, None)
            }
            journal::InverseResult::Conflict(c) => (
                journal::FinishPlacement::ReturnToOrigin,
                false,
                Some(describe_conflict(c)),
            ),
            journal::InverseResult::Failed(m) => (
                journal::FinishPlacement::ReturnToOrigin,
                false,
                Some(m.clone()),
            ),
        };
        match self
            .model
            .journal
            .finish_undo_attempt(attempt, entry, placement)
        {
            journal::FinishAck::WrongAttempt => {
                self.finish_file_activity(
                    activity,
                    0,
                    TerminalOutcome::Failed,
                    crate::i18n::tr("Stale Undo"),
                );
                Task::none()
            }
            journal::FinishAck::Accepted if is_applied => {
                self.finish_file_activity(
                    activity,
                    1,
                    TerminalOutcome::Completed,
                    crate::i18n::tr("Undone"),
                );
                self.post_status(crate::i18n::tr("Undo applied"), ToastTone::Info);
                self.reload()
            }
            journal::FinishAck::Accepted => {
                let summary = summary_for_prompt.unwrap_or_default();
                let headline = crate::i18n::format(
                    "files-undo-blocked-describe-fe4bba2b",
                    &[("describe", (describe))],
                );
                self.finish_file_activity(activity, 0, TerminalOutcome::Failed, summary.clone());
                self.publish_journal_prompt(journal::JournalPrompt {
                    entry_id,
                    entry_generation,
                    headline,
                    detail: summary,
                    is_undo: true,
                });
                Task::none()
            }
        }
    }

    pub(crate) fn finish_redo(
        &mut self,
        attempt: journal::AttemptId,
        entry: journal::JournalEntry,
        activity: activity::FileActivity,
        result: journal::InverseResult,
    ) -> Task<Msg> {
        let entry_id = entry.id;
        let entry_generation = entry.generation;
        let describe = journal::describe(&entry);
        let (placement, is_applied, summary_for_prompt) = match &result {
            journal::InverseResult::Applied => {
                (journal::FinishPlacement::AppliedToOpposite, true, None)
            }
            journal::InverseResult::Conflict(c) => (
                journal::FinishPlacement::ReturnToOrigin,
                false,
                Some(describe_conflict(c)),
            ),
            journal::InverseResult::Failed(m) => (
                journal::FinishPlacement::ReturnToOrigin,
                false,
                Some(m.clone()),
            ),
        };
        match self
            .model
            .journal
            .finish_redo_attempt(attempt, entry, placement)
        {
            journal::FinishAck::WrongAttempt => {
                self.finish_file_activity(
                    activity,
                    0,
                    TerminalOutcome::Failed,
                    crate::i18n::tr("Stale Redo"),
                );
                Task::none()
            }
            journal::FinishAck::Accepted if is_applied => {
                self.finish_file_activity(
                    activity,
                    1,
                    TerminalOutcome::Completed,
                    crate::i18n::tr("Redone"),
                );
                self.post_status(crate::i18n::tr("Redo applied"), ToastTone::Info);
                self.reload()
            }
            journal::FinishAck::Accepted => {
                let summary = summary_for_prompt.unwrap_or_default();
                let headline = crate::i18n::format(
                    "files-redo-blocked-describe-15d23bbf",
                    &[("describe", (describe))],
                );
                self.finish_file_activity(activity, 0, TerminalOutcome::Failed, summary.clone());
                self.publish_journal_prompt(journal::JournalPrompt {
                    entry_id,
                    entry_generation,
                    headline,
                    detail: summary,
                    is_undo: false,
                });
                Task::none()
            }
        }
    }

    // Cut-mode Moved items only. Copy leaves the source in place
    // and Replaced/Merged/Skipped/Failed have no clean inverse.
    pub(crate) fn admit_moves_from_queue(&mut self, queue: &conflict::ConflictQueue) {
        if queue.clip_mode != conflict::ClipMode::Cut {
            return;
        }
        let now_ms = journal::now_ms();
        let mut tally = JournalAdmissionTally::default();
        for item in &queue.resolved {
            if !matches!(item.outcome, conflict::ItemOutcome::Moved) {
                continue;
            }
            let Ok(dst_inode) = journal::InodeIdent::of(&item.dst) else {
                tally.note_capture_unavailable();
                continue;
            };
            tally.observe(self.model.journal.record(
                journal::OpRecord::Move {
                    src: item.src.clone(),
                    dst: item.dst.clone(),
                    dst_inode,
                },
                now_ms,
            ));
        }
        if let Some(msg) = tally.warning_for(crate::i18n::tr("Move")) {
            self.post_status(msg, ToastTone::Warning);
        }
        if tally.touched() {
            self.sweep_journal_prompts();
        }
    }

    // Replaces an existing prompt for `entry_id`, so the banner shows the newest
    // generation, then bounds by `MAX_JOURNAL_PROMPTS`.
    pub(crate) fn publish_journal_prompt(&mut self, prompt: journal::JournalPrompt) {
        self.model
            .journal_prompts
            .retain(|p| p.entry_id != prompt.entry_id);
        while self.model.journal_prompts.len() >= journal::MAX_JOURNAL_PROMPTS {
            self.model.journal_prompts.remove(0);
        }
        self.model.journal_prompts.push(prompt);
    }

    // Called after every journal mutation so a banner never outlives
    // its entry (cap eviction, retire, or a redo-tail clear).
    pub(crate) fn sweep_journal_prompts(&mut self) {
        let journal = &self.model.journal;
        self.model
            .journal_prompts
            .retain(|p| journal.generation_of(p.entry_id) == Some(p.entry_generation));
    }

    pub(crate) fn resolve_journal_conflict(
        &mut self,
        entry_id: journal::JournalId,
        entry_generation: u64,
        choice: journal::JournalConflictChoice,
    ) -> Task<Msg> {
        // A stale generation cannot route to a later entry reusing the id.
        if self.model.journal.generation_of(entry_id) != Some(entry_generation) {
            self.model
                .journal_prompts
                .retain(|p| p.entry_id != entry_id);
            return Task::none();
        }
        match choice {
            journal::JournalConflictChoice::Retry => {
                self.model
                    .journal_prompts
                    .retain(|p| p.entry_id != entry_id);
                if let Some(top) = self.model.journal.peek_undo()
                    && top.id == entry_id
                {
                    return self.begin_undo();
                }
                if let Some(top) = self.model.journal.peek_redo()
                    && top.id == entry_id
                {
                    return self.begin_redo();
                }
                Task::none()
            }
            journal::JournalConflictChoice::Skip => {
                self.model
                    .journal_prompts
                    .retain(|p| p.entry_id != entry_id);
                Task::none()
            }
            journal::JournalConflictChoice::Retire => {
                self.model.journal.retire(entry_id);
                self.model
                    .journal_prompts
                    .retain(|p| p.entry_id != entry_id);
                Task::none()
            }
        }
    }

    // Whether a dialog is up, so window keys must not act on the file list
    // behind it. Built on [`Self::active_generic_dialog`], so a new dialog is
    // covered the moment it is presented; the rest are the presentations that
    // list leaves out.
    pub(crate) fn modal_owns_keyboard(&self) -> bool {
        self.active_generic_dialog().is_some()
            || self.picker.is_pending()
            || self.model.downloads.destination_fallback_open()
            || self.model.pending_permanent_delete.is_some()
            || settings_surface::is_closing(self)
    }
}
