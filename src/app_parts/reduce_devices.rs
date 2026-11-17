// Removable-media reducer arms. Enumeration and every operation run on
// [`crate::devices::DeviceWork`]'s bounded owner.

use super::*;
use crate::devices::{DeviceError, DeviceOp};

impl App {
    // Refresh the kernel fallback for removable media.
    pub(crate) fn enumerate_devices(&mut self) -> Task<Msg> {
        let task = self.device_work.enumerate();
        Task::perform(task, |settlement| {
            Msg::RemovableDevicesLoaded(settlement.into_result().unwrap_or_default())
        })
    }

    pub(crate) fn reduce_phones_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PhonesLoaded(phones) = message else {
            unreachable!("misrouted message in reduce_phones_loaded");
        };
        self.model.phones = phones;
        Task::none()
    }

    pub(crate) fn reduce_open_phone(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenPhone(id) = message else {
            unreachable!("misrouted message in reduce_open_phone");
        };
        let Some(phone) = self.model.phones.iter().find(|p| p.id == id).cloned() else {
            self.post_status(
                crate::i18n::tr("That phone is no longer connected"),
                ToastTone::Warning,
            );
            return Task::none();
        };
        if let Some(path) = phone.mount {
            return Task::done(Msg::NavigateTo(path));
        }
        let activity = self.start_file_activity(
            ActivityKind::Other("Open"),
            crate::i18n::format("files-open-de7585f8", &[("arg0", (phone.name).clone())]),
            1,
        );
        let opening = id.clone();
        Task::perform(
            crate::shell_work::run(move || crate::devices::phones::open(&opening)),
            move |result| Msg::PhoneOpened {
                id: id.clone(),
                activity,
                result,
            },
        )
    }

    pub(crate) fn reduce_phone_opened(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PhoneOpened {
            id,
            activity,
            result,
        } = message
        else {
            unreachable!("misrouted message in reduce_phone_opened");
        };
        let name = self.model.phones.iter().find(|p| p.id == id).map_or_else(
            || crate::i18n::tr("the phone").to_owned(),
            |p| p.name.clone(),
        );
        let refresh = Task::perform(
            crate::shell_work::run(crate::devices::phones::list),
            Msg::PhonesLoaded,
        );
        match result {
            Ok(path) => {
                self.finish_file_activity(
                    activity,
                    1,
                    TerminalOutcome::Completed,
                    crate::i18n::format("files-opened-name-0c49269f", &[("name", (name))]),
                );
                Task::batch([refresh, Task::done(Msg::NavigateTo(path))])
            }
            Err(error) => {
                let detail = crate::i18n::format(
                    "files-could-not-open-name-error-22340e22",
                    &[("name", (name)), ("error", (error).to_string())],
                );
                self.finish_file_activity(activity, 0, TerminalOutcome::Failed, detail.clone());
                self.post_status(detail, ToastTone::Warning);
                refresh
            }
        }
    }

    pub(crate) fn reduce_removable_devices_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RemovableDevicesLoaded(devices) = message else {
            unreachable!("misrouted message in reduce_removable_devices_loaded");
        };
        self.model.removable_devices = devices;
        if let Some((id, _)) = &self.model.pending_device_confirm
            && !self.model.removable_devices.iter().any(|d| &d.id == id)
        {
            // The device went away while its confirmation was open.
            self.model.pending_device_confirm = None;
        }
        Task::none()
    }

    pub(crate) fn reduce_device_action(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DeviceAction(id, op) = message else {
            unreachable!("misrouted message in reduce_device_action");
        };
        if op.needs_confirmation() {
            self.model.pending_device_confirm = Some((id, op));
            return Task::none();
        }
        self.start_device_action(id, op)
    }

    pub(crate) fn reduce_confirm_device_action(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ConfirmDeviceAction = message else {
            unreachable!("misrouted message in reduce_confirm_device_action");
        };
        match self.model.pending_device_confirm.take() {
            Some((id, op)) => self.start_device_action(id, op),
            None => Task::none(),
        }
    }

    pub(crate) fn reduce_cancel_device_action(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelDeviceAction = message else {
            unreachable!("misrouted message in reduce_cancel_device_action");
        };
        self.model.pending_device_confirm = None;
        Task::none()
    }

    fn start_device_action(&mut self, id: String, op: DeviceOp) -> Task<Msg> {
        let Some(device) = self
            .model
            .removable_devices
            .iter()
            .find(|d| d.id == id)
            .cloned()
        else {
            self.post_status(
                crate::i18n::tr("That device is no longer connected"),
                ToastTone::Warning,
            );
            return Task::none();
        };
        let mounted_on_drive: Vec<PathBuf> = self
            .model
            .removable_devices
            .iter()
            .filter(|other| other.drive == device.drive && other.mount.is_some())
            .map(|other| other.device.clone())
            .collect();
        let kind = match op {
            DeviceOp::Mount => ActivityKind::Other("Mount"),
            DeviceOp::Unmount | DeviceOp::Eject | DeviceOp::PowerOff => ActivityKind::Unmount,
        };
        let activity =
            self.start_file_activity(kind, format!("{} {}", op.label(), device.label), 1);
        // The mount service owns the drives it reports.
        if let Some(service_id) = self
            .model
            .storage_locations
            .iter()
            .find(|location| location.device.as_ref() == Some(&device.device))
            .and_then(|location| crate::mount_service::service_id(&location.id))
        {
            use crate::mount_service as service;
            let service_id = service_id.to_owned();
            let operation = async move {
                match op {
                    DeviceOp::Mount => service::mount(service_id).await.map(Some),
                    DeviceOp::Unmount => service::operate(service_id, true).await,
                    DeviceOp::Eject | DeviceOp::PowerOff => {
                        service::remove_drive(service_id, op == DeviceOp::PowerOff)
                            .await
                            .map(|()| None)
                    }
                }
                .map_err(service::ServiceError::into_device_error)
            };
            return Task::perform(operation, move |result| Msg::DeviceActionFinished {
                id: id.clone(),
                op,
                activity,
                result,
            });
        }
        let task = self.device_work.perform(op, device, mounted_on_drive);
        Task::perform(task, move |settlement| Msg::DeviceActionFinished {
            id: id.clone(),
            op,
            activity,
            result: settlement
                .into_result()
                .unwrap_or_else(|error| Err(DeviceError::Failed(error.to_string()))),
        })
    }

    pub(crate) fn reduce_device_action_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DeviceActionFinished {
            id,
            op,
            activity,
            result,
        } = message
        else {
            unreachable!("misrouted message in reduce_device_action_finished");
        };
        let label = self
            .model
            .removable_devices
            .iter()
            .find(|d| d.id == id)
            .map_or_else(
                || crate::i18n::tr("the device").to_owned(),
                |d| d.label.clone(),
            );
        let (outcome, detail) = match &result {
            Ok(mounted_at) => (
                TerminalOutcome::Completed,
                match (op, mounted_at) {
                    (DeviceOp::Mount, Some(at)) => crate::i18n::format(
                        "files-mounted-label-at-e5b7a36e",
                        &[("label", (label)), ("arg0", (at.display()).to_string())],
                    ),
                    (DeviceOp::Mount, None) => {
                        crate::i18n::format("files-mounted-label-e4a4a9c8", &[("label", (label))])
                    }
                    (DeviceOp::Unmount, _) => {
                        crate::i18n::format("files-unmounted-label-54c76ccf", &[("label", (label))])
                    }
                    (DeviceOp::Eject, _) => {
                        crate::i18n::format("files-ejected-label-c8574644", &[("label", (label))])
                    }
                    (DeviceOp::PowerOff, _) => crate::i18n::format(
                        "files-label-can-be-removed-safely-38536d65",
                        &[("label", (label))],
                    ),
                },
            ),
            Err(DeviceError::Cancelled) => (
                TerminalOutcome::Cancelled,
                crate::i18n::tr("Cancelled").to_owned(),
            ),
            Err(error) => (
                TerminalOutcome::Failed,
                crate::i18n::format(
                    "files-could-not-label-error-da836a81",
                    &[
                        ("arg0", (op.label().to_lowercase())),
                        ("label", (label)),
                        ("error", (error).to_string()),
                    ],
                ),
            ),
        };
        self.finish_file_activity(
            activity,
            usize::from(result.is_ok()),
            outcome,
            detail.clone(),
        );
        self.post_outcome(detail, outcome);
        if let Err(DeviceError::InUse(message)) = result {
            self.offer_forced_removal(&id, op, message);
        }
        // Reloading drives re-enumerates removable media as well.
        Task::perform(load_drives(), Msg::DrivesLoaded)
    }
}

impl App {
    pub(crate) fn reduce_storage_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::StorageLoaded(result) = message else {
            unreachable!("storage inventory message")
        };
        match result {
            Ok(crate::storage::Inventory { locations, cloud }) => {
                self.model.cloud_providers = cloud;
                if self.model.storage_ready
                    && self.model.storage_locations == locations
                    && self.model.storage_error.is_none()
                {
                    return Task::none();
                }
                let containing_mount = |locations: &[crate::storage::Location]| {
                    locations
                        .iter()
                        .filter(|location| location.contains(&self.model.cwd))
                        .max_by_key(|location| {
                            location
                                .mount
                                .as_ref()
                                .map_or(0, |root| root.components().count())
                        })
                        .map(|location| location.id.clone())
                };
                let mounted_here = self.model.screen == Screen::Directory
                    && containing_mount(&self.model.storage_locations)
                        != containing_mount(&locations);
                let departed = if self.model.screen == Screen::Directory {
                    self.model
                        .storage_locations
                        .iter()
                        .filter(|location| location.contains(&self.model.cwd))
                        .filter(|location| {
                            !locations.iter().any(|next| next.mount == location.mount)
                        })
                        .max_by_key(|location| {
                            location
                                .mount
                                .as_ref()
                                .map_or(0, |root| root.components().count())
                        })
                        .map(|location| location.name.clone())
                } else {
                    None
                };
                self.model.storage_error = None;
                self.model.storage_ready = true;
                self.model.phones = locations
                    .iter()
                    .filter(|location| location.kind == crate::storage::Kind::Phone)
                    .map(|location| crate::devices::phones::Phone {
                        id: location.id.clone(),
                        name: location.name.clone(),
                        mount: location.mount.clone(),
                    })
                    .collect();
                self.model.storage_locations = locations;
                self.follow_cloud_sign_in();
                let refresh = Task::perform(load_drives(), Msg::DrivesLoaded);
                if let Some(name) = departed {
                    self.post_status(
                        crate::i18n::format(
                            "files-name-is-no-longer-available-f4d93a33",
                            &[("name", (name))],
                        ),
                        ToastTone::Info,
                    );
                    let navigate = self.nav_navigate_to(Msg::NavigateTo(self.model.home.clone()));
                    Task::batch([refresh, navigate])
                } else if mounted_here {
                    self.arm_dir_watch();
                    Task::batch([refresh, self.reload()])
                } else {
                    refresh
                }
            }
            Err(error) => {
                if self.model.storage_error.is_none() {
                    self.post_status(
                        crate::i18n::format(
                            "files-storage-discovery-error-fe7df2ee",
                            &[("error", (error).clone())],
                        ),
                        ToastTone::Warning,
                    );
                }
                self.model.storage_error = Some(error);
                Task::none()
            }
        }
    }

    pub(crate) fn reduce_storage_operation(&mut self, message: Msg) -> Task<Msg> {
        let (id, opening) = match message {
            Msg::OpenStorage(id) => (id, true),
            Msg::UnmountStorage(id) => (id, false),
            _ => unreachable!("storage operation message"),
        };
        if self.model.storage_busy.contains(&id) {
            return Task::none();
        }
        let Some(location) = self
            .model
            .storage_locations
            .iter()
            .find(|location| location.id == id)
            .cloned()
        else {
            self.post_status(
                crate::i18n::tr("That device or mount is no longer connected"),
                ToastTone::Warning,
            );
            return Task::none();
        };
        if opening && let Some(root) = location.mount {
            return self.nav_navigate_to(Msg::NavigateTo(root));
        }
        if location.empty_media {
            self.post_status(
                crate::i18n::tr("No media is inserted in this drive"),
                ToastTone::Info,
            );
            return Task::none();
        }
        if let Some(service) = location
            .service
            .as_ref()
            .filter(|service| service.formatting || service.signing_in)
        {
            // Nothing to open yet: a sign-in shows where it stands.
            if opening && service.signing_in {
                return self.reduce_cloud(Msg::ShowAddCloud);
            }
            return Task::none();
        }
        if opening
            && location
                .service
                .as_ref()
                .is_some_and(|service| service.locked)
        {
            self.model.unlock = Some(crate::mount_service::UnlockState::new(id, location.name));
            return Task::none();
        }
        self.model.storage_busy.insert(id.clone());
        let activity = self.start_file_activity(
            if opening {
                ActivityKind::Other("Mount")
            } else {
                ActivityKind::Unmount
            },
            format!(
                "{} {}",
                if opening { "Opening" } else { "Unmounting" },
                location.name
            ),
            1,
        );
        if let Some(service_id) = crate::mount_service::service_id(&id).map(str::to_owned) {
            use crate::mount_service as service;
            // Only a forced unmount detaches a server that stopped answering.
            let force = location
                .service
                .as_ref()
                .is_some_and(|service| service.not_responding);
            let operation = async move {
                if opening {
                    service::mount(service_id).await.map(Some)
                } else {
                    service::unmount(service_id, force).await.map(|()| None)
                }
            };
            return Task::perform(operation, move |result| Msg::MountServiceFinished {
                id: id.clone(),
                name: location.name.clone(),
                opening,
                activity,
                result,
                plan: None,
                boot: None,
            });
        }
        if opening {
            return Task::perform(
                crate::storage::open_authenticated(id.clone(), None),
                move |result| Msg::MountServiceFinished {
                    id: id.clone(),
                    name: location.name.clone(),
                    opening,
                    activity,
                    result,
                    plan: None,
                    boot: None,
                },
            );
        }
        let operation = crate::storage::operate(id.clone(), !opening);
        Task::perform(operation, move |result| Msg::StorageOperationFinished {
            id: id.clone(),
            name: location.name.clone(),
            opening,
            activity,
            result,
        })
    }

    pub(crate) fn reduce_mount_disk_image(&mut self, path: PathBuf) -> Task<Msg> {
        if !crate::storage::is_disk_image(&path) || crate::services::is_virtual(&path) {
            return Task::none();
        }
        let id = format!(
            "image:{}",
            gio::prelude::FileExt::uri(&gio::File::for_path(&path))
        );
        if !self.model.storage_busy.insert(id.clone()) {
            return Task::none();
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let activity = self.start_file_activity(
            ActivityKind::Other("Mount"),
            crate::i18n::format("files-mounting-name-a9625493", &[("name", (name).clone())]),
            1,
        );
        Task::perform(crate::storage::mount_image(path), move |result| {
            Msg::StorageOperationFinished {
                id: id.clone(),
                name: name.clone(),
                opening: true,
                activity,
                result,
            }
        })
    }

    pub(crate) fn reduce_storage_finished(&mut self, message: Msg) -> Task<Msg> {
        let Msg::StorageOperationFinished {
            id,
            name,
            opening,
            activity,
            result,
        } = message
        else {
            unreachable!("storage operation settlement")
        };
        self.model.storage_busy.remove(&id);
        let outcome = if result.is_ok() {
            TerminalOutcome::Completed
        } else {
            TerminalOutcome::Failed
        };
        let detail = match &result {
            Ok(_) => {
                if opening {
                    crate::i18n::format("files-opened-name-0c49269f", &[("name", (name))])
                } else {
                    crate::i18n::format("files-unmounted-name-ba297d3b", &[("name", (name))])
                }
            }
            Err(error) if opening && id.starts_with("image:") => crate::i18n::format(
                "files-could-not-mount-name-error-90a8b5ed",
                &[("name", (name)), ("error", (error).clone())],
            ),
            Err(error) => crate::i18n::format(
                "files-could-not-error-a468ba78",
                &[
                    (
                        "arg0",
                        (if opening {
                            crate::i18n::tr("open storage")
                        } else {
                            crate::i18n::tr("unmount storage")
                        })
                        .to_owned(),
                    ),
                    ("error", (error).clone()),
                ],
            ),
        };
        self.finish_file_activity(
            activity,
            usize::from(result.is_ok()),
            outcome,
            detail.clone(),
        );
        self.post_outcome(detail, outcome);
        let refresh = Task::perform(load_drives(), Msg::DrivesLoaded);
        match result {
            Ok(Some(root)) if opening => Task::batch([refresh, Task::done(Msg::NavigateTo(root))]),
            _ => refresh,
        }
    }
}
