// Mount service reducer arms: network connects, saved network locations, and
// the password, host-key, share, SMB1, busy and passphrase prompts around
// storage operations.

use super::*;
use crate::mount_service::{
    self as service, AfterForce, Confirmation, ConnectPlan, ConnectionTiming, LocationAction,
    LocationDialog, Login, ServiceError,
};
use crate::network::{AskedQuestion, MountAnswer, MountQuestion, TargetPurpose};

fn finished(
    id: String,
    name: String,
    opening: bool,
    activity: crate::activity::FileActivity,
    plan: Option<ConnectPlan>,
    result: Result<Option<PathBuf>, ServiceError>,
) -> Msg {
    Msg::MountServiceFinished {
        id,
        name,
        opening,
        activity,
        result,
        plan,
    }
}

fn service_id(storage_id: &str) -> String {
    service::service_id(storage_id)
        .unwrap_or_default()
        .to_owned()
}

impl App {
    // Connect to Server through the mount service: answer a password request
    // for an existing location, or save and mount a new address.
    pub(crate) fn service_confirm_attach(&mut self) -> Task<Msg> {
        let Some(attach) = self.model.attach.as_ref() else {
            return Task::none();
        };
        let password_missing = !attach.password_requirement.allows_empty()
            && attach.password.expose_secret().trim().is_empty();
        let address = attach.service_address();
        if attach
            .service_target
            .as_ref()
            .is_some_and(|target| target.carry.change_account)
            && attach.username.trim().is_empty()
        {
            return Task::none();
        }
        if password_missing {
            self.post_status(crate::i18n::tr("Password required"), ToastTone::Warning);
            return Task::none();
        }
        if attach.service_target.is_none() && address.is_none() {
            self.post_status(
                crate::i18n::tr("Enter a server or an address"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        let Some(attach) = self.model.attach.take() else {
            return Task::none();
        };
        let login = Login {
            username: attach.username.trim().to_owned(),
            domain: attach.domain.trim().to_owned(),
            password: attach.password.clone(),
        };
        let login = (!login.username.is_empty() || !login.password.is_empty()).then_some(login);
        match (attach.service_target, address) {
            (Some(target), _) => {
                let mut plan = target.carry;
                plan.remember = attach.remember_password && target.can_remember;
                match target.purpose {
                    TargetPurpose::Mount => {
                        plan.login = login;
                        let leave = if plan.change_account
                            && self
                                .storage_location(&target.id)
                                .is_some_and(|location| location.contains(&self.model.cwd))
                        {
                            self.nav_navigate_screen(Msg::NavigateScreen(
                                Screen::Network,
                                PathBuf::from(services::NETWORK_VIEW_ROOT),
                            ))
                        } else {
                            Task::none()
                        };
                        Task::batch([leave, self.service_mount_planned(target.id, plan)])
                    }
                    TargetPurpose::MountAtBoot => {
                        let password = login.map(|login| login.password);
                        let enable =
                            service::set_mount_at_boot(service_id(&target.id), true, password);
                        self.location_update(
                            target.id,
                            LocationAction::SetMountAtBoot(true),
                            enable,
                        )
                    }
                }
            }
            (None, Some(address)) => self.service_connect(
                address,
                ConnectPlan {
                    login,
                    remember: attach.remember_password,
                    ..ConnectPlan::default()
                },
            ),
            (None, None) => Task::none(),
        }
    }

    // Saves the network location `address` with the service, then mounts it.
    pub(crate) fn service_connect(&mut self, address: String, plan: ConnectPlan) -> Task<Msg> {
        if self.model.network.opening.is_some() {
            return Task::none();
        }
        if plan.login.is_none()
            && let Some((server, None, path)) = service::smb_authority(&address)
            && !path.is_empty()
            && let Some(id) = self.model.storage_locations.iter().find_map(|location| {
                let saved = location.service.as_ref()?.uri.as_deref()?;
                let (known_server, _, known_path) = service::smb_authority(saved)?;
                let decode = percent_encoding::percent_decode_str;
                (known_server.eq_ignore_ascii_case(server) && decode(known_path).eq(decode(path)))
                    .then(|| location.id.clone())
            })
        {
            return self.reduce_storage_operation(Msg::OpenStorage(id));
        }
        let activity = self.start_service_activity(
            ActivityKind::Other("Mounting"),
            crate::i18n::format("files-mounting-uri-b9db6582", &[("uri", address.clone())]),
        );
        // Until the location is listed, its address names it.
        let name = address.clone();
        self.model.network.opening = Some(name.clone());
        let carried = plan.clone();
        let known: std::collections::HashSet<String> = self
            .model
            .storage_locations
            .iter()
            .map(|location| location.id.clone())
            .collect();
        let connect = async move {
            let id = match service::add_location(address).await {
                Ok(id) => id,
                Err(error) => return (String::new(), false, Err(error)),
            };
            let id_text = service::storage_id(&id);
            let created = !known.contains(&id_text);
            (
                id_text,
                created,
                service::mount_planned(id, plan).await.map(Some),
            )
        };
        Task::perform(connect, move |(id, created, result)| {
            let plan = ConnectPlan {
                new_location: created,
                ..carried.clone()
            };
            finished(id, name.clone(), true, activity, Some(plan), result)
        })
    }

    pub(crate) fn service_mount_planned(&mut self, id: String, plan: ConnectPlan) -> Task<Msg> {
        let name = self.storage_name(&id);
        self.model.storage_busy.insert(id.clone());
        let activity = self.start_service_activity(
            ActivityKind::Other("Mount"),
            crate::i18n::format("files-mounting-name-a9625493", &[("name", name.clone())]),
        );
        let carried = plan.clone();
        let target = service_id(&id);
        let mount = async move {
            if plan.change_account {
                service::unmount(target.clone(), false).await?;
            }
            service::mount_planned(target, plan).await
        };
        Task::perform(mount, move |result| {
            finished(id, name, true, activity, Some(carried), result.map(Some))
        })
    }

    // A listed location's name, otherwise the host of the address it was
    // started from.
    fn storage_name_or_host(&self, id: &str, fallback: &str) -> String {
        self.storage_location(id).map_or_else(
            || {
                let rest = fallback
                    .split_once("://")
                    .map_or(fallback, |(_, rest)| rest);
                let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
                rest.split(['/', '\\'])
                    .find(|part| !part.is_empty())
                    .unwrap_or(fallback)
                    .to_owned()
            },
            |location| location.name.clone(),
        )
    }

    pub(crate) fn close_location_dialog(&mut self) -> Task<Msg> {
        self.model.location_dialog = None;
        Task::none()
    }

    fn storage_name(&self, id: &str) -> String {
        self.storage_location(id)
            .map_or_else(|| id.to_owned(), |location| location.name.clone())
    }

    pub(crate) fn storage_location(&self, id: &str) -> Option<&crate::storage::Location> {
        self.model
            .storage_locations
            .iter()
            .find(|location| location.id == id)
    }

    fn storage_service(&self, id: &str) -> Option<&crate::desktop_storage::ServiceItem> {
        self.storage_location(id)
            .and_then(|location| location.service.as_ref())
    }

    // Opens the attach form to answer a password request for `id`.
    fn ask_credentials(
        &mut self,
        id: String,
        name: &str,
        request: &service::CredentialRequest,
        purpose: TargetPurpose,
        carry: ConnectPlan,
    ) {
        let uri = self
            .storage_service(&id)
            .and_then(|service| service.uri.clone())
            .or_else(|| name.contains("://").then(|| name.to_owned()));
        self.next_attach_generation = self.next_attach_generation.saturating_add(1);
        // Until the location is listed, its address names it.
        let name = self
            .storage_location(&id)
            .map_or_else(|| name.to_owned(), |location| location.name.clone());
        self.model.attach = Some(service::credential_form(
            id,
            &name,
            uri.as_deref(),
            request,
            self.next_attach_generation,
            purpose,
            ConnectPlan {
                login: None,
                ..carry
            },
        ));
    }

    pub(crate) fn reduce_mount_service_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MountServiceFinished {
            id,
            name,
            opening,
            activity,
            result,
            plan,
        } = message
        else {
            unreachable!("misrouted message in reduce_mount_service_finished");
        };
        if self.model.network.opening.as_deref() == Some(name.as_str()) {
            self.model.network.opening = None;
        }
        let settle = |app: &mut Self, outcome: TerminalOutcome, detail: &str| {
            app.model.storage_busy.remove(&id);
            app.finish_file_activity(activity, 0, outcome, detail.to_owned());
        };
        // The operation waits for the person: no activity row is left behind.
        let pause = |app: &mut Self| {
            app.model.storage_busy.remove(&id);
            app.drop_file_activity(activity);
        };
        let locked = self
            .storage_service(&id)
            .is_some_and(|service| service.locked)
            || self
                .model
                .unlock
                .as_ref()
                .is_some_and(|state| state.id == id);
        match result {
            Err(ServiceError::NeedsPassphrase) => {
                pause(self);
                self.model.unlock = Some(service::UnlockState::new(id, name));
                iced::widget::operation::focus(crate::dialog::unlock_passphrase_id())
            }
            Err(ServiceError::NeedsCredentials(request)) => {
                pause(self);
                let target = if request.id.is_empty() {
                    id.clone()
                } else {
                    service::storage_id(&request.id)
                };
                self.ask_credentials(
                    target,
                    &name,
                    &request,
                    TargetPurpose::Mount,
                    plan.unwrap_or_default(),
                );
                Task::none()
            }
            Err(ServiceError::HostKey(request)) => {
                // The operation stays busy while the person compares fingerprints.
                let values = [
                    ("host", request.host.clone()),
                    ("fingerprints", request.fingerprints.join("\n")),
                ];
                let text = if request.changed {
                    crate::i18n::format("files-the-key-of-host-has-changed-6e0f53d2", &values)
                } else {
                    crate::i18n::format("files-the-key-of-host-is-not-known-2c94a1f5", &values)
                };
                // A changed key is the impersonation case: it is not offered
                // with the same one-click trust as a first contact.
                let choices = [
                    crate::i18n::tr("Cancel").to_owned(),
                    if request.changed {
                        crate::i18n::tr("Replace Key and Connect").to_owned()
                    } else {
                        crate::i18n::tr("Trust and Connect").to_owned()
                    },
                ];
                let question = MountQuestion::sanitized(&text, &choices).map(|question| {
                    if request.changed {
                        question.warning()
                    } else {
                        question
                    }
                });
                let Some(question) = question else {
                    settle(
                        self,
                        TerminalOutcome::Cancelled,
                        crate::i18n::tr("Cancelled"),
                    );
                    return Task::none();
                };
                let (asked, answer) = AskedQuestion::asking(question);
                if let Some(previous) = self.model.mount_question.replace(asked) {
                    previous.answer(MountAnswer::Decline);
                }
                self.model.storage_busy.insert(id.clone());
                let target = if request.id.is_empty() {
                    service_id(&id)
                } else {
                    request.id
                };
                let fingerprints = request.fingerprints;
                // The retry keeps the password and choices already given.
                let plan = plan.unwrap_or_default();
                let carried = plan.clone();
                let trust = async move {
                    if answer.await != MountAnswer::Choice(1) {
                        return Err(ServiceError::Cancelled);
                    }
                    service::trust_host_key(target.clone(), fingerprints).await?;
                    service::mount_planned(target, plan).await.map(Some)
                };
                Task::perform(trust, move |result| {
                    finished(id, name, opening, activity, Some(carried), result)
                })
            }
            Err(ServiceError::ChooseShare(request)) => {
                pause(self);
                let server = if request.id.is_empty() {
                    service_id(&id)
                } else {
                    request.id
                };
                let plan = plan.unwrap_or_default();
                let cleanup = if plan.new_location {
                    Task::perform(service::remove_location(server.clone()), |_| Msg::Noop)
                } else {
                    Task::none()
                };
                let server = service::storage_id(&server);
                let path = network::server_path(&server);
                let name = self.storage_name_or_host(&server, &name);
                self.model.network.servers.insert(
                    path.clone(),
                    network::ServerPage {
                        name,
                        shares: request.shares,
                        plan,
                    },
                );
                self.model.device_row_selected = None;
                self.model.last_device_press = None;
                let browse = self.nav_navigate_screen(Msg::NavigateScreen(Screen::Network, path));
                Task::batch([cleanup, browse])
            }
            Err(ServiceError::ProtocolMismatch(_)) => {
                pause(self);
                let name = self.storage_name_or_host(&id, &name);
                self.model.location_dialog = Some(LocationDialog::Confirm {
                    id,
                    name,
                    confirmation: Confirmation::Smb1 {
                        plan: plan.unwrap_or_default(),
                    },
                });
                Task::none()
            }
            Err(ServiceError::Busy(message)) if !opening => {
                settle(self, TerminalOutcome::Failed, &message);
                self.model.location_dialog = Some(LocationDialog::Confirm {
                    id,
                    name,
                    confirmation: Confirmation::Force {
                        message,
                        then: AfterForce::Nothing,
                    },
                });
                Task::none()
            }
            Err(ServiceError::Cancelled) => {
                settle(
                    self,
                    TerminalOutcome::Cancelled,
                    crate::i18n::tr("Cancelled"),
                );
                Task::none()
            }
            Err(ServiceError::AuthFailed(reason)) if locked => {
                // A wrong passphrase: the dialog asks again.
                pause(self);
                match self.model.unlock.as_mut().filter(|state| state.id == id) {
                    Some(state) => {
                        state.busy = false;
                        state.error = Some(reason);
                        state.passphrase = enzui_core::form::SecretText::new(String::new());
                    }
                    None => {
                        let mut state = service::UnlockState::new(id, name);
                        state.error = Some(reason);
                        self.model.unlock = Some(state);
                    }
                }
                iced::widget::operation::focus(crate::dialog::unlock_passphrase_id())
            }
            Err(ServiceError::AuthFailed(_))
                if plan.as_ref().is_some_and(|plan| plan.login.is_some()) =>
            {
                // A typed password was refused: ask again, keeping the name.
                pause(self);
                let mut plan = plan.unwrap_or_default();
                let login = plan.login.take();
                let request = service::CredentialRequest {
                    id: service_id(&id),
                    username: login
                        .as_ref()
                        .map(|l| l.username.clone())
                        .unwrap_or_default(),
                    domain: login.map(|l| l.domain).unwrap_or_default(),
                    can_remember: true,
                    retry: true,
                };
                self.ask_credentials(id, &name, &request, TargetPurpose::Mount, plan);
                Task::none()
            }
            Err(error) => {
                // The notice names the location and says what failed in the
                // service's own words.
                if self
                    .model
                    .unlock
                    .as_ref()
                    .is_some_and(|state| state.id == id)
                {
                    self.model.unlock = None;
                }
                let network = self.storage_location(&id).is_some_and(|location| {
                    matches!(
                        location.kind,
                        crate::storage::Kind::Network | crate::storage::Kind::Cloud
                    )
                }) || name.contains("://");
                let key = match (opening, network) {
                    (true, true) => "files-could-not-connect-to-name-error",
                    (true, false) => "files-could-not-open-name-error",
                    (false, true) => "files-could-not-disconnect-name-error",
                    (false, false) => "files-could-not-unmount-name-error",
                };
                let name = self.storage_name_or_host(&id, &name);
                let detail =
                    crate::i18n::format(key, &[("name", name), ("error", error.to_string())]);
                settle(self, TerminalOutcome::Failed, &detail);
                self.post_status(detail, ToastTone::Error);
                Task::perform(load_drives(), Msg::DrivesLoaded)
            }
            result => {
                // Unlocked, or another success the notice reports.
                if self
                    .model
                    .unlock
                    .as_ref()
                    .is_some_and(|state| state.id == id)
                {
                    self.model.unlock = None;
                }
                self.reduce_storage_finished(Msg::StorageOperationFinished {
                    id,
                    name,
                    opening,
                    activity,
                    result: result.map_err(|error| error.to_string()),
                })
            }
        }
    }

    // Runs a change to a saved location; its outcome arrives as
    // `LocationUpdated`.
    fn location_update(
        &mut self,
        id: String,
        action: LocationAction,
        change: impl std::future::Future<Output = Result<(), ServiceError>> + Send + 'static,
    ) -> Task<Msg> {
        // Named now: a removed location is gone from the inventory by the
        // time its outcome arrives.
        let name = self.storage_name(&id);
        Task::perform(change, move |result| Msg::LocationUpdated {
            id,
            name,
            action,
            result,
        })
    }

    pub(crate) fn reduce_location(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::OpenLocationContextMenu(id, at) => {
                self.model.context_menu = self
                    .storage_location(&id)
                    .filter(|location| location.service.is_some())
                    .map(context_menu::location_menu_model)
                    .and_then(|items| context_menu::OpenContextMenu::location(&id, at, items));
                Task::none()
            }
            Msg::ShowLocationMenu(id) => iced::advanced::widget::operate(
                enzui_widgets::context_menu_action::show_context_menu(
                    crate::view::storage::location_menu_target_id(&id),
                ),
            ),
            Msg::DeviceRowPressed { key, open } => {
                // As in the file list: one click selects, a second on the same
                // row within the multi-click interval opens it.
                let again = self
                    .model
                    .last_device_press
                    .as_ref()
                    .is_some_and(|(last, at)| {
                        *last == key
                            && at.elapsed() <= enzui_core::click::DEFAULT_MULTI_CLICK_INTERVAL
                    });
                if again || self.model.single_click_open {
                    self.model.last_device_press = None;
                    self.model.device_row_selected = Some(key);
                    return self.update(*open);
                }
                self.model.last_device_press = Some((key.clone(), std::time::Instant::now()));
                self.model.device_row_selected = Some(key);
                Task::none()
            }
            Msg::LocationAction(id, action) => self.location_action(id, action),
            Msg::LocationUpdated {
                id,
                name,
                action,
                result,
            } => {
                self.location_updated(&id, name, action, result);
                Task::perform(load_drives(), Msg::DrivesLoaded)
            }
            Msg::LocationRenameChanged(text) => {
                if let Some(LocationDialog::Rename { name, .. }) = &mut self.model.location_dialog {
                    *name = text;
                }
                Task::none()
            }
            Msg::LocationShareChosen(index) => {
                if self.model.network.opening.is_some() {
                    return Task::none();
                }
                let Some(page) = self.model.network.servers.get_mut(&self.model.cwd) else {
                    return Task::none();
                };
                let Some(share) = page.shares.get(index) else {
                    return Task::none();
                };
                let plan = ConnectPlan {
                    login: page.plan.login.take(),
                    ..page.plan.clone()
                };
                let uri = share.uri.clone();
                self.service_connect(uri, plan)
            }
            Msg::ConfirmLocationDialog => self.confirm_location_dialog(),
            Msg::CloseLocationDialog => self.close_location_dialog(),
            _ => unreachable!("misrouted message in reduce_location"),
        }
    }

    fn change_share_account(&mut self, id: String) -> Task<Msg> {
        let Some(location) = self.storage_location(&id) else {
            return Task::none();
        };
        let Some(item) = location
            .service
            .as_ref()
            .filter(|item| !item.mount_at_boot && !item.busy)
        else {
            return Task::none();
        };
        if self.model.storage_busy.contains(&id) {
            return Task::none();
        }
        let Some((username, domain)) = item.uri.as_deref().and_then(service::smb_account) else {
            return Task::none();
        };
        let name = location.name.clone();
        let request = service::CredentialRequest {
            id: service_id(&id),
            username,
            domain,
            can_remember: item.can_remember_password,
            retry: false,
        };
        self.model.location_dialog = None;
        self.ask_credentials(
            id,
            &name,
            &request,
            TargetPurpose::Mount,
            ConnectPlan {
                change_account: true,
                ..ConnectPlan::default()
            },
        );
        Task::none()
    }

    fn set_share_connection_timing(&mut self, id: String, timing: ConnectionTiming) -> Task<Msg> {
        let Some(item) = self.storage_service(&id) else {
            return Task::none();
        };
        if item.busy
            || self.model.storage_busy.contains(&id)
            || ConnectionTiming::current(item) == timing
        {
            return Task::none();
        }
        let target = service_id(&id);
        let was_at_startup = item.mount_at_boot;
        let auto = timing == ConnectionTiming::AtSignIn;
        let action = if timing == ConnectionTiming::AtStartup {
            if !item.can_mount_at_boot {
                return Task::none();
            }
            LocationAction::SetMountAtBoot(true)
        } else {
            LocationAction::SetAutoConnect(auto)
        };
        self.model.storage_busy.insert(id.clone());
        let change = async move {
            if timing == ConnectionTiming::AtStartup {
                return service::set_mount_at_boot(target, true, None).await;
            }
            if was_at_startup {
                service::set_mount_at_boot(target.clone(), false, None).await?;
            }
            service::update_location(target, service::LocationUpdate::AutoConnect(auto)).await
        };
        self.location_update(id, action, change)
    }

    fn location_action(&mut self, id: String, action: LocationAction) -> Task<Msg> {
        self.model.context_menu = None;
        let target = service_id(&id);
        match action {
            // A locked drive's Open asks for its passphrase.
            LocationAction::Open | LocationAction::Connect | LocationAction::Unlock => {
                self.reduce_storage_operation(Msg::OpenStorage(id))
            }
            LocationAction::Unmount => self.reduce_storage_operation(Msg::UnmountStorage(id)),
            LocationAction::SafelyRemove => {
                let power_off = self
                    .storage_service(&id)
                    .is_some_and(|service| service.can_power_off);
                self.model.storage_busy.insert(id.clone());
                self.location_update(id, action, service::remove_drive(target, power_off))
            }
            LocationAction::ShowSignIn => self.reduce_cloud(Msg::ShowAddCloud),
            LocationAction::Format => {
                self.show_format(id);
                Task::none()
            }
            LocationAction::CancelSignIn => {
                self.location_update(id, action, service::remove_location(target))
            }
            // A location that stopped answering is detached by force.
            LocationAction::Disconnect | LocationAction::ForceDisconnect => {
                self.reduce_storage_operation(Msg::UnmountStorage(id))
            }
            LocationAction::Rename => {
                let name = self.storage_name(&id);
                self.model.location_dialog = Some(LocationDialog::Rename { id, name });
                Task::none()
            }
            LocationAction::ShareSettings => {
                self.model.location_dialog = Some(LocationDialog::ShareSettings { id });
                Task::none()
            }
            LocationAction::ChangeAccount => self.change_share_account(id),
            LocationAction::SetConnectionTiming(timing) => {
                self.set_share_connection_timing(id, timing)
            }
            LocationAction::SetAutoConnect(enabled) => {
                let change =
                    service::update_location(target, service::LocationUpdate::AutoConnect(enabled));
                self.location_update(id, action, change)
            }
            LocationAction::SetMountAtBoot(enabled) => {
                let change = service::set_mount_at_boot(target, enabled, None);
                self.location_update(id, action, change)
            }
            LocationAction::ForgetPassword => {
                let change =
                    service::update_location(target, service::LocationUpdate::ForgetPassword);
                self.location_update(id, action, change)
            }
            LocationAction::Remove => {
                let name = self.storage_name(&id);
                self.model.location_dialog = Some(LocationDialog::Confirm {
                    id,
                    name,
                    confirmation: Confirmation::Remove,
                });
                Task::none()
            }
        }
    }

    fn location_updated(
        &mut self,
        id: &str,
        name: String,
        action: LocationAction,
        result: Result<(), ServiceError>,
    ) {
        self.model.storage_busy.remove(id);
        match result {
            Ok(()) => {
                let key = match action {
                    LocationAction::SafelyRemove => "files-name-can-be-unplugged",
                    LocationAction::SetAutoConnect(true) => {
                        "files-name-reconnects-when-you-log-in-1c7a4d90"
                    }
                    LocationAction::SetAutoConnect(false) => {
                        "files-name-no-longer-reconnects-when-you-log-e2f06b3d"
                    }
                    LocationAction::SetMountAtBoot(true) => {
                        "files-name-is-mounted-at-startup-9a41c5e8"
                    }
                    LocationAction::SetMountAtBoot(false) => {
                        "files-name-is-no-longer-mounted-at-startup-37d2b8f1"
                    }
                    LocationAction::ForgetPassword => {
                        "files-the-password-for-name-is-forgotten-6c9e0a14"
                    }
                    LocationAction::Remove => "files-name-was-removed-d84f2b67",
                    LocationAction::CancelSignIn => {
                        "files-the-sign-in-to-name-was-cancelled-58c3e1a7"
                    }
                    LocationAction::Rename => "files-name-was-renamed-4e1a9c3b",
                    // Reported by the storage operation they start.
                    LocationAction::Open
                    | LocationAction::Unlock
                    | LocationAction::Unmount
                    | LocationAction::Format
                    | LocationAction::ShowSignIn
                    | LocationAction::Connect
                    | LocationAction::ShareSettings
                    | LocationAction::ChangeAccount
                    | LocationAction::SetConnectionTiming(_)
                    | LocationAction::Disconnect
                    | LocationAction::ForceDisconnect => return,
                };
                self.post_status(
                    crate::i18n::format(key, &[("name", name)]),
                    ToastTone::Success,
                );
            }
            Err(ServiceError::NeedsCredentials(request))
                if action == LocationAction::SetMountAtBoot(true) =>
            {
                self.model.location_dialog = None;
                self.ask_credentials(
                    id.to_owned(),
                    &name,
                    &request,
                    TargetPurpose::MountAtBoot,
                    ConnectPlan::default(),
                );
            }
            Err(ServiceError::Busy(message)) if action == LocationAction::SafelyRemove => {
                let power_off = self
                    .storage_service(id)
                    .is_none_or(|service| service.can_power_off);
                self.model.location_dialog = Some(LocationDialog::Confirm {
                    id: id.to_owned(),
                    name,
                    confirmation: Confirmation::Force {
                        message,
                        then: if power_off {
                            AfterForce::PowerOff
                        } else {
                            AfterForce::Eject
                        },
                    },
                });
            }
            Err(ServiceError::Cancelled) => {}
            Err(error) if action == LocationAction::SafelyRemove => self.post_status(
                crate::i18n::format(
                    "files-name-could-not-be-removed-error",
                    &[("name", name), ("error", error.to_string())],
                ),
                ToastTone::Error,
            ),
            Err(error) => self.post_status(
                crate::i18n::format(
                    "files-could-not-change-name-error-71b3e0d5",
                    &[("name", name), ("error", error.to_string())],
                ),
                ToastTone::Error,
            ),
        }
    }

    fn confirm_location_dialog(&mut self) -> Task<Msg> {
        let Some(dialog) = self.model.location_dialog.take() else {
            return Task::none();
        };
        match dialog {
            LocationDialog::Rename { id, name } => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    self.model.location_dialog = Some(LocationDialog::Rename { id, name });
                    return Task::none();
                }
                let change =
                    service::update_location(service_id(&id), service::LocationUpdate::Label(name));
                self.location_update(id, LocationAction::Rename, change)
            }
            LocationDialog::ShareSettings { .. } => Task::none(),
            LocationDialog::Confirm {
                id,
                name,
                confirmation,
            } => {
                let target = service_id(&id);
                match confirmation {
                    Confirmation::Remove => self.location_update(
                        id,
                        LocationAction::Remove,
                        service::remove_location(target),
                    ),
                    Confirmation::Smb1 { plan } => {
                        self.model.storage_busy.insert(id.clone());
                        let activity = self.start_service_activity(
                            ActivityKind::Other("Mount"),
                            crate::i18n::format(
                                "files-mounting-name-a9625493",
                                &[("name", name.clone())],
                            ),
                        );
                        let carried = plan.clone();
                        let retry = async move {
                            service::update_location(target.clone(), service::LocationUpdate::Smb1)
                                .await?;
                            service::mount_planned(target, plan).await.map(Some)
                        };
                        Task::perform(retry, move |result| {
                            finished(id, name, true, activity, Some(carried), result)
                        })
                    }
                    Confirmation::Force { then, .. } => {
                        self.model.storage_busy.insert(id.clone());
                        let activity = self.start_service_activity(
                            ActivityKind::Unmount,
                            crate::i18n::format(
                                "files-disconnecting-name",
                                &[("name", name.clone())],
                            ),
                        );
                        let detach = async move {
                            service::unmount(target.clone(), true).await?;
                            match then {
                                AfterForce::Nothing => {}
                                AfterForce::Eject => service::remove_drive(target, false).await?,
                                AfterForce::PowerOff => service::remove_drive(target, true).await?,
                            }
                            Ok(None)
                        };
                        Task::perform(detach, move |result| {
                            finished(id, name, false, activity, None, result)
                        })
                    }
                }
            }
        }
    }

    // A drive the service refused to remove because files are open: offer to
    // detach it anyway, then finish the removal.
    pub(crate) fn offer_forced_removal(
        &mut self,
        device_id: &str,
        op: crate::devices::DeviceOp,
        message: String,
    ) {
        use crate::devices::DeviceOp;
        let Some(device) = self
            .model
            .removable_devices
            .iter()
            .find(|device| device.id == device_id)
        else {
            return;
        };
        let Some(location) = self
            .model
            .storage_locations
            .iter()
            .find(|location| location.device.as_ref() == Some(&device.device))
            .filter(|location| service::service_id(&location.id).is_some())
        else {
            return;
        };
        let then = match op {
            DeviceOp::Eject => AfterForce::Eject,
            DeviceOp::PowerOff => AfterForce::PowerOff,
            DeviceOp::Mount | DeviceOp::Unmount => AfterForce::Nothing,
        };
        self.model.location_dialog = Some(LocationDialog::Confirm {
            id: location.id.clone(),
            name: location.name.clone(),
            confirmation: Confirmation::Force { message, then },
        });
    }

    pub(crate) fn reduce_unlock(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::UnlockPassphraseChanged(text) => {
                if let Some(state) = self.model.unlock.as_mut().filter(|state| !state.busy) {
                    state.error = None;
                    state.passphrase = text;
                }
                Task::none()
            }
            Msg::ToggleUnlockRevealed => {
                if let Some(state) = &mut self.model.unlock {
                    state.revealed = !state.revealed;
                }
                Task::none()
            }
            Msg::CloseUnlock => {
                if !self.model.unlock.as_ref().is_some_and(|state| state.busy) {
                    self.model.unlock = None;
                }
                Task::none()
            }
            Msg::ConfirmUnlock => {
                let Some(state) = self
                    .model
                    .unlock
                    .as_mut()
                    .filter(|state| !state.passphrase.is_empty() && !state.busy)
                else {
                    return Task::none();
                };
                // The dialog stays open while the drive unlocks, so a wrong
                // passphrase is answered in place.
                state.busy = true;
                state.error = None;
                let (id, name, passphrase) = (
                    state.id.clone(),
                    state.name.clone(),
                    state.passphrase.clone(),
                );
                self.model.storage_busy.insert(id.clone());
                let activity = self.start_service_activity(
                    ActivityKind::Other("Mount"),
                    crate::i18n::format("files-mounting-name-a9625493", &[("name", name.clone())]),
                );
                let operation_id = id.clone();
                let operation = async move {
                    if let Some(target) = service::service_id(&operation_id) {
                        service::unlock(target.to_owned(), passphrase)
                            .await
                            .map(Some)
                    } else {
                        crate::storage::open_authenticated(operation_id, Some(passphrase)).await
                    }
                };
                Task::perform(operation, move |result| {
                    finished(id, name, true, activity, None, result)
                })
            }
            _ => unreachable!("misrouted message in reduce_unlock"),
        }
    }
}
