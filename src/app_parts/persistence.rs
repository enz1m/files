// Saving bookmarks and view settings, and holding edits while the view
// settings are still loading.

use super::*;

impl App {
    // Publish the current bookmarks through the held authority.
    pub(crate) fn save_bookmarks(&mut self) -> SaveOutcome {
        use enzui_core::state_authority::PublishCustody;

        #[cfg(test)]
        if self.pin_mode == PinPersistenceMode::TestDisarmed && self.pin_authority.is_none() {
            return SaveOutcome::Disarmed;
        }
        let Some(ref mut authority) = self.pin_authority else {
            return SaveOutcome::Blocked;
        };
        if self.model.pins_refused.is_some() {
            return SaveOutcome::Blocked;
        }
        match services::publish_bookmarks(authority, &self.model.bookmarks) {
            Ok(()) => {
                self.model.pin_durability_debt = false;
                SaveOutcome::Published
            }
            Err(e) => match e.custody() {
                None | Some(PublishCustody::Unchanged) => SaveOutcome::Unchanged,
                Some(PublishCustody::Rotated) => {
                    self.model.pin_durability_debt = true;
                    SaveOutcome::Rotated
                }
                Some(_) => {
                    self.model.pins_refused = Some(services::PinRefusal::CustodyLost);
                    SaveOutcome::Lost
                }
            },
        }
    }

    // Coalesced, off-thread save of the durable view settings.
    //
    // A cheap token is compared first, so an unchanged model costs nothing. The
    // snapshot is serialised on the UI thread and written by `Task::perform`.
    pub(crate) fn persist_view_settings(&mut self) -> Task<Msg> {
        let token = view_settings_store::ViewSettings::token(&self.model);
        let model = &self.model;
        match self
            .view_settings_gate
            .prepare_save_if_changed(token, || view_settings_store::ViewSettings::snapshot(model))
        {
            Ok(Some(op)) => Task::perform(
                crate::shell_work::run(move || op.execute()),
                Msg::ViewSettingsSaved,
            ),
            Ok(None) => Task::none(),
            Err(error) => {
                self.view_settings_save_error = Some(error.to_string());
                Task::none()
            }
        }
    }

    // Whether the async view-settings load is still in flight.
    //
    // While `true`, every reducer arm that mutates a
    // [`view_settings_store::ViewSettings::snapshot`] field must refuse, or a
    // pre-load snapshot would overwrite the durable record.
    pub(crate) fn view_settings_load_pending(&self) -> bool {
        self.view_settings_controller
            .as_ref()
            .is_some_and(enzui_core::persist::LoadController::is_loading)
    }

    // Refuses a view-settings mutation while the load is pending.
    //
    // Returns `Some(Task::none())` after posting a path-free toast, for the caller
    // to return before touching the model; `None` once the load has settled.
    pub(crate) fn guard_view_settings_mutation(&mut self) -> Option<Task<Msg>> {
        if self.view_settings_load_pending() {
            self.post_status(
                crate::i18n::tr("View settings are still loading; please retry in a moment"),
                ToastTone::Warning,
            );
            Some(Task::none())
        } else {
            None
        }
    }

    // Writes the view settings synchronously before exit, since a task batched
    // after `iced::exit()` may never run. No-op when unchanged.
    pub(crate) fn flush_view_settings_blocking(&mut self) {
        let token = view_settings_store::ViewSettings::token(&self.model);
        let model = &self.model;
        match self
            .view_settings_gate
            .prepare_save_if_changed(token, || view_settings_store::ViewSettings::snapshot(model))
        {
            Ok(Some(op)) => {
                if let Err(error) = op.execute() {
                    self.view_settings_save_error = Some(error.to_string());
                }
            }
            Ok(None) => {}
            Err(error) => self.view_settings_save_error = Some(error.to_string()),
        }
    }
}
