// Cleanup and retry: jobs, debts, shutdown, and outcomes.

use super::*;

impl DownloadsState {
    // Snapshots cleanup work for a paused job.
    pub(crate) fn paused_cleanup_job(&mut self, id: DownloadId) -> Option<PausedCleanupJob> {
        let attempt = self.queue.get(id)?.attempt;
        self.paused_cleanup_job_for(id, attempt)
    }

    // Whether `(id, attempt)` is current, as the queue still accepts it.
    pub(crate) fn accepts_attempt(&self, id: DownloadId, attempt: Attempt) -> bool {
        self.queue
            .get(id)
            .is_some_and(|download| download.accepts(attempt))
    }

    pub(crate) fn stage_cleanup_job(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<PausedCleanupJob> {
        if !self.accepts_attempt(id, attempt) {
            return None;
        }
        let torrent_stage = self.torrent_shared_stages.get(&id.0).cloned();
        let has_stage = torrent_stage.as_ref().is_some_and(|s| {
            s.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_some()
        });
        if !has_stage {
            return None;
        }
        Some(PausedCleanupJob {
            id,
            attempt,
            torrent_stage,
            http_reservation: None,
        })
    }

    fn paused_cleanup_job_for(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<PausedCleanupJob> {
        let torrent_stage = self.torrent_shared_stages.get(&id.0).cloned();
        let http_reservation = self.http_cleanup_authorities.remove(&id.0);
        if torrent_stage.is_none() && http_reservation.is_none() {
            return None;
        }
        Some(PausedCleanupJob {
            id,
            attempt,
            torrent_stage,
            http_reservation,
        })
    }

    // Releases cleanup state only while `(id, attempt)` is current.
    pub(crate) fn finish_paused_cleanup(&mut self, id: DownloadId, attempt: Attempt) {
        let current = self.queue.get(id).map(|d| d.attempt);
        if current.is_some_and(|a| a != attempt) {
            return;
        }
        self.controls.remove(&id.0);
        self.owned_http_partials.remove(&id.0);
        self.http_cleanup_authorities.remove(&id.0);
        self.torrent_stages.remove(&id.0);
        self.torrent_shared_stages.remove(&id.0);
        self.torrent_output_dirs.remove(&id.0);
        self.errors.remove(&id.0);
        self.cleanup_error = None;
        self.settle_cleanup(id, attempt);
        self.cleanup_slots.remove(&id.0);
    }

    // Inserts a Held cleanup slot.
    //
    // A Submitted or FailedDebt slot refuses with Conflict, the same Held attempt
    // with AlreadyHeld, and a Settled slot only after no authority remains.
    pub(crate) fn hold_cleanup(&mut self, job: PausedCleanupJob) -> Result<(), HoldRefusal> {
        let id = job.id;
        let _attempt = job.attempt;
        if let Some(existing) = self.cleanup_slots.get(&id.0) {
            match existing.phase {
                CleanupPhase::Submitted | CleanupPhase::FailedDebt => {
                    return Err(HoldRefusal::Conflict);
                }
                CleanupPhase::Held => {
                    return Err(HoldRefusal::AlreadyHeld);
                }
                CleanupPhase::Settled => {
                    if existing.has_authority() {
                        return Err(HoldRefusal::Conflict);
                    }
                }
            }
        }
        let slot = CleanupSlot::new(job);
        self.cleanup_slots.insert(id.0, slot);
        Ok(())
    }

    // Submits a Held or FailedDebt slot through user admission; returns `None`
    // and leaves the slot unchanged while admission is closed.
    pub(crate) fn submit_held_cleanup(
        &mut self,
        id: DownloadId,
    ) -> Option<iced::Task<crate::message::Msg>> {
        let slot = self.cleanup_slots.get_mut(&id.0)?;
        if !matches!(slot.phase, CleanupPhase::Held | CleanupPhase::FailedDebt) {
            return None;
        }
        if !slot.has_authority() {
            return None;
        }
        let attempt = slot.attempt;
        let shared = slot.shared_authority();
        let bt = self.cleanup_worker.submit(shared, id, attempt)?;
        slot.phase = CleanupPhase::Submitted;
        Some(cleanup_task(bt, id, attempt))
    }

    // Submits a Held or FailedDebt slot through drain admission, bypassing user
    // admission.
    pub(crate) fn submit_drain_slot(
        &mut self,
        id: DownloadId,
    ) -> Option<iced::Task<crate::message::Msg>> {
        let slot = self.cleanup_slots.get_mut(&id.0)?;
        if !matches!(slot.phase, CleanupPhase::Held | CleanupPhase::FailedDebt) {
            return None;
        }
        if !slot.has_authority() {
            return None;
        }
        let attempt = slot.attempt;
        let shared = slot.shared_authority();
        let bt = self.cleanup_worker.submit_drain(shared, id, attempt)?;
        slot.phase = CleanupPhase::Submitted;
        Some(cleanup_task(bt, id, attempt))
    }

    pub(crate) fn record_cleanup_debt(&mut self, id: DownloadId, attempt: Attempt) {
        if let Some(slot) = self.cleanup_slots.get_mut(&id.0)
            && slot.attempt == attempt
        {
            slot.phase = CleanupPhase::FailedDebt;
        }
    }

    pub(crate) fn settle_cleanup(&mut self, id: DownloadId, attempt: Attempt) {
        if let Some(slot) = self.cleanup_slots.get_mut(&id.0)
            && slot.attempt == attempt
        {
            slot.phase = CleanupPhase::Settled;
        }
    }

    pub(crate) fn has_cleanup_debt(&self) -> bool {
        self.cleanup_slots
            .values()
            .any(|s| !matches!(s.phase, CleanupPhase::Settled))
            || self.has_any_publication_debt()
    }

    pub(crate) fn has_unresolved_cleanup(&self, id: DownloadId) -> bool {
        self.cleanup_slots
            .get(&id.0)
            .is_some_and(|s| !matches!(s.phase, CleanupPhase::Settled))
    }

    pub(crate) fn retry_publication_debts(&mut self) -> Vec<iced::Task<crate::message::Msg>> {
        let ids_with_debt: Vec<(DownloadId, Attempt)> = self
            .publication_debts
            .iter()
            .filter(|(_, arc)| {
                // Skips in-flight (`WouldBlock`) and permanently lost debt.
                match arc.try_lock() {
                    Ok(guard) => guard.as_ref().is_some_and(|d| !d.custody_lost),
                    Err(std::sync::TryLockError::WouldBlock) => false,
                    Err(std::sync::TryLockError::Poisoned(e)) => {
                        e.into_inner().as_ref().is_some_and(|d| !d.custody_lost)
                    }
                }
            })
            .filter_map(|(&raw, _)| {
                let id = DownloadId(raw);
                let attempt = self.queue.get(id)?.attempt;
                Some((id, attempt))
            })
            .collect();
        let mut tasks = Vec::new();
        for (id, attempt) in ids_with_debt {
            if let Some(task) = self.submit_publication_reconciliation(id, attempt) {
                tasks.push(task);
            }
        }
        tasks
    }

    // Submits one publication reconciliation to the bounded worker.
    fn submit_publication_reconciliation(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<iced::Task<crate::message::Msg>> {
        let debt_arc = std::sync::Arc::clone(self.publication_debts.get(&id.0)?);
        let output_dir = self.torrent_output_dirs.get(&id.0)?.clone();
        let bt = self.cleanup_worker.submit_reconciliation(move |_ctx| {
            run_publication_reconciliation(&debt_arc, &output_dir, id, attempt)
        })?;
        Some(iced::Task::future(async move {
            let settlement = bt.await;
            match settlement.into_result() {
                Ok(ps) => crate::message::Msg::PublicationReconciled(ps),
                Err(_) => crate::message::Msg::PublicationReconciled(PublicationSettlement {
                    id,
                    attempt,
                    complete: false,
                    detail: Some(crate::i18n::tr("reconciliation worker refused").into()),
                }),
            }
        }))
    }

    // Resubmits Held and FailedDebt slots through drain admission, never
    // Submitted ones.
    pub(crate) fn retry_cleanup_debts(&mut self) -> Vec<iced::Task<crate::message::Msg>> {
        let eligible: Vec<DownloadId> = self
            .cleanup_slots
            .values()
            .filter(|s| matches!(s.phase, CleanupPhase::Held | CleanupPhase::FailedDebt))
            .map(|s| s.id)
            .collect();
        let mut tasks = Vec::new();
        for id in eligible {
            if let Some(task) = self.submit_drain_slot(id) {
                tasks.push(task);
            }
        }
        tasks
    }

    pub(crate) fn close_cleanup_admission(&mut self) {
        self.cleanup_worker.close_admission();
    }

    pub(crate) fn cleanup_idle(&self) -> bool {
        self.cleanup_worker.is_idle()
    }

    // Spawns a helper thread that runs `shutdown_and_join` on the cleanup worker.
    //
    // The caller retains the returned handle and polls it without blocking. On
    // spawn failure the worker is restored.
    pub(crate) fn spawn_shutdown_helper(
        &mut self,
    ) -> Result<std::thread::JoinHandle<ShutdownOutcome>, ()> {
        // The worker is sent only after spawn succeeds, so failure can restore it.
        let (tx, rx) = std::sync::mpsc::sync_channel::<CleanupWorker>(1);
        let handle = std::thread::Builder::new()
            .name("files-cleanup-shutdown".into())
            .spawn(move || {
                let Ok(worker) = rx.recv() else {
                    return ShutdownOutcome::HelperPanicked;
                };
                ShutdownOutcome::from_join(worker.shutdown_and_join())
            })
            .map_err(|_| ())?;
        let worker = std::mem::replace(&mut self.cleanup_worker, CleanupWorker::unavailable());
        match tx.send(worker) {
            Ok(()) => {
                self.cleanup_owner_state = CleanupOwnerState::HelperRunning;
                Ok(handle)
            }
            Err(std::sync::mpsc::SendError(returned_worker)) => {
                // The helper exited first: restore the worker and join the thread.
                self.cleanup_worker = returned_worker;
                let _ = handle.join();
                Err(())
            }
        }
    }

    // Records the helper join outcome in `cleanup_owner_state`.
    pub(crate) fn record_helper_outcome(&mut self, outcome: &ShutdownOutcome) {
        self.cleanup_owner_state = match outcome {
            ShutdownOutcome::Clean => CleanupOwnerState::JoinedClean,
            ShutdownOutcome::Panicked { .. } => CleanupOwnerState::JoinedPanicked,
            ShutdownOutcome::HelperPanicked | ShutdownOutcome::Unconfirmed => {
                CleanupOwnerState::OwnerUncertain
            }
        };
    }

    pub(crate) fn record_cleanup_error(&mut self, id: DownloadId, error: String) {
        self.cleanup_error = Some(crate::i18n::format(
            "files-could-not-clean-up-error-d486669d",
            &[("arg0", (self.name_of(id)).to_owned()), ("error", (error))],
        ));
    }

    // Records the engine's failure text before the worker's final outcome.
    pub(crate) fn record_event_failure(&mut self, id: DownloadId, error: &str) {
        self.errors.insert(id.0, error.to_owned());
    }

    // Whether a retry is allowed, checked before queue mutation.
    pub(crate) fn can_retry(&self, id: DownloadId) -> bool {
        !self.is_owner_uncertain()
            && !self.has_unresolved_cleanup(id)
            && !self.has_publication_debt(id)
    }

    // Clears prior attempt state after the queue accepted a retry. Refuses while
    // publication debt, live reconciliation or unresolved cleanup exists.
    pub(crate) fn begin_retry(&mut self, id: DownloadId) -> bool {
        if self.has_publication_debt(id) || self.has_unresolved_cleanup(id) {
            return false;
        }
        // The admission-time destination stays; the next attempt is built from it.
        self.cleanup_slots.remove(&id.0);
        self.errors.remove(&id.0);
        self.controls.remove(&id.0);
        self.torrent_shared_stages.remove(&id.0);
        self.http_cleanup_authorities.remove(&id.0);
        self.publication_debts.remove(&id.0);
        true
    }

    // Folds a finished run's outcome into the panel; returns `false` for a
    // superseded attempt.
    //
    // Only `BackendUnavailable { backend: "aria2c" }` changes backend status: it
    // measures a missing aria2c. Other unavailable backends fail only this download.
    pub(crate) fn record_outcome(
        &mut self,
        id: DownloadId,
        attempt: Attempt,
        outcome: &TransferOutcome,
    ) -> bool {
        if !self
            .queue
            .get(id)
            .is_some_and(|download| download.accepts(attempt))
        {
            return false;
        }
        match outcome {
            TransferOutcome::Failed { error, .. } => {
                self.clear_torrent_telemetry(id);
                if let TransferError::BackendUnavailable { backend, .. } = error
                    && backend == ARIA2C_BACKEND
                {
                    self.set_backend(Aria2Status::Absent);
                    self.errors.remove(&id.0);
                } else {
                    self.errors.insert(id.0, error.message());
                }
                self.controls.remove(&id.0);
                // Stage/output authority stays for CleanupSlot submission.
            }
            TransferOutcome::Completed { .. } => {
                self.clear_torrent_telemetry(id);
                self.errors.remove(&id.0);
                if let Some(folder) = self.completed_folder_for(id) {
                    self.completed_folders.insert(id.0, folder);
                }
                self.controls.remove(&id.0);
                if !self.has_unresolved_cleanup(id) {
                    self.owned_http_partials.remove(&id.0);
                    self.http_cleanup_authorities.remove(&id.0);
                    self.torrent_stages.remove(&id.0);
                    self.torrent_shared_stages.remove(&id.0);
                    self.torrent_output_dirs.remove(&id.0);
                }
            }
            TransferOutcome::Cancelled { .. } => {
                self.clear_torrent_telemetry(id);
                self.errors.remove(&id.0);
                self.controls.remove(&id.0);
                // Stage/output authority stays for CleanupSlot submission.
            }
            TransferOutcome::Paused { .. } => {
                self.controls.remove(&id.0);
            }
        }
        true
    }

    // The failure text for `id`, if its last run failed.
    pub(crate) fn error_of(&self, id: DownloadId) -> Option<&str> {
        self.errors.get(&id.0).map(String::as_str)
    }

    // Where a completed download landed, for the row's Show in Folder action.
    pub(crate) fn completed_folder(&self, id: DownloadId) -> Option<&Path> {
        self.completed_folders.get(&id.0).map(PathBuf::as_path)
    }

    // The folder a finishing transfer wrote into, read before its destination
    // authority is released: an HTTP file's parent, or the
    // torrent's container directory.
    fn completed_folder_for(&self, id: DownloadId) -> Option<PathBuf> {
        if let Some((root, _)) = self.torrent_stages.get(&id.0) {
            return Some(root.clone());
        }
        self.owned_http_partials
            .get(&id.0)
            .and_then(|file| file.parent().map(Path::to_path_buf))
    }

    // The failure for the panel's own banner. Missing-aria2 errors are excluded,
    // as the torrent-capability banner explains them.
    pub(crate) fn banner_error(&self) -> Option<&str> {
        self.cleanup_error
            .as_deref()
            .or_else(|| self.queue.iter().filter_map(|d| self.error_of(d.id)).last())
    }

    // Whether a download has publication debt or live reconciliation
    // (`try_lock` returning `WouldBlock`).
    pub(crate) fn has_publication_debt(&self, id: DownloadId) -> bool {
        self.publication_debts
            .get(&id.0)
            .is_some_and(|d| match d.try_lock() {
                Ok(guard) => guard.is_some(),
                Err(std::sync::TryLockError::WouldBlock) => true,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().is_some(),
            })
    }

    fn has_any_publication_debt(&self) -> bool {
        self.publication_debts.values().any(|d| match d.try_lock() {
            Ok(guard) => guard.is_some(),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().is_some(),
        })
    }

    pub(crate) fn is_owner_uncertain(&self) -> bool {
        self.cleanup_owner_state == CleanupOwnerState::OwnerUncertain
    }
}
