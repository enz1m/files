// Stale-download cleanup: slots, workers, and settlement.

use super::*;

type SharedCleanupAuthority = Arc<Mutex<Option<PausedCleanupJob>>>;
pub(super) type SharedTorrentStage = Arc<Mutex<Option<enzui_security::tempstage::DirectoryStage>>>;

// Explicit phase for one cleanup slot per `(DownloadId, Attempt)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CleanupPhase {
    // Authority held, not yet submitted to the worker.
    Held,
    // Submitted to the bounded worker; awaiting settlement.
    Submitted,
    // Worker returned a nonterminal refusal; authority still present.
    FailedDebt,
    // Cleanup proved complete; authority consumed.
    Settled,
}

// One cleanup slot with move-only authority and explicit phase.
pub(crate) struct CleanupSlot {
    pub(crate) id: DownloadId,
    pub(crate) attempt: Attempt,
    pub(crate) phase: CleanupPhase,
    pub(super) authority: Arc<Mutex<Option<PausedCleanupJob>>>,
}

impl std::fmt::Debug for CleanupSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupSlot")
            .field("id", &self.id)
            .field("attempt", &self.attempt)
            .field("phase", &self.phase)
            .finish()
    }
}

impl CleanupSlot {
    pub(super) fn new(job: PausedCleanupJob) -> Self {
        let id = job.id;
        let attempt = job.attempt;
        Self {
            id,
            attempt,
            phase: CleanupPhase::Held,
            authority: Arc::new(Mutex::new(Some(job))),
        }
    }

    pub(super) fn shared_authority(&self) -> Arc<Mutex<Option<PausedCleanupJob>>> {
        Arc::clone(&self.authority)
    }

    pub(super) fn has_authority(&self) -> bool {
        self.authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

fn run_cleanup_on_slot(
    shared: &SharedCleanupAuthority,
    expected_id: DownloadId,
    expected_attempt: Attempt,
) -> CleanupSettlement {
    let mut guard = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(job) = guard.as_mut() else {
        return CleanupSettlement {
            id: expected_id,
            attempt: expected_attempt,
            result: Err(CleanupRefusal::Io),
            detail: Some(crate::i18n::tr("cleanup authority was lost").into()),
        };
    };
    let id = job.id;
    let attempt = job.attempt;
    if id != expected_id || attempt != expected_attempt {
        return CleanupSettlement {
            id: expected_id,
            attempt: expected_attempt,
            result: Err(CleanupRefusal::Io),
            detail: Some(crate::i18n::tr("cleanup authority identity mismatch").into()),
        };
    }

    if let Some(shared_stage) = &job.torrent_stage {
        let mut stage_guard = shared_stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(stage) = stage_guard.as_mut() {
            match stage.try_cleanup() {
                Ok(true) => {
                    stage_guard.take();
                    drop(stage_guard);
                    guard.take();
                    return CleanupSettlement {
                        id,
                        attempt,
                        result: Ok(()),
                        detail: None,
                    };
                }
                Ok(false) => {
                    return CleanupSettlement {
                        id,
                        attempt,
                        result: Err(CleanupRefusal::Incomplete),
                        detail: Some(
                            crate::i18n::tr("identity or budget prevented complete cleanup").into(),
                        ),
                    };
                }
                Err(error) => {
                    return CleanupSettlement {
                        id,
                        attempt,
                        result: Err(CleanupRefusal::Io),
                        detail: Some(crate::i18n::format(
                            "files-could-not-remove-partial-download-280db6a1",
                            &[(
                                "arg0",
                                (io_failure_reason(IoFailureCategory::classify(&error))).to_owned(),
                            )],
                        )),
                    };
                }
            }
        }
        drop(stage_guard);
        guard.take();
        return CleanupSettlement {
            id,
            attempt,
            result: Ok(()),
            detail: None,
        };
    }

    if let Some(reservation) = job.http_reservation.as_ref() {
        match reservation.remove_if_reserved() {
            Ok(()) => {
                guard.take();
                return CleanupSettlement {
                    id,
                    attempt,
                    result: Ok(()),
                    detail: None,
                };
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                guard.take();
                return CleanupSettlement {
                    id,
                    attempt,
                    result: Ok(()),
                    detail: None,
                };
            }
            Err(error) => {
                return CleanupSettlement {
                    id,
                    attempt,
                    result: Err(CleanupRefusal::Io),
                    detail: Some(crate::i18n::format(
                        "files-could-not-remove-partial-download-280db6a1",
                        &[(
                            "arg0",
                            (io_failure_reason(IoFailureCategory::classify(&error))).to_owned(),
                        )],
                    )),
                };
            }
        }
    }
    CleanupSettlement {
        id,
        attempt,
        result: Err(CleanupRefusal::Io),
        detail: Some(crate::i18n::tr("refusing to remove an unowned partial download").into()),
    }
}

const CLEANUP_WORKER_COUNT: usize = 1;
const CLEANUP_QUEUE_CAPACITY: usize = 4;
const CLEANUP_WORKERS: NonZeroUsize =
    NonZeroUsize::new(CLEANUP_WORKER_COUNT).expect("positive cleanup worker count");
const CLEANUP_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(CLEANUP_QUEUE_CAPACITY).expect("positive cleanup queue capacity");
// Typed result of the off-thread shutdown helper.
#[derive(Debug, Clone)]
pub(crate) enum ShutdownOutcome {
    // All workers joined cleanly (zero panics).
    Clean,
    // Workers joined but some caught/unwound panics.
    Panicked { count: usize },
    // The helper thread itself panicked.
    HelperPanicked,
    // Shutdown returned without proof that every worker joined.
    Unconfirmed,
}

impl ShutdownOutcome {
    pub(crate) fn from_join(result: enzui_core::blocking_task::BlockingShutdown) -> Self {
        use enzui_core::blocking_task::BlockingShutdown;
        match result {
            BlockingShutdown::Joined { panicked: 0, .. } => Self::Clean,
            BlockingShutdown::Joined { panicked, .. } => Self::Panicked { count: panicked },
            _ => Self::Unconfirmed,
        }
    }
}

// Tracked state of the cleanup worker owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CleanupOwnerState {
    // Worker is live and usable.
    Ready,
    // Helper thread is running shutdown; worker consumed.
    HelperRunning,
    // All cleanup workers joined without panics.
    JoinedClean,
    // Cleanup workers joined after a panic; publication custody remains uncertain.
    JoinedPanicked,
    // Worker termination is unconfirmed, including a helper panic.
    // Unconfirmed termination prevents process exit and new transfers.
    OwnerUncertain,
}

// Why a `hold_cleanup` insertion was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldRefusal {
    // A slot for this id is Submitted, FailedDebt, or Settled-with-authority.
    Conflict,
    // Same id is already Held; original authority preserved.
    AlreadyHeld,
}

// Why a cleanup submission did not enter the bounded queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CleanupRefusal {
    Busy,
    Shutdown,
    Cancelled,
    Panicked,
    Io,
    Incomplete,
}

// Settlement of one cleanup generation keyed by `(DownloadId, Attempt)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CleanupSettlement {
    pub(crate) id: DownloadId,
    pub(crate) attempt: Attempt,
    pub(crate) result: Result<(), CleanupRefusal>,
    pub(crate) detail: Option<String>,
}

// Owned bounded cleanup executor with explicit shutdown and join.
pub(crate) struct CleanupWorker {
    owner: enzui_core::blocking_task::BlockingTaskOwner,
    generation: u64,
    user_admission_closed: bool,
}

impl std::fmt::Debug for CleanupWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupWorker")
            .field("generation", &self.generation)
            .field("user_admission_closed", &self.user_admission_closed)
            .finish()
    }
}

impl CleanupWorker {
    pub(crate) fn new() -> Option<Self> {
        let owner = enzui_core::blocking_task::BlockingTaskOwner::new(
            "files-download-cleanup",
            CLEANUP_WORKERS,
            CLEANUP_CAPACITY,
        )
        .ok()?;
        Some(Self {
            owner,
            generation: 0,
            user_admission_closed: false,
        })
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            owner: enzui_core::blocking_task::BlockingTaskOwner::unavailable(CLEANUP_CAPACITY),
            generation: 0,
            user_admission_closed: true,
        }
    }

    // Submits from a user action; refused once user admission closes or
    // generations run out.
    pub(crate) fn submit(
        &mut self,
        shared: SharedCleanupAuthority,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<enzui_core::blocking_task::BlockingTask<CleanupSettlement>> {
        if self.user_admission_closed {
            return None;
        }
        self.submit_inner(shared, id, attempt)
    }

    // Submits from the close-drain path, ignoring user admission; fails only when
    // generations run out.
    pub(crate) fn submit_drain(
        &mut self,
        shared: SharedCleanupAuthority,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<enzui_core::blocking_task::BlockingTask<CleanupSettlement>> {
        self.submit_inner(shared, id, attempt)
    }

    fn submit_inner(
        &mut self,
        shared: SharedCleanupAuthority,
        id: DownloadId,
        attempt: Attempt,
    ) -> Option<enzui_core::blocking_task::BlockingTask<CleanupSettlement>> {
        let generation = self.next_generation()?;
        Some(self.owner.submit(generation, move |_ctx| {
            run_cleanup_on_slot(&shared, id, attempt)
        }))
    }

    // The next nonzero generation; 0 is the exhausted sentinel.
    fn next_generation(&mut self) -> Option<u64> {
        let next = self.generation.checked_add(1)?;
        self.generation = next;
        Some(next)
    }

    pub(crate) fn close_admission(&mut self) {
        self.user_admission_closed = true;
    }

    pub(crate) fn snapshot(&self) -> enzui_core::blocking_task::BlockingTaskSnapshot {
        self.owner.dispatcher().snapshot()
    }

    // Consumes the worker and blocks until every thread joins. Call from a helper
    // thread, never the UI thread.
    pub(crate) fn shutdown_and_join(self) -> enzui_core::blocking_task::BlockingShutdown {
        self.owner.shutdown_and_join()
    }

    pub(crate) fn is_idle(&self) -> bool {
        let snap = self.snapshot();
        snap.live() == 0
    }

    // Submits a publication reconciliation closure to the bounded worker.
    pub(crate) fn submit_reconciliation<F, R>(
        &mut self,
        work: F,
    ) -> Option<enzui_core::blocking_task::BlockingTask<R>>
    where
        F: FnOnce(&enzui_core::blocking_task::BlockingTaskContext) -> R + Send + 'static,
        R: Send + 'static,
    {
        let generation = self.next_generation()?;
        Some(self.owner.submit(generation, move |context| work(&context)))
    }
}

// Drives a cleanup `BlockingTask` settlement into the reducer.
pub(crate) fn cleanup_task(
    task: enzui_core::blocking_task::BlockingTask<CleanupSettlement>,
    id: DownloadId,
    attempt: Attempt,
) -> iced::Task<crate::message::Msg> {
    iced::Task::future(async move {
        let settlement = task.await;
        let cs = match settlement.into_result() {
            Ok(cs) => cs,
            Err(enzui_core::blocking_task::BlockingTaskError::Busy) => CleanupSettlement {
                id,
                attempt,
                result: Err(CleanupRefusal::Busy),
                detail: None,
            },
            Err(enzui_core::blocking_task::BlockingTaskError::Cancelled) => CleanupSettlement {
                id,
                attempt,
                result: Err(CleanupRefusal::Cancelled),
                detail: None,
            },
            Err(enzui_core::blocking_task::BlockingTaskError::Shutdown) => CleanupSettlement {
                id,
                attempt,
                result: Err(CleanupRefusal::Shutdown),
                detail: None,
            },
            Err(enzui_core::blocking_task::BlockingTaskError::Panicked) => CleanupSettlement {
                id,
                attempt,
                result: Err(CleanupRefusal::Panicked),
                detail: None,
            },
            Err(error) => CleanupSettlement {
                id,
                attempt,
                result: Err(CleanupRefusal::Io),
                detail: Some(crate::i18n::format(
                    "files-cleanup-worker-refused-error-5e14bd54",
                    &[("error", (error).to_string())],
                )),
            },
        };
        crate::message::Msg::PausedDownloadCleaned(cs)
    })
}
