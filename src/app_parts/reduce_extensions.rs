use super::*;
use crate::extensions::{self, Change};

impl App {
    pub(crate) fn reduce_extensions(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::ExtensionsLoaded(mut catalog) => {
                catalog.pending.clone_from(&self.model.extensions.pending);
                catalog.remove.clone_from(&self.model.extensions.remove);
                catalog.expanded.clone_from(&self.model.extensions.expanded);
                catalog.drafts = self.model.extensions.drafts.clone();
                catalog.busy = self.model.extensions.busy;
                catalog.running = self.model.extensions.running;
                self.model.extensions = catalog;
                Task::none()
            }
            Msg::BeginInstallExtension => {
                if self.model.extensions.busy {
                    return Task::none();
                }
                let target = picker::PickerTarget::InstallExtension;
                self.picker.start(target, &self.model, self.main_window)
            }
            Msg::ReviewExtension(path) => {
                if self.model.extensions.busy {
                    return Task::none();
                }
                self.model.view_settings_open = true;
                self.model.settings_page = model::SettingsSection::Extensions;
                self.model.extensions.busy = true;
                self.model.extensions.pending = None;
                Task::perform(
                    crate::shell_work::run(move || {
                        extensions::Prepared::open(&path).map(std::sync::Arc::new)
                    }),
                    Msg::ExtensionReviewed,
                )
            }
            Msg::ExtensionReviewed(result) => {
                self.model.extensions.busy = false;
                match result {
                    Ok(package) => self.model.extensions.pending = Some(package),
                    Err(error) => self.post_status(error, ToastTone::Error),
                }
                Task::none()
            }
            Msg::CancelExtensionReview => {
                self.model.extensions.pending = None;
                self.model.extensions.remove = None;
                Task::none()
            }
            Msg::ConfirmInstallExtension => {
                let Some(package) = self.model.extensions.pending.clone() else {
                    return Task::none();
                };
                self.reduce_extensions(Msg::ChangeExtension(Change::Install(package)))
            }
            // Opens the extension's settings subpage, or leaves it when it is
            // the one shown.
            Msg::ExtensionDetails(id) => {
                self.model.downloads_settings_open = false;
                self.model.extensions.expanded =
                    if self.model.extensions.expanded.as_ref() == Some(&id) {
                        None
                    } else {
                        Some(id)
                    };
                Task::none()
            }
            Msg::ShowDownloadsSettings(open) => {
                self.model.extensions.expanded = None;
                self.model.downloads_settings_open = open;
                Task::none()
            }
            Msg::ExtensionSettingEdited {
                extension,
                key,
                value,
            } => {
                if self.model.extensions.busy {
                    return Task::none();
                }
                if let Some(installed) = self
                    .model
                    .extensions
                    .entries
                    .iter()
                    .find(|e| e.id == extension)
                {
                    let Some(manifest) = &installed.manifest else {
                        return Task::none();
                    };
                    let defaults = enzui_services::extension_package::SettingsDraft::new(
                        &manifest.settings,
                        &installed.settings(),
                    );
                    self.model
                        .extensions
                        .drafts
                        .entry(extension)
                        .or_insert(defaults)
                        .edit(&key, value);
                }
                Task::none()
            }
            Msg::SaveExtensionSettings(id) => {
                let Some(draft) = self.model.extensions.drafts.get(&id) else {
                    return Task::none();
                };
                let Some(manifest) = self
                    .model
                    .extensions
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .and_then(|entry| entry.manifest.as_ref())
                else {
                    return Task::none();
                };
                let Ok(values) = draft.apply(&manifest.settings) else {
                    return Task::none();
                };
                self.reduce_extensions(Msg::ChangeExtension(Change::Settings(id, values)))
            }
            Msg::AskRemoveExtension(id) => {
                self.model.extensions.remove = Some(id);
                self.model.extensions.pending = None;
                Task::none()
            }
            Msg::ConfirmRemoveExtension => {
                let Some(id) = self.model.extensions.remove.clone() else {
                    return Task::none();
                };
                self.reduce_extensions(Msg::ChangeExtension(Change::Remove(id)))
            }
            Msg::ChangeExtension(change) => {
                if self.model.extensions.busy {
                    return Task::none();
                }
                let Some(root) = self.model.extensions.root.clone() else {
                    self.post_status(
                        crate::i18n::tr("Extension storage is unavailable"),
                        ToastTone::Warning,
                    );
                    return Task::none();
                };
                self.model.extensions.busy = true;
                Task::perform(
                    crate::shell_work::run(move || extensions::change(root, change)),
                    Msg::ExtensionsChanged,
                )
            }
            Msg::ExtensionsChanged(result) => {
                self.model.extensions.busy = false;
                match result {
                    Ok(catalog) => {
                        let mut catalog = catalog;
                        catalog.expanded.clone_from(&self.model.extensions.expanded);
                        self.model.extensions = catalog;
                        self.post_status(crate::i18n::tr("Extensions updated"), ToastTone::Success);
                    }
                    Err(error) => {
                        self.post_status(error, ToastTone::Error);
                    }
                }
                Task::none()
            }
            Msg::RunExtension {
                extension,
                action,
                path,
                background,
            } => {
                if self.model.extensions.busy || crate::services::is_virtual(&self.model.cwd) {
                    return Task::none();
                }
                let entries = self.extension_selection(&path, background);
                let Some((installed, contribution)) = self
                    .model
                    .extensions
                    .actions(&entries, background)
                    .into_iter()
                    .find(|(e, a)| e.id == extension && a.id == action)
                else {
                    return Task::none();
                };
                let Some(root) = self.model.extensions.root.clone() else {
                    return Task::none();
                };
                let request =
                    extensions::request(installed, contribution, &self.model.cwd, entries);
                let label = installed.display_text(&contribution.label);
                let installed = installed.clone();
                let cancel = enzui_core::cancellation::CancelToken::new();
                let activity = self.start_file_activity(
                    ActivityKind::Other("Extension"),
                    crate::i18n::format(
                        "files-extension-action",
                        &[("extension", installed.display_name()), ("action", label)],
                    ),
                    1,
                );
                let now = self.activity_now_ms();
                activity::offer_cancel(&mut self.activity_center, activity, now);
                self.extension_activity = Some(activity);
                self.extension_cancel = Some(cancel.clone());
                self.model.extensions.running = true;
                self.model.extensions.busy = true;
                self.post_status(
                    crate::i18n::tr("Running extension action…"),
                    ToastTone::Info,
                );
                Task::perform(
                    crate::shell_work::run(move || {
                        extensions::run(installed, root, request, &cancel)
                    }),
                    Msg::ExtensionFinished,
                )
            }
            Msg::ExtensionFinished(result) => {
                self.model.extensions.busy = false;
                let cancelled = self
                    .extension_cancel
                    .as_ref()
                    .is_some_and(enzui_core::cancellation::CancelToken::is_terminal);
                let result = if cancelled {
                    Err(crate::i18n::tr("Extension action cancelled").to_owned())
                } else {
                    result
                };
                self.extension_cancel = None;
                if let Some(activity) = self.extension_activity.take() {
                    let outcome = if cancelled {
                        TerminalOutcome::Cancelled
                    } else if result.is_ok() {
                        TerminalOutcome::Completed
                    } else {
                        TerminalOutcome::Failed
                    };
                    let detail = result
                        .as_ref()
                        .map_or_else(std::clone::Clone::clone, |response| {
                            response.message.clone()
                        });
                    self.finish_file_activity(
                        activity,
                        usize::from(result.is_ok()),
                        outcome,
                        detail,
                    );
                }
                self.model.extensions.running = false;
                let task = match result {
                    Err(error) => {
                        self.post_status(
                            error,
                            if cancelled {
                                ToastTone::Info
                            } else {
                                ToastTone::Error
                            },
                        );
                        Task::none()
                    }
                    Ok(response) => {
                        if !response.message.is_empty() {
                            self.post_status(response.message, ToastTone::Success);
                        }
                        let mut tasks = Vec::new();
                        if let Some(text) = response.clipboard {
                            tasks.push(iced::clipboard::write(text).discard());
                        }
                        if let Some(path) = response.navigate {
                            use std::os::unix::ffi::OsStringExt;
                            tasks.push(Task::done(Msg::NavigateTo(PathBuf::from(
                                std::ffi::OsString::from_vec(path),
                            ))));
                        } else if response.refresh {
                            tasks.push(self.reload());
                        }
                        Task::batch(tasks)
                    }
                };
                if self.close_pending.is_some() {
                    Task::batch([task, Task::done(Msg::CleanupDrainComplete)])
                } else {
                    task
                }
            }
            Msg::CancelExtensionAction => {
                if let Some(cancel) = &self.extension_cancel {
                    cancel.cancel(enzui_core::cancellation::CancelCause::Cancelled);
                }
                Task::none()
            }
            _ => unreachable!("extension message routing"),
        }
    }
    pub(crate) fn extension_selection(
        &self,
        path: &Path,
        background: bool,
    ) -> Vec<model::FileEntry> {
        if background || (self.model.is_selected(path) && self.model.selection.len() > 128) {
            return Vec::new();
        }
        self.model
            .projected_entries()
            .into_iter()
            .filter(|entry| {
                if self.model.is_selected(path) {
                    self.model.is_selected(&entry.path)
                } else {
                    entry.path == path
                }
            })
            .cloned()
            .collect()
    }
}
