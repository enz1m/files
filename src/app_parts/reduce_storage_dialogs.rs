// Format and Add a Cloud Drive dialog reducer arms. Both act only through the
// mount service; without it neither dialog can open.

use super::*;
use crate::mount_service::{
    self as service, CloudState, ConnectPlan, FormatPhase, FormatState, ServiceError,
};

impl App {
    // Opens the Format dialog for the service volume `id`, if the service
    // allows formatting it.
    pub(crate) fn show_format(&mut self, id: String) {
        let Some(location) = self.storage_location(&id) else {
            return;
        };
        let Some(item) = location
            .service
            .as_ref()
            .filter(|item| item.can_format && !item.formatting)
        else {
            return;
        };
        let mut state = FormatState::new(id, location.name.clone(), item.filesystem.as_deref());
        state.detail.clone_from(&item.detail);
        self.model.format = Some(state);
    }

    pub(crate) fn reduce_format(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::FormatLabelChanged(text) => {
                if let Some(state) = self.choosing_format() {
                    state.label = text;
                }
                Task::none()
            }
            Msg::FormatFileSystemChosen(file_system) => {
                if let Some(state) = self.choosing_format() {
                    state.file_system = file_system;
                }
                Task::none()
            }
            Msg::SetFormatWholeDrive(whole) => {
                if let Some(state) = self.choosing_format() {
                    state.whole_drive = whole;
                }
                Task::none()
            }
            Msg::CloseFormat => {
                self.model.format = None;
                Task::none()
            }
            Msg::ConfirmFormat => self.start_format(),
            Msg::FormatFinished {
                id,
                name,
                activity,
                result,
            } => {
                self.model.storage_busy.remove(&id);
                let result = result.map_err(|error| match error {
                    // The service names the programs with open files.
                    ServiceError::Busy(message) => message,
                    error => error.to_string(),
                });
                let (outcome, detail) = match &result {
                    Ok(_) => (
                        TerminalOutcome::Completed,
                        crate::i18n::format(
                            "files-name-was-formatted-e5b1c0d7",
                            &[("name", name.clone())],
                        ),
                    ),
                    Err(reason) => (TerminalOutcome::Failed, reason.clone()),
                };
                self.finish_file_activity(
                    activity,
                    usize::from(result.is_ok()),
                    outcome,
                    detail.clone(),
                );
                let open = self
                    .model
                    .format
                    .as_mut()
                    .filter(|state| state.id == id && state.phase == FormatPhase::Formatting);
                match (open, result) {
                    (Some(state), Ok(formatted)) => {
                        state.phase = FormatPhase::Done {
                            id: service::storage_id(&formatted),
                        };
                    }
                    (Some(state), Err(reason)) => {
                        state.phase = FormatPhase::Choosing {
                            error: Some(reason),
                        };
                    }
                    // The dialog was closed meanwhile.
                    (None, Ok(_)) => self.post_status(detail, ToastTone::Success),
                    (None, Err(reason)) => self.post_status(
                        crate::i18n::format(
                            "files-name-was-not-formatted-error-2a9f4c61",
                            &[("name", name), ("error", reason)],
                        ),
                        ToastTone::Error,
                    ),
                }
                Task::none()
            }
            Msg::OpenFormatted => {
                let Some(FormatState {
                    phase: FormatPhase::Done { id },
                    ..
                }) = self.model.format.take()
                else {
                    return Task::none();
                };
                // The new volume may not be listed yet; the service mounts it
                // by identity either way.
                match self.storage_location(&id) {
                    Some(location) if location.mount.is_some() => {
                        self.reduce_storage_operation(Msg::OpenStorage(id))
                    }
                    _ => self.service_mount_planned(id, ConnectPlan::default()),
                }
            }
            _ => unreachable!("misrouted message in reduce_format"),
        }
    }

    // The Format dialog while its options can still change.
    fn choosing_format(&mut self) -> Option<&mut FormatState> {
        self.model
            .format
            .as_mut()
            .filter(|state| matches!(state.phase, FormatPhase::Choosing { .. }))
    }

    fn start_format(&mut self) -> Task<Msg> {
        let Some(state) = self.choosing_format() else {
            return Task::none();
        };
        if state.label_problem().is_some() {
            return Task::none();
        }
        let Some(target) = service::service_id(&state.id).map(str::to_owned) else {
            return Task::none();
        };
        let (id, name, request) = (state.id.clone(), state.name.clone(), state.request());
        if !self.model.storage_busy.insert(id.clone()) {
            return Task::none();
        }
        if let Some(state) = self.model.format.as_mut() {
            state.phase = FormatPhase::Formatting;
        }
        let activity = self.start_service_activity(
            ActivityKind::Other("Format"),
            crate::i18n::format("files-formatting-name-3b7e90a2", &[("name", name.clone())]),
        );
        Task::perform(service::format(target, request), move |result| {
            Msg::FormatFinished {
                id,
                name,
                activity,
                result,
            }
        })
    }

    pub(crate) fn reduce_cloud(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::ShowAddCloud => {
                // A sign-in already open is followed rather than restarted.
                self.model.cloud = Some(
                    self.model
                        .storage_locations
                        .iter()
                        .find(|location| {
                            location
                                .service
                                .as_ref()
                                .is_some_and(|service| service.signing_in)
                        })
                        .map_or_else(CloudState::new, |location| CloudState::SigningIn {
                            id: location.id.clone(),
                            service: location.name.clone(),
                            listed: true,
                        }),
                );
                Task::none()
            }
            Msg::CloudProviderChosen(provider) => {
                if !matches!(self.model.cloud, Some(CloudState::Choose { .. })) {
                    return Task::none();
                }
                if provider == service::CloudProvider::Nextcloud {
                    self.model.cloud = Some(CloudState::Server {
                        server: String::new(),
                        error: None,
                    });
                    return Task::none();
                }
                self.start_cloud(provider, String::new())
            }
            Msg::CloudServerChanged(text) => {
                if let Some(CloudState::Server { server, error }) = &mut self.model.cloud {
                    *server = text;
                    *error = None;
                }
                Task::none()
            }
            Msg::ConfirmCloudServer => {
                let Some(CloudState::Server { server, .. }) = &self.model.cloud else {
                    return Task::none();
                };
                let server = server.trim().to_owned();
                if server.is_empty() {
                    return Task::none();
                }
                self.start_cloud(service::CloudProvider::Nextcloud, server)
            }
            Msg::CloudBack => {
                if matches!(self.model.cloud, Some(CloudState::Server { .. })) {
                    self.model.cloud = Some(CloudState::new());
                }
                Task::none()
            }
            Msg::CloudAdded { provider, result } => {
                let server = match &self.model.cloud {
                    Some(CloudState::Starting {
                        provider: started,
                        server,
                    }) if *started == provider => server.clone(),
                    // The dialog was closed; the sidebar shows the sign-in.
                    _ => {
                        if let Err(error) = result {
                            self.post_status(error.to_string(), ToastTone::Error);
                        }
                        return Task::none();
                    }
                };
                self.model.cloud = Some(match result {
                    Ok(id) => {
                        let mut state = CloudState::SigningIn {
                            id: service::storage_id(&id),
                            service: provider.name().to_owned(),
                            listed: false,
                        };
                        state.follow(&self.model.storage_locations);
                        state
                    }
                    Err(error) if provider == service::CloudProvider::Nextcloud => {
                        CloudState::Server {
                            server,
                            error: Some(error.to_string()),
                        }
                    }
                    Err(error) => CloudState::Choose {
                        error: Some(error.to_string()),
                    },
                });
                Task::none()
            }
            Msg::CancelCloudSignIn => {
                let Some(CloudState::SigningIn { id, .. }) = self.model.cloud.take() else {
                    return Task::none();
                };
                self.reduce_location(Msg::LocationAction(
                    id,
                    service::LocationAction::CancelSignIn,
                ))
            }
            Msg::OpenCloudDrive => {
                let Some(CloudState::Done { id, .. }) = self.model.cloud.take() else {
                    return Task::none();
                };
                self.reduce_storage_operation(Msg::OpenStorage(id))
            }
            Msg::CloseCloud => {
                self.model.cloud = None;
                Task::none()
            }
            _ => unreachable!("misrouted message in reduce_cloud"),
        }
    }

    fn start_cloud(&mut self, provider: service::CloudProvider, server: String) -> Task<Msg> {
        self.model.cloud = Some(CloudState::Starting {
            provider,
            server: server.clone(),
        });
        Task::perform(service::add_cloud(provider, server), move |result| {
            Msg::CloudAdded { provider, result }
        })
    }

    // Moves the Add a Cloud Drive dialog along as its sign-in finishes.
    pub(crate) fn follow_cloud_sign_in(&mut self) {
        if let Some(state) = self.model.cloud.as_mut() {
            state.follow(&self.model.storage_locations);
        }
    }
}
