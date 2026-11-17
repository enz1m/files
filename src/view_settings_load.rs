// Take-once handoff for the framework `LoadCompletion<ViewSettings>`.
//
// iced requires `Msg: Clone`, but a completion must reach
// `LoadController::settle` at most once, or a cloned message could settle the
// same worker slot twice or re-apply a superseded outcome. The message
// carries `Arc<ViewSettingsLoadHandoff>`; [`ViewSettingsLoadHandoff::take`]
// yields the completion on first delivery and `None` afterwards.

use std::sync::Mutex;

use enzui_core::persist::LoadCompletion;

use crate::view_settings_store::ViewSettings;

pub(crate) struct ViewSettingsLoadHandoff(Mutex<Option<LoadCompletion<ViewSettings>>>);

impl ViewSettingsLoadHandoff {
    pub(crate) fn new(completion: LoadCompletion<ViewSettings>) -> Self {
        Self(Mutex::new(Some(completion)))
    }

    pub(crate) fn take(&self) -> Option<LoadCompletion<ViewSettings>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl std::fmt::Debug for ViewSettingsLoadHandoff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ViewSettingsLoadHandoff")
            .finish_non_exhaustive()
    }
}
