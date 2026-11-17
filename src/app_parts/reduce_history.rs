// Previous versions, browsing a folder as it was, and backup actions.

use super::*;
use crate::history::{self, BrowseDialog, Dialog, HistoryMsg, VersionsDialog};

fn send(message: HistoryMsg) -> Msg {
    Msg::History(message)
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl App {
    fn versions_dialog(&mut self) -> Option<&mut VersionsDialog> {
        match self.model.history.as_mut() {
            Some(Dialog::Versions(d)) => Some(d),
            _ => None,
        }
    }
    fn browse_dialog(&mut self) -> Option<&mut BrowseDialog> {
        match self.model.history.as_mut() {
            Some(Dialog::Browse(d)) => Some(d),
            _ => None,
        }
    }
    fn selected_version(&mut self) -> Option<(PathBuf, history::Version)> {
        let d = self.versions_dialog()?;
        let version = d
            .versions
            .as_ref()?
            .as_ref()
            .ok()?
            .get(d.selected?)?
            .clone();
        Some((d.path.clone(), version))
    }
    fn load_items(&mut self) -> Task<Msg> {
        let Some(d) = self.browse_dialog() else {
            return Task::none();
        };
        let Some(Ok(snapshots)) = &d.snapshots else {
            return Task::none();
        };
        let Some((number, _)) = snapshots.get(d.at).copied() else {
            return Task::none();
        };
        d.items = None;
        d.chosen.clear();
        let folder = d.folder.clone();
        Task::perform(history::folder(folder, number), move |r| {
            send(HistoryMsg::Items(number, r))
        })
    }

    pub(crate) fn history_message(&mut self, message: HistoryMsg) -> Task<Msg> {
        match message {
            HistoryMsg::Refreshed(_) if self.model.history_fixture => Task::none(),
            HistoryMsg::Refreshed(found) => {
                self.model.history_available = found.is_some();
                self.model.history_excluded = found.unwrap_or_default();
                // An open menu picks up the Backup submenu as soon as it is known.
                let target = self
                    .model
                    .context_menu
                    .as_ref()
                    .map(|open| (open.path.clone(), open.origin));
                if let Some((path, origin)) = target
                    && let Some(items) = context_menu::items_for_target(&self.model, &path, origin)
                    && let Some(open) = self.model.context_menu.as_mut()
                {
                    open.refresh(items);
                }
                Task::none()
            }
            HistoryMsg::ShowVersions(path) => {
                // Opened from Properties, it takes that dialog's place.
                self.model.close_properties();
                self.model.history = Some(Dialog::Versions(VersionsDialog {
                    path: path.clone(),
                    versions: None,
                    selected: None,
                    preview: None,
                    busy: false,
                    error: None,
                }));
                Task::perform(history::versions(path), |r| send(HistoryMsg::Versions(r)))
            }
            HistoryMsg::Versions(result) => {
                let Some(d) = self.versions_dialog() else {
                    return Task::none();
                };
                // The newest version that differs from the file now is the
                // most likely one wanted.
                let first = result
                    .as_ref()
                    .ok()
                    .and_then(|v| v.iter().position(|v| !v.current));
                d.versions = Some(result);
                match first {
                    Some(index) => self.history_message(HistoryMsg::Select(index)),
                    None => Task::none(),
                }
            }
            HistoryMsg::Select(index) => {
                let Some(d) = self.versions_dialog() else {
                    return Task::none();
                };
                d.selected = Some(index);
                d.preview = None;
                d.error = None;
                let Some((path, version)) = self.selected_version() else {
                    return Task::none();
                };
                let number = version.number;
                Task::perform(history::preview(path, number), move |r| {
                    send(HistoryMsg::Previewed(number, r))
                })
            }
            HistoryMsg::Previewed(number, result) => {
                if self
                    .selected_version()
                    .is_some_and(|(_, v)| v.number == number)
                    && let Some(d) = self.versions_dialog()
                {
                    d.preview = Some(result.unwrap_or(history::Preview::None));
                }
                Task::none()
            }
            HistoryMsg::OpenSelected => {
                let Some((path, version)) = self.selected_version() else {
                    return Task::none();
                };
                Task::perform(
                    history::open_copy(path, version.number, version.modified),
                    |r| send(HistoryMsg::Opened(r)),
                )
            }
            HistoryMsg::Opened(result) => match result {
                Ok(path) => self.open_with_default_handler(path),
                Err(error) => {
                    if let Some(d) = self.versions_dialog() {
                        d.error = Some(error);
                    }
                    Task::none()
                }
            },
            HistoryMsg::SaveSelected => {
                let Some((path, version)) = self.selected_version() else {
                    return Task::none();
                };
                if let Some(d) = self.versions_dialog() {
                    d.busy = true;
                }
                Task::perform(
                    history::save_copy(path, version.number, version.taken, version.modified),
                    |r| {
                        send(HistoryMsg::Done(r.map(|saved| {
                            crate::i18n::format(
                                "files-version-saved-as",
                                &[("name", name_of(&saved))],
                            )
                        })))
                    },
                )
            }
            HistoryMsg::RestoreSelected => {
                let Some((path, version)) = self.selected_version() else {
                    return Task::none();
                };
                if let Some(d) = self.versions_dialog() {
                    d.busy = true;
                }
                let name = name_of(&path);
                Task::perform(
                    history::restore(path, version.number, self.home_data_dir.clone()),
                    move |r| {
                        send(HistoryMsg::Done(r.map(|_| {
                            crate::i18n::format("files-version-restored", &[("name", name.clone())])
                        })))
                    },
                )
            }
            HistoryMsg::Browse(folder) => {
                self.model.close_properties();
                self.model.history = Some(Dialog::Browse(BrowseDialog {
                    folder,
                    snapshots: None,
                    at: 0,
                    items: None,
                    chosen: Vec::new(),
                    busy: false,
                    error: None,
                }));
                Task::perform(history::snapshots(), |r| send(HistoryMsg::Snapshots(r)))
            }
            HistoryMsg::Snapshots(result) => {
                let Some(d) = self.browse_dialog() else {
                    return Task::none();
                };
                d.snapshots = Some(result);
                self.load_items()
            }
            HistoryMsg::Step(older) => {
                let Some(d) = self.browse_dialog() else {
                    return Task::none();
                };
                let count = d
                    .snapshots
                    .as_ref()
                    .and_then(|s| s.as_ref().ok())
                    .map_or(0, Vec::len);
                d.at = if older {
                    (d.at + 1).min(count.saturating_sub(1))
                } else {
                    d.at.saturating_sub(1)
                };
                self.load_items()
            }
            HistoryMsg::Items(number, result) => {
                if let Some(d) = self.browse_dialog()
                    && d.snapshots
                        .as_ref()
                        .and_then(|s| s.as_ref().ok())
                        .and_then(|s| s.get(d.at))
                        .is_some_and(|s| s.0 == number)
                {
                    d.items = Some(result);
                }
                Task::none()
            }
            HistoryMsg::Choose(name, on) => {
                if let Some(d) = self.browse_dialog() {
                    d.chosen.retain(|n| *n != name);
                    if on {
                        d.chosen.push(name);
                    }
                }
                Task::none()
            }
            HistoryMsg::RestoreChosen => {
                let Some(d) = self.browse_dialog() else {
                    return Task::none();
                };
                let Some(Ok(snapshots)) = &d.snapshots else {
                    return Task::none();
                };
                let Some((number, _)) = snapshots.get(d.at).copied() else {
                    return Task::none();
                };
                let paths: Vec<PathBuf> = d.chosen.iter().map(|n| d.folder.join(n)).collect();
                d.busy = true;
                d.error = None;
                let home_data_dir = self.home_data_dir.clone();
                Task::perform(
                    async move {
                        let mut restored = 0;
                        for path in paths {
                            restored +=
                                history::restore(path, number, home_data_dir.clone()).await?;
                        }
                        Ok(crate::i18n::count("files-items-restored", restored as u64))
                    },
                    |r| send(HistoryMsg::Done(r)),
                )
            }
            HistoryMsg::Done(result) => match result {
                Ok(text) => {
                    self.model.history = None;
                    self.post_status(text, ToastTone::Success);
                    Task::none()
                }
                Err(error) => {
                    match self.model.history.as_mut() {
                        Some(Dialog::Versions(d)) => {
                            d.busy = false;
                            d.error = Some(error);
                        }
                        Some(Dialog::Browse(d)) => {
                            d.busy = false;
                            d.error = Some(error);
                        }
                        None => self.post_status(error, ToastTone::Error),
                    }
                    Task::none()
                }
            },
            HistoryMsg::Exclude(path, excluded) => {
                Task::perform(history::set_excluded(path.clone(), excluded), move |r| {
                    send(HistoryMsg::Excluded(path, r))
                })
            }
            HistoryMsg::Excluded(path, result) => {
                let name = name_of(&path);
                match result {
                    Ok(true) => self.post_status(
                        crate::i18n::format("files-name-is-no-longer-backed-up", &[("name", name)]),
                        ToastTone::Success,
                    ),
                    Ok(false) => self.post_status(
                        crate::i18n::format("files-name-is-backed-up-again", &[("name", name)]),
                        ToastTone::Success,
                    ),
                    Err(error) => self.post_status(error, ToastTone::Error),
                }
                Task::perform(history::refresh(), |r| send(HistoryMsg::Refreshed(r)))
            }
            HistoryMsg::BackUpNow => Task::perform(history::back_up_now(), |r| {
                send(HistoryMsg::BackupStarted(r))
            }),
            HistoryMsg::BackupStarted(result) => {
                match result {
                    Ok(()) => self.post_status(
                        crate::i18n::tr("Backup started. You can keep working while it runs."),
                        ToastTone::Success,
                    ),
                    Err(error) => self.post_status(error, ToastTone::Error),
                }
                Task::none()
            }
            HistoryMsg::Forgotten(result) => {
                match result {
                    Ok((_, _, queued)) => self.post_status(
                        if queued {
                            crate::i18n::tr("Removed from earlier versions. The backup drive drops it at its next backup.")
                        } else {
                            crate::i18n::tr("Removed from earlier versions.")
                        },
                        ToastTone::Success,
                    ),
                    Err(error) => self.post_status(error, ToastTone::Error),
                }
                Task::none()
            }
        }
    }
}
