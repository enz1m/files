// Graceful stop binds activity identities; newly admitted work invalidates the reviewed stop revision.

use super::*;
use enzui_agent_app::{StopChoice, StopRequest, activity_consequences};
use enzui_core::operation_activity::ActivityControl;

// An accepted stop in progress.
#[derive(Debug, Clone)]
pub(crate) struct Stopping {
    // The work the person reviewed, by key.
    reviewed: Vec<String>,
}

impl App {
    pub(crate) fn begin_stop(&mut self, request: StopRequest) -> Task<Msg> {
        // The live work must be exactly what was reviewed: work started this
        // turn has not been published yet, so the revision check alone
        // cannot see it.
        let mut live: Vec<String> = activity_consequences(&self.activity_center)
            .into_iter()
            .map(|consequence| consequence.key)
            .collect();
        let mut reviewed = request.reviewed;
        live.sort_unstable();
        reviewed.sort_unstable();
        if live != reviewed {
            return Task::none();
        }
        self.stopping = Some(Stopping { reviewed });
        if request.choice != StopChoice::CancelAndExit {
            return Task::none();
        }
        self.model.downloads.cancel_all();
        let cancellable: Vec<_> = self
            .activity_center
            .iter()
            .filter(|row| row.is_active() && row.controls.contains(&ActivityControl::Cancel))
            .map(|row| row.key)
            .collect();
        Task::batch(
            cancellable
                .into_iter()
                .map(|key| self.update(Msg::ActivityControl(key, ActivityControl::Cancel))),
        )
    }

    // After every turn: publish what a stop would interrupt, and finish an
    // accepted stop once nothing is left.
    pub(crate) fn reconcile_graceful_stop(&mut self) -> Task<Msg> {
        let live = activity_consequences(&self.activity_center);
        self.agent.publish_stop_consequences(live.clone());
        self.agent
            .publish_work(enzui_agent_app::activity_work(&self.activity_center));
        let Some(stopping) = &self.stopping else {
            return Task::none();
        };
        if live
            .iter()
            .any(|consequence| stopping.reviewed.binary_search(&consequence.key).is_err())
        {
            self.stopping = None;
            return Task::none();
        }
        if !live.is_empty() {
            return Task::none();
        }
        self.stopping = None;
        if self.agent.is_windowless() {
            self.stop_exit = true;
            return Task::none();
        }
        match self.main_window {
            Some(window) => self.update(Msg::CloseRequested(window)),
            None => iced::exit(),
        }
    }
}
