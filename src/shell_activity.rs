// App-owned shell publication for file transfers and downloads.

use crate::{activity::FileActivity, conflict};
use enzui_activity::{
    CenterJobs, Job, Outcome, Progress, Publisher, State, Status, Unit, Visibility,
};
use enzui_core::operation_activity::{ActivityKind, LivePhase, TerminalOutcome};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct ShellActivity {
    publisher: Option<Publisher>,
    pending_tag: Option<String>,
    queued_tag: Option<String>,
    jobs: BTreeMap<(u64, u32), Transfer>,
    downloads: CenterJobs,
}

impl ShellActivity {
    #[cfg(not(test))]
    pub(crate) fn production() -> Self {
        Self {
            publisher: Publisher::new("io.github.enzui.Files").ok(),
            pending_tag: enzui_activity::new_window_token().ok(),
            ..Self::default()
        }
    }

    pub(crate) fn pending_tag(&self) -> Option<&str> {
        self.publisher.as_ref()?;
        self.pending_tag.as_deref()
    }

    pub(crate) fn confirm_tag(&mut self, tag: String, queued: bool) {
        if self.pending_tag.as_deref() != Some(&tag) {
            return;
        }
        self.queued_tag = queued.then_some(tag);
        for transfer in self.jobs.values() {
            let _ = transfer.job.set_window_token(self.queued_tag.clone());
        }
    }

    pub(crate) fn start(&mut self, activity: FileActivity, kind: ActivityKind, total: usize) {
        let label = match kind {
            ActivityKind::Copy => crate::i18n::tr("Copying files"),
            ActivityKind::Move => crate::i18n::tr("Moving files"),
            _ => return,
        };
        let Some(publisher) = &self.publisher else {
            return;
        };
        let status = Status {
            window_token: self.queued_tag.clone(),
            progress: (total > 0).then_some(Progress {
                done: 0,
                total: total as u64,
                unit: Unit::Items,
            }),
            ..Status::running(glib::dgettext(Some("files-app"), label).to_string())
        };
        if let Ok(job) = publisher.start_after(
            status,
            Visibility::WhenUnfocused,
            std::time::Duration::from_millis(800),
        ) {
            self.jobs.insert(
                (activity.key.op, activity.generation.0),
                Transfer {
                    job,
                    total: total as u64,
                    done: 0,
                },
            );
        }
    }

    pub(crate) fn sync_downloads(
        &mut self,
        center: &enzui_core::operation_activity::ActivityCenter,
    ) {
        let Some(publisher) = &self.publisher else {
            return;
        };
        self.downloads.sync(
            publisher,
            center,
            self.queued_tag.as_deref(),
            Visibility::WhenUnfocused,
            |row| {
                (row.key.provider == enzui_core::operation_activity::downloads::DOWNLOADS)
                    .then(|| crate::i18n::tr("Downloading").to_owned())
            },
        );
    }

    pub(crate) fn transfer(&self, activity: FileActivity) -> Option<Transfer> {
        self.jobs
            .get(&(activity.key.op, activity.generation.0))
            .cloned()
    }

    pub(crate) fn phase(&self, activity: FileActivity, phase: LivePhase) {
        if let Some(transfer) = self.transfer(activity) {
            let state = match phase {
                LivePhase::Paused | LivePhase::Waiting => State::Paused,
                LivePhase::Queued | LivePhase::Running => State::Running,
            };
            let _ = transfer.job.set_state(state);
        }
    }

    pub(crate) fn finish(&mut self, activity: FileActivity, outcome: TerminalOutcome) {
        if let Some(transfer) = self.jobs.remove(&(activity.key.op, activity.generation.0)) {
            let _ = transfer.job.finish(match outcome {
                TerminalOutcome::Completed => Outcome::Succeeded,
                TerminalOutcome::Failed => Outcome::Failed,
                TerminalOutcome::Cancelled => Outcome::Cancelled,
            });
        }
    }
}

#[derive(Clone)]
pub(crate) struct Transfer {
    job: Job,
    total: u64,
    done: u64,
}

impl Transfer {
    pub(crate) fn resume_from(&mut self, queue: &conflict::ConflictQueue) {
        self.done = queue.completed_items() as u64;
        let _ = self.job.set_state(State::Running);
    }

    pub(crate) fn performed(&mut self, outcome: &conflict::ItemOutcome) {
        if outcome.changed_destination() {
            self.done = self.done.saturating_add(1).min(self.total);
            let progress = (self.total > 0).then_some(Progress {
                done: self.done,
                total: self.total,
                unit: Unit::Items,
            });
            let _ = self.job.set_progress(progress, None);
        }
    }

    pub(crate) fn settle(&self, queue: &conflict::ConflictQueue) {
        if queue.is_open() {
            let _ = self.job.set_state(State::Paused);
        } else {
            let outcome = if queue.cancelled {
                Outcome::Cancelled
            } else if queue
                .resolved
                .iter()
                .any(|item| matches!(item.outcome, conflict::ItemOutcome::Failed(_)))
            {
                Outcome::Failed
            } else {
                Outcome::Succeeded
            };
            let _ = self.job.finish(outcome);
        }
    }
}
