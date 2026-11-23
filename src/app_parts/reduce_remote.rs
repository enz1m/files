// Open-with, attach, and network reducer arms.

use super::*;

impl App {
    pub(crate) fn remote_show_open_with(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowOpenWith(path) = message else {
            unreachable!("misrouted message in remote_show_open_with");
        };
        // An entry outside the listing gets an empty MIME; the query then reports
        // an empty or unavailable state.
        let (name, mime) = self
            .model
            .entries
            .iter()
            .find(|e| e.path == path)
            .map_or_else(
                || {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (name, String::new())
                },
                |e| (e.name.clone(), e.mime.clone()),
            );
        let mut ow = open_with::OpenWithState::for_target(name, mime.clone());
        ow.target_path = Some(path);
        self.model.open_with = Some(ow);
        let (generation, process_cancel, scan_cancel) = self.begin_mime_query();
        let task = query_mime_applications(
            std::sync::Arc::clone(&self.mime_query),
            mime,
            generation,
            process_cancel,
            scan_cancel,
        );
        self.own_mime_query(generation, task)
    }

    pub(crate) fn remote_open_with_queried(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenWithQueried {
            generation,
            mime,
            outcome,
        } = message
        else {
            unreachable!("misrouted message in remote_open_with_queried");
        };
        if !self.finish_mime_query(generation) {
            return Task::none();
        }
        // The off-thread reply may arrive after close or for another MIME.
        if let Some(ow) = &mut self.model.open_with
            && ow.mime == mime
        {
            ow.apply_query(outcome);
        }
        Task::none()
    }

    pub(crate) fn remote_close_open_with(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseOpenWith = message else {
            unreachable!("misrouted message in remote_close_open_with");
        };
        self.cancel_mime_query();
        self.model.open_with = None;
        Task::none()
    }

    pub(crate) fn remote_select_open_candidate(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectOpenCandidate(i) = message else {
            unreachable!("misrouted message in remote_select_open_candidate");
        };
        if let Some(ow) = &mut self.model.open_with
            && i < ow.candidates.len()
        {
            ow.selected = Some(i);
        }
        Task::none()
    }

    pub(crate) fn remote_set_open_behavior(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetOpenBehavior(behavior) = message else {
            unreachable!("misrouted message in remote_set_open_behavior");
        };
        if let Some(ow) = &mut self.model.open_with {
            ow.behavior = behavior;
        }
        Task::none()
    }

    pub(crate) fn remote_open_with_command_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenWithCommandChanged(text) = message else {
            unreachable!("misrouted message in remote_open_with_command_changed");
        };
        if let Some(ow) = &mut self.model.open_with {
            ow.custom_command = text;
        }
        Task::none()
    }

    pub(crate) fn remote_confirm_open_with(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ConfirmOpenWith = message else {
            unreachable!("misrouted message in remote_confirm_open_with");
        };
        // Launched off-thread through the injected launcher; the outcome arrives as
        // `OpenWithLaunched`.
        let launch = self.model.open_with.as_ref().and_then(|ow| {
            match (ow.effective_command(), ow.target_path.clone()) {
                (Some(cmd), Some(path)) => Some((cmd, path)),
                _ => None,
            }
        });
        // "Always this app" caches the command per MIME for `Msg::OpenEntry` and,
        // for an installed handler, writes the freedesktop association off-thread.
        // A typed command has no desktop-file ID and is cached for the session only.
        let persist = self.model.open_with.as_ref().and_then(|ow| {
            if ow.behavior != open_with::OpenBehavior::Always || ow.mime.is_empty() {
                return None;
            }
            let spec = ow.effective_command()?;
            let desktop_id = if ow.custom_command.trim().is_empty() {
                ow.selected
                    .and_then(|i| ow.candidates.get(i))
                    .map(|c| c.desktop_id.clone())
            } else {
                None
            };
            Some((ow.mime.clone(), spec, desktop_id))
        });
        let open_in_files = self.model.open_with.as_ref().is_some_and(|ow| {
            ow.custom_command.trim().is_empty()
                && ow
                    .selected
                    .and_then(|index| ow.candidates.get(index))
                    .is_some_and(|app| app.desktop_id == format!("{}.desktop", crate::APP_ID))
        });
        // Closing cancels only the chooser query; the durable write is owned
        // separately so a later dialog cannot revoke it.
        self.cancel_mime_query();
        self.model.open_with = None;
        let mut tasks: Vec<Task<Msg>> = Vec::new();
        if let Some((mime, spec, desktop_id)) = persist {
            self.model.mime_defaults.insert(mime.clone(), spec);
            if let Some(desktop_id) = desktop_id {
                let query = std::sync::Arc::clone(&self.mime_query);
                let (generation, cancel) = self.begin_mime_write();
                let write = Task::perform(
                    crate::shell_work::run(move || {
                        query.set_default_application_cancellable(&mime, &desktop_id, &cancel)
                    }),
                    move |result| Msg::MimeDefaultPersisted { generation, result },
                );
                tasks.push(self.own_mime_write(generation, write));
            }
        }
        match launch {
            Some((_, path))
                if open_in_files && crate::archive::ArchiveFormat::detect(&path).is_some() =>
            {
                tasks.push(self.reduce_extract_archive(Msg::ExtractArchive {
                    archive: path,
                    dest_dir: None,
                }));
            }
            Some((cmd, path)) => {
                // The opening app is the feedback; only a failure gets a toast.
                let launcher = std::sync::Arc::clone(&self.launcher);
                let (generation, cancel) = self.begin_launch();
                let launch = Task::perform(
                    crate::shell_work::run(move || {
                        launcher.run_command_cancellable(&cmd, &path, &cancel)
                    }),
                    move |result| Msg::OpenWithLaunched { generation, result },
                );
                tasks.push(self.own_launch(generation, launch));
            }
            None => {
                self.post_status(
                    crate::i18n::tr("No application selected"),
                    ToastTone::Warning,
                );
            }
        }
        Task::batch(tasks)
    }

    pub(crate) fn remote_open_with_launched(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenWithLaunched { generation, result } = message else {
            unreachable!("misrouted message in remote_open_with_launched");
        };
        if !self.finish_launch(generation) {
            return Task::none();
        }
        if let Err(message) = result {
            self.post_status(message, ToastTone::Error);
        }
        Task::none()
    }

    pub(crate) fn remote_mime_default_persisted(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MimeDefaultPersisted { generation, result } = message else {
            unreachable!("misrouted message in remote_mime_default_persisted");
        };
        if !self.finish_mime_write(generation) {
            return Task::none();
        }
        // The session cache is authoritative, so a failed write is only reported.
        if let Err(msg) = result {
            self.post_status(
                crate::i18n::format(
                    "files-could-not-set-the-default-application-963e5c1f",
                    &[("msg", (msg))],
                ),
                ToastTone::Error,
            );
        }
        Task::none()
    }

    pub(crate) fn remote_show_attach(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ShowAttach = message else {
            unreachable!("misrouted message in remote_show_attach");
        };
        if self.model.features.enabled(features::FeatureId::Network) {
            // A new generation lets the backend refuse stale-session callbacks.
            self.next_attach_generation = self.next_attach_generation.saturating_add(1);
            let generation = self.next_attach_generation;
            self.model.attach = Some(network::AttachState {
                generation,
                service: crate::mount_service::detected(),
                ..network::AttachState::default()
            });
        }
        Task::none()
    }

    pub(crate) fn remote_close_attach(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseAttach = message else {
            unreachable!("misrouted message in remote_close_attach");
        };
        if let Some(cancel) = self.network_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.network_operation.take() {
            handle.abort();
        }
        self.model.attach = None;
        Task::none()
    }

    pub(crate) fn remote_set_attach_protocol(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetAttachProtocol(protocol) = message else {
            unreachable!("misrouted message in remote_set_attach_protocol");
        };
        if let Some(a) = &mut self.model.attach {
            a.protocol = protocol;
        }
        Task::none()
    }

    pub(crate) fn remote_attach_server_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachServerChanged(text) = message else {
            unreachable!("misrouted message in remote_attach_server_changed");
        };
        if let Some(a) = &mut self.model.attach {
            a.server = text;
        }
        Task::none()
    }

    pub(crate) fn remote_attach_share_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachShareChanged(text) = message else {
            unreachable!("misrouted message in remote_attach_share_changed");
        };
        if let Some(a) = &mut self.model.attach {
            a.share = text;
        }
        Task::none()
    }

    pub(crate) fn remote_attach_domain_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachDomainChanged(text) = message else {
            unreachable!("misrouted message in remote_attach_domain_changed");
        };
        if let Some(a) = &mut self.model.attach {
            a.domain = text;
        }
        Task::none()
    }

    pub(crate) fn remote_attach_username_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachUsernameChanged(text) = message else {
            unreachable!("misrouted message in remote_attach_username_changed");
        };
        if let Some(a) = &mut self.model.attach {
            a.username = text;
        }
        Task::none()
    }

    pub(crate) fn remote_attach_password_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachPasswordChanged(text) = message else {
            unreachable!("misrouted message in remote_attach_password_changed");
        };
        if let Some(a) = &mut self.model.attach {
            // A fresh SecretText per keystroke, so the previous bytes are wiped.
            a.set_password(text);
        }
        Task::none()
    }

    pub(crate) fn remote_toggle_attach_password_revealed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleAttachPasswordRevealed = message else {
            unreachable!("misrouted message in remote_toggle_attach_password_revealed");
        };
        if let Some(a) = &mut self.model.attach {
            a.password_revealed = !a.password_revealed;
        }
        Task::none()
    }

    pub(crate) fn remote_discover_smb(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DiscoverSmb = message else {
            unreachable!("misrouted message in remote_discover_smb");
        };
        if !self.model.features.enabled(features::FeatureId::Network) {
            return Task::none();
        }
        if let Some(a) = &mut self.model.attach {
            a.backend = network::BackendStatus::Working;
        }
        // The scan runs `avahi-browse` on a worker, never in the reducer.
        if let Some(cancel) = self.network_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.network_operation.take() {
            handle.abort();
        }
        let scanner = std::sync::Arc::clone(&self.scanner);
        let context_probe = std::sync::Arc::clone(&self.network_context_probe);
        let cancel = network::OperationCancel::default();
        let worker_cancel = cancel.clone();
        let (task, handle) = Task::perform(
            crate::shell_work::run(move || {
                let result = scanner.scan_cancellable(&worker_cancel);
                let context = if matches!(&result, Ok(shares) if shares.is_empty())
                    && !worker_cancel.is_cancelled()
                {
                    context_probe.probe()
                } else {
                    network::DiscoveryContext::default()
                };
                (result, context)
            }),
            |(result, context)| Msg::AttachDiscovered(result, context),
        )
        .abortable();
        self.network_cancel = Some(cancel);
        self.network_operation = Some(handle);
        task
    }

    pub(crate) fn remote_attach_discovered(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachDiscovered(result, context) = message else {
            unreachable!("misrouted message in remote_attach_discovered");
        };
        self.network_operation = None;
        self.network_cancel = None;
        if let Some(a) = &mut self.model.attach {
            a.context = context;
            match result {
                Ok(shares) if shares.is_empty() => {
                    a.discovered.clear();
                    a.discovery = network::DiscoveryState::Empty;
                    a.backend = network::BackendStatus::Ready;
                }
                Ok(shares) => {
                    a.discovered = network::unique_shares(shares);
                    a.discovery = network::DiscoveryState::Found;
                    a.backend = network::BackendStatus::Ready;
                }
                Err(reason) => {
                    if !reason.preserve_previous {
                        a.discovered.clear();
                    }
                    a.backend = if reason.preserve_previous {
                        network::BackendStatus::Ready
                    } else {
                        network::BackendStatus::Unavailable
                    };
                    a.discovery = network::DiscoveryState::Failed(reason.message);
                }
            }
        }
        Task::none()
    }

    pub(crate) fn remote_adopt_discovered(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AdoptDiscovered(i) = message else {
            unreachable!("misrouted message in remote_adopt_discovered");
        };
        if let Some(a) = &mut self.model.attach
            && let Some(share) = a.discovered.get(i).cloned()
        {
            a.adopt(&share);
        }
        Task::none()
    }

    pub(crate) fn remote_confirm_attach(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ConfirmAttach = message else {
            unreachable!("misrouted message in remote_confirm_attach");
        };
        if !self.model.features.enabled(features::FeatureId::Network) {
            return Task::none();
        }
        if crate::mount_service::detected() {
            return self.service_confirm_attach();
        }
        // Blocked on form state and backend support, with distinct reasons. Only
        // the password-free URI is ever logged; credentials travel as a typed
        // [`network::MountCredentials`].
        let attach = self.model.attach.as_ref().filter(|a| a.can_attach());
        let blocker = self
            .model
            .attach
            .as_ref()
            .and_then(|a| a.attach_blocker(&*self.mounter));
        let mount_plan = attach.and_then(|a| {
            let uri = a.mount_uri()?;
            let password_present = !a.password.expose_secret().is_empty();
            if password_present && !a.backend_can_carry_password(&*self.mounter) {
                return None;
            }
            // Only the in-process backend can ask about an unknown SFTP host key,
            // so key-based SFTP uses it too.
            let asks_questions = matches!(a.protocol, network::NetworkProtocol::Sftp)
                && self
                    .mounter
                    .supports_authenticated_mount(network::NetworkProtocol::Sftp);
            Some(AttachDispatch {
                uri,
                credentials: if password_present || asks_questions {
                    Some(a.mount_credentials())
                } else {
                    None
                },
            })
        });
        match mount_plan {
            Some(plan) => {
                let AttachDispatch { uri, credentials } = plan;
                self.model.attach = None;
                // An activity row, not a toast that would expire mid-mount;
                // `AttachMounted` closes it and posts the result.
                self.mount_activity = Some(self.start_file_activity(
                    ActivityKind::Other("Mounting"),
                    crate::i18n::format("files-mounting-uri-b9db6582", &[("uri", (uri).clone())]),
                    1,
                ));
                let mounter = std::sync::Arc::clone(&self.mounter);
                let cancel = network::OperationCancel::default();
                let worker_cancel = cancel.clone();
                self.model.network.pending = true;
                self.model.network.pending_mount = true;
                self.model.network.backend = network::BackendStatus::Working;
                let (task, handle) = match credentials {
                    None => {
                        let anon_uri = uri;
                        Task::perform(
                            crate::shell_work::run(move || {
                                mounter.mount_cancellable(&anon_uri, &worker_cancel)
                            }),
                            Msg::AttachMounted,
                        )
                        .abortable()
                    }
                    Some(credentials) => {
                        // The credentials move into the worker and are wiped when it
                        // returns. One stream carries the questions, then the outcome.
                        Task::run(
                            iced::stream::channel(1, move |mut output: iced::futures::channel::mpsc::Sender<Msg>| async move {
                                use iced::futures::future::{Either, select};
                                use iced::futures::{SinkExt, StreamExt};
                                let (port, mut questions) = network::question_channel();
                                let credentials = credentials.asking(port);
                                let mut mount = Box::pin(crate::shell_work::run(move || {
                                    mounter.mount_authenticated(credentials, &worker_cancel)
                                }));
                                let result = loop {
                                    match select(mount, questions.next()).await {
                                        Either::Left((result, _)) => break result,
                                        Either::Right((Some(asked), pending)) => {
                                            mount = pending;
                                            let _ = output.send(Msg::MountQuestionAsked(asked)).await;
                                        }
                                        Either::Right((None, pending)) => break pending.await,
                                    }
                                };
                                let _ = output.send(Msg::AttachMounted(result)).await;
                            }),
                            |message| message,
                        )
                        .abortable()
                    }
                };
                self.network_cancel = Some(cancel);
                self.network_operation = Some(handle);
                task
            }
            None => {
                // The dialog stays open, with the specific reason from
                // [`network::AttachState::attach_blocker`].
                if let Some(reason) = blocker {
                    self.post_status(reason, ToastTone::Warning);
                }
                Task::none()
            }
        }
    }

    pub(crate) fn remote_mount_question(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::MountQuestionAsked(asked) => {
                // A newer question replaces and declines an older one.
                if let Some(previous) = self.model.mount_question.replace(asked) {
                    previous.answer(network::MountAnswer::Decline);
                }
            }
            Msg::MountQuestionAnswered(answer) => {
                if let Some(asked) = self.model.mount_question.take() {
                    asked.answer(answer);
                }
            }
            _ => unreachable!("misrouted message in remote_mount_question"),
        }
        Task::none()
    }

    pub(crate) fn remote_attach_mounted(&mut self, message: Msg) -> Task<Msg> {
        let Msg::AttachMounted(result) = message else {
            unreachable!("misrouted message in remote_attach_mounted");
        };
        // A question still on screen can no longer be answered.
        if let Some(asked) = self.model.mount_question.take() {
            asked.answer(network::MountAnswer::Decline);
        }
        self.network_operation = None;
        self.network_cancel = None;
        self.model.network.pending = false;
        self.model.network.pending_mount = false;
        self.model.network.backend = network::BackendStatus::Ready;
        let mount_activity = self.mount_activity.take();
        match result {
            Ok(uri) => {
                self.model.network.operation_error = None;
                let detail = crate::i18n::format("files-mounted-uri-f1283bf7", &[("uri", (uri))]);
                if let Some(activity) = mount_activity {
                    self.finish_file_activity(
                        activity,
                        1,
                        TerminalOutcome::Completed,
                        detail.clone(),
                    );
                }
                self.post_status(detail, ToastTone::Success);
                // Re-reading picks up the new mount under /run/user/<uid>/gvfs.
                return Task::perform(load_drives(), Msg::DrivesLoaded);
            }
            Err(msg) => {
                self.model.network.operation_error = Some(msg.clone());
                if let Some(activity) = mount_activity {
                    self.finish_file_activity(activity, 0, TerminalOutcome::Failed, msg.clone());
                }
                self.post_status(msg, ToastTone::Error);
            }
        }
        Task::none()
    }

    pub(crate) fn remote_discover_network(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DiscoverNetwork = message else {
            unreachable!("misrouted message in remote_discover_network");
        };
        if !self.model.features.enabled(features::FeatureId::Network) {
            return Task::none();
        }
        if let Some(cancel) = self.network_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.network_operation.take() {
            handle.abort();
        }
        self.model.network.begin_scan();
        let scanner = std::sync::Arc::clone(&self.scanner);
        let context_probe = std::sync::Arc::clone(&self.network_context_probe);
        let cancel = network::OperationCancel::default();
        let worker_cancel = cancel.clone();
        let (task, handle) = Task::perform(
            crate::shell_work::run(move || {
                let result = scanner.scan_cancellable(&worker_cancel);
                let context = if matches!(&result, Ok(shares) if shares.is_empty())
                    && !worker_cancel.is_cancelled()
                {
                    context_probe.probe()
                } else {
                    network::DiscoveryContext::default()
                };
                (result, context)
            }),
            |(result, context)| Msg::NetworkDiscovered(result, context),
        )
        .abortable();
        self.network_cancel = Some(cancel);
        self.network_operation = Some(handle);
        task
    }

    pub(crate) fn remote_network_discovered(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NetworkDiscovered(result, context) = message else {
            unreachable!("misrouted message in remote_network_discovered");
        };
        self.network_operation = None;
        self.network_cancel = None;
        if self.model.features.enabled(features::FeatureId::Network) {
            self.model.network.finish_scan(result, context);
        }
        Task::none()
    }

    pub(crate) fn remote_open_network_settings(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenNetworkSettings = message else {
            unreachable!("misrouted message in remote_open_network_settings");
        };
        let opener = std::sync::Arc::clone(&self.settings_opener);
        let activation = self.main_window.map_or_else(
            || Task::done(None),
            |window| iced::window::request_activation_token(window).map(Result::ok),
        );
        activation.then(move |token| {
            let opener = std::sync::Arc::clone(&opener);
            Task::perform(
                crate::shell_work::run(move || opener.open_network_settings(token)),
                Msg::NetworkSettingsOpened,
            )
        })
    }

    pub(crate) fn remote_network_settings_opened(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NetworkSettingsOpened(result) = message else {
            unreachable!("misrouted message in remote_network_settings_opened");
        };
        if let Err(msg) = result {
            self.post_status(msg, ToastTone::Error);
        }
        Task::none()
    }

    pub(crate) fn remote_mount_discovered(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MountDiscovered(index) = message else {
            unreachable!("misrouted message in remote_mount_discovered");
        };
        if !self.model.features.enabled(features::FeatureId::Network) {
            return Task::none();
        }
        let Some(uri) = self
            .model
            .network
            .discovered
            .get(index)
            .map(super::network::DiscoveredShare::uri)
        else {
            return Task::none();
        };
        if crate::mount_service::detected() {
            return self.service_connect(uri, crate::mount_service::ConnectPlan::default());
        }
        if let Some(cancel) = self.network_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.network_operation.take() {
            handle.abort();
        }
        self.model.network.pending = true;
        self.model.network.pending_mount = true;
        self.model.network.backend = network::BackendStatus::Working;
        // A mount like ConfirmAttach, so it gets an activity row too.
        self.mount_activity = Some(self.start_file_activity(
            ActivityKind::Other("Mounting"),
            crate::i18n::format("files-mounting-uri-b9db6582", &[("uri", (uri).clone())]),
            1,
        ));
        let mounter = std::sync::Arc::clone(&self.mounter);
        let cancel = network::OperationCancel::default();
        let worker_cancel = cancel.clone();
        let (task, handle) = Task::perform(
            crate::shell_work::run(move || mounter.mount_cancellable(&uri, &worker_cancel)),
            Msg::AttachMounted,
        )
        .abortable();
        self.network_cancel = Some(cancel);
        self.network_operation = Some(handle);
        task
    }

    pub(crate) fn remote_cancel_network_operation(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelNetworkOperation = message else {
            unreachable!("misrouted message in remote_cancel_network_operation");
        };
        if let Some(cancel) = self.network_cancel.take() {
            cancel.cancel();
        }
        if let Some(handle) = self.network_operation.take() {
            handle.abort();
        }
        self.model.network.pending = false;
        self.model.network.pending_mount = false;
        self.model.network.backend = network::BackendStatus::Ready;
        if let Some(attach) = &mut self.model.attach {
            attach.backend = network::BackendStatus::Ready;
        }
        // Closes the activity row as Cancelled so no "Mounting…" row lingers.
        if let Some(activity) = self.mount_activity.take() {
            self.finish_file_activity(
                activity,
                0,
                TerminalOutcome::Cancelled,
                crate::i18n::tr("Network operation cancelled"),
            );
        }
        self.post_status(
            crate::i18n::tr("Network operation cancelled"),
            ToastTone::Info,
        );
        Task::none()
    }
}
