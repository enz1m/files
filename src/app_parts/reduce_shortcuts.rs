// Rebinding keyboard shortcuts from the Settings Shortcuts page.

use super::*;
use enzui_core::editable_shortcuts::RebindRefusal;
use enzui_widgets::editable_shortcuts::EditableShortcutsEvent;
use enzui_widgets::key_capture_chip::KeyCapture;

impl App {
    pub(crate) fn reduce_shortcuts(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::Shortcut(event) => self.shortcut_event(event),
            Msg::ShortcutsSaved(result) => {
                self.model.shortcuts_notice = result
                    .as_ref()
                    .err()
                    .map(enzui_core::persist::PersistError::user_summary);
                self.shortcuts.finish_save(result);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn shortcut_event(&mut self, event: EditableShortcutsEvent) -> Task<Msg> {
        let outcome = match event {
            EditableShortcutsEvent::Capture {
                action,
                event: KeyCapture::Started,
            } => {
                self.model.shortcut_capture = Some(action);
                self.model.shortcut_feedback = None;
                return Task::none();
            }
            EditableShortcutsEvent::Capture { action, event } => {
                // A report from a chip that is no longer recording is stale.
                if self.model.shortcut_capture.as_deref() != Some(action.as_str()) {
                    return Task::none();
                }
                self.model.shortcut_capture = None;
                self.model.shortcut_feedback = None;
                match event {
                    KeyCapture::Candidate(candidate) => self
                        .shortcuts
                        .try_rebind(&action, candidate, keybindings::validate)
                        .map_err(|refusal| (action, refusal.message())),
                    KeyCapture::Cleared => self.reset_shortcut(&action),
                    KeyCapture::Cancelled | KeyCapture::Started => return Task::none(),
                }
            }
            EditableShortcutsEvent::Reset { action } => {
                self.model.shortcut_capture = None;
                self.model.shortcut_feedback = None;
                self.reset_shortcut(&action)
            }
            EditableShortcutsEvent::ResetAll => {
                self.model.shortcut_capture = None;
                self.model.shortcut_feedback = None;
                self.shortcuts.reset_all();
                Ok(())
            }
        };
        if let Err(feedback) = outcome {
            self.model.shortcut_feedback = Some(feedback);
            return Task::none();
        }
        keybindings::install(&self.shortcuts);
        self.save_shortcuts()
    }

    // Restores one action's default, refusing when another action now owns
    // that chord.
    fn reset_shortcut(&mut self, action: &str) -> Result<(), (String, String)> {
        let Some(default) = self
            .shortcuts
            .defaults()
            .iter()
            .find(|binding| binding.id() == action)
            .map(|binding| binding.primary().clone())
        else {
            return Err((
                action.to_owned(),
                RebindRefusal::UnknownAction(action.to_owned()).message(),
            ));
        };
        if let Some(owner) = self
            .shortcuts
            .rows()
            .find(|row| row.id != action && row.overridden && *row.primary == default)
        {
            return Err((
                action.to_owned(),
                RebindRefusal::Conflict {
                    action_id: owner.id.to_owned(),
                    action_name: owner.name.to_owned(),
                }
                .message(),
            ));
        }
        self.shortcuts.reset(action);
        Ok(())
    }

    // Writes the overrides off the UI thread.
    fn save_shortcuts(&mut self) -> Task<Msg> {
        match self.shortcuts.prepare_save(&self.shortcuts_store) {
            Ok(op) => Task::perform(
                crate::shell_work::run(move || op.execute()),
                Msg::ShortcutsSaved,
            ),
            Err(error) => {
                self.model.shortcuts_notice = Some(error.user_summary());
                Task::none()
            }
        }
    }
}
