// Ordinary desktop launches use the same application identity as the window.

use crate::{App, Msg};
use enzui_core::activation::ApplicationIntent;
use enzui_widgets::single_instance::SingleInstance;
use iced::Task;
use std::time::Instant;

pub(crate) type DesktopLaunch = SingleInstance;

#[cfg(not(feature = "center"))]
pub(crate) enum Claim {
    Owner(DesktopLaunch),
    Forwarded,
}

#[cfg(not(feature = "center"))]
pub(crate) fn claim() -> Result<Claim, String> {
    use enzui_widgets::single_instance::InstanceStart;
    match SingleInstance::start(crate::APP_ID) {
        InstanceStart::Forwarded => Ok(Claim::Forwarded),
        InstanceStart::Owner(instance, _) if instance.is_shared() => Ok(Claim::Owner(instance)),
        InstanceStart::Owner(_, _) => {
            Err(crate::i18n::tr("Files could not establish single-instance ownership.").to_owned())
        }
    }
}

impl App {
    pub(crate) fn desktop_launch_wake(&mut self) -> Task<Msg> {
        // Keep queued intents and activation contexts in the shared service until a target exists.
        let Some(window) = self.main_window else {
            return Task::none();
        };
        let Some(owner) = self.desktop_launch.as_mut() else {
            return Task::none();
        };
        let (intents, raise) = owner.drain(Some(window));
        let mut tasks = vec![raise];
        for intent in intents {
            if let ApplicationIntent::Open(uris) = intent {
                tasks.push(self.open_forwarded(&uris));
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn route_desktop_calls(
        &mut self,
        calls: Vec<enzui_core::activation::dbus::ApplicationCall>,
    ) -> Task<Msg> {
        if self.close_pending.is_some() {
            return Task::none();
        }
        if self.main_window.is_none() {
            let available = enzui_core::activation::dbus::APPLICATION_CALL_CAPACITY
                .saturating_sub(self.pending_launches.len());
            self.pending_launches
                .extend(calls.into_iter().take(available));
            return Task::none();
        }
        let mut tasks = Vec::new();
        for call in calls {
            if let ApplicationIntent::Open(uris) = call.intent {
                tasks.push(self.open_forwarded(&uris));
            }
            if let (Some(window), Some(context)) = (self.main_window, call.context) {
                tasks.push(
                    context
                        .activate(window, Instant::now())
                        .map(|_| Msg::DesktopLaunchActivated),
                );
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn pending_desktop_launches(&mut self) -> Task<Msg> {
        let calls = std::mem::take(&mut self.pending_launches);
        Task::batch([self.route_desktop_calls(calls), self.desktop_launch_wake()])
    }

    pub(crate) fn open_forwarded(&mut self, uris: &[String]) -> Task<Msg> {
        let arguments: Vec<_> = uris.iter().map(std::ffi::OsString::from).collect();
        if let Some(package) = crate::startup::first_extension_package(&arguments) {
            return self.update(Msg::ReviewExtension(package));
        }
        if let Some(archive) = crate::startup::first_archive(&arguments) {
            return self.update(Msg::OpenArchiveExternally(archive));
        }
        match crate::startup::first_local_directory(arguments) {
            Some(path) => self.update(Msg::NavigateTo(path)),
            None => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::activation::dbus::ApplicationCall;

    fn call(intent: ApplicationIntent) -> ApplicationCall {
        ApplicationCall {
            intent,
            parameters: Vec::new(),
            context: None,
            context_error: None,
        }
    }

    #[test]
    fn forwarded_remote_uris_never_become_local_paths() {
        let (mut app, _) = App::new();
        app.main_window = Some(iced::window::Id::unique());
        let before = app.model.cwd.clone();
        let task = app.route_desktop_calls(vec![call(ApplicationIntent::Open(vec![
            "smb://host/share".into(),
            "file://elsewhere/private".into(),
            "file:///tmp/a?query".into(),
        ]))]);
        crate::file_clipboard_tests::drive(&mut app, task);
        assert_eq!(app.model.cwd, before);
        assert!(app.pending_launches.is_empty());
    }
}
