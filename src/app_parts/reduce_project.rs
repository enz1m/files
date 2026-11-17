// The open folder's repository, read off the UI thread, and the explicit
// actions on it.

use super::*;
use crate::project::{self, ProjectMsg, StatusView};

impl App {
    // Look at the folder now on screen: its repository and branch first,
    // then (if it has a work tree) its changes. A newer folder cancels this.
    pub(crate) fn project_probe(&mut self) -> Task<Msg> {
        let dir = self.model.cwd.clone();
        if crate::services::is_virtual(&dir) {
            self.model.project.begin(&dir);
            self.model.project.current = None;
            return Task::none();
        }
        let (generation, _cancel) = self.model.project.begin(&dir);
        Task::perform(
            project::run(move || {
                let trusted = project::trust_path()
                    .map(|path| project::load_trusted(&path))
                    .unwrap_or_default();
                project::find(dir, trusted)
            }),
            move |found| {
                Msg::Project(ProjectMsg::Found {
                    generation,
                    found: found.flatten().map(Box::new),
                })
            },
        )
    }

    pub(crate) fn project_message(&mut self, message: ProjectMsg) -> Task<Msg> {
        match message {
            ProjectMsg::Found { generation, found } => {
                if !self.model.project.is_current(generation) {
                    return Task::none();
                }
                let Some(found) = found else {
                    self.model.project.current = None;
                    return Task::none();
                };
                let (info, repository) = *found;
                let checking = info.status == StatusView::Checking;
                self.model.project.current = Some(info);
                let Some(cancel) = self.model.project.cancel_flag().filter(|_| checking) else {
                    return Task::none();
                };
                Task::perform(
                    project::run(move || project::status(&repository, &cancel)),
                    move |status| {
                        Msg::Project(ProjectMsg::StatusRead {
                            generation,
                            status: status.unwrap_or(StatusView::Unavailable),
                        })
                    },
                )
            }
            ProjectMsg::CountChanges => {
                let Some(info) = self.model.project.current.as_mut() else {
                    return Task::none();
                };
                if info.status != StatusView::OnRequest {
                    return Task::none();
                }
                info.status = StatusView::Checking;
                let dir = info.dir.clone();
                let (generation, cancel) = self.model.project.begin(&dir);
                Task::perform(
                    project::run(move || {
                        enzui_services::vcs::git::discover(&dir)
                            .map(|repository| project::status(&repository, &cancel))
                    }),
                    move |status| {
                        Msg::Project(ProjectMsg::StatusRead {
                            generation,
                            status: status.flatten().unwrap_or(StatusView::Unavailable),
                        })
                    },
                )
            }
            ProjectMsg::StatusRead { generation, status } => {
                if self.model.project.is_current(generation)
                    && let Some(info) = &mut self.model.project.current
                {
                    info.status = status;
                }
                Task::none()
            }
            ProjectMsg::CopyRelativePath => {
                // The selected item when there is exactly one, else the folder.
                let target = match self.model.selection.len() {
                    1 => self.model.selection.iter().next().cloned(),
                    _ => None,
                }
                .unwrap_or_else(|| self.model.cwd.clone());
                match self.model.project.relative_path(&target) {
                    Some(relative) => iced::clipboard::write(relative).discard(),
                    None => Task::none(),
                }
            }
            ProjectMsg::CopyRemote => {
                match self
                    .model
                    .project
                    .current
                    .as_ref()
                    .and_then(|info| info.remote.clone())
                {
                    Some(url) => iced::clipboard::write(url).discard(),
                    None => Task::none(),
                }
            }
            ProjectMsg::OpenRemote => {
                let page = self
                    .model
                    .project
                    .current
                    .as_ref()
                    .and_then(|info| info.remote.as_deref())
                    .and_then(project::remote_web_url);
                match page {
                    // The desktop's default handler opens a URL as it opens a
                    // file.
                    Some(page) => self.open_with_default_handler(std::path::PathBuf::from(page)),
                    None => Task::none(),
                }
            }
            ProjectMsg::Trust | ProjectMsg::Untrust => {
                let trusted = matches!(message, ProjectMsg::Trust);
                let (Some(info), Some(path)) =
                    (self.model.project.current.as_mut(), project::trust_path())
                else {
                    return Task::none();
                };
                info.trusted = trusted;
                let root = info.root.clone();
                let name = root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.post_status(
                    if trusted {
                        crate::i18n::format(
                            "files-marked-name-trusted-in-files-nothing-2909140e",
                            &[("name", (name))],
                        )
                    } else {
                        crate::i18n::format(
                            "files-name-is-no-longer-marked-trusted-b68d56e9",
                            &[("name", (name))],
                        )
                    },
                    ToastTone::Info,
                );
                Task::perform(
                    project::run(move || {
                        project::set_trusted(&path, &root, trusted)
                            .map_err(|error| error.to_string())
                    }),
                    |result| {
                        Msg::Project(ProjectMsg::TrustSaved(result.unwrap_or_else(|| {
                            Err(crate::i18n::tr("the worker stopped").to_owned())
                        })))
                    },
                )
            }
            ProjectMsg::OpenProject => {
                let Some(info) = self
                    .model
                    .project
                    .current
                    .as_ref()
                    .filter(|info| info.open_project)
                else {
                    return Task::none();
                };
                let root = info.root.clone();
                Task::perform(
                    project::run(move || {
                        project::open_project(Path::new(project::OPEN_PROJECT_HELPER), &root)
                    }),
                    |result| {
                        Msg::Project(ProjectMsg::ProjectOpened(result.unwrap_or_else(|| {
                            Err(crate::i18n::tr("the worker stopped").to_owned())
                        })))
                    },
                )
            }
            ProjectMsg::ProjectOpened(Ok(())) => Task::none(),
            ProjectMsg::ProjectOpened(Err(reason)) => {
                self.post_status(reason, ToastTone::Error);
                Task::none()
            }
            ProjectMsg::TrustSaved(Ok(())) => Task::none(),
            ProjectMsg::TrustSaved(Err(error)) => {
                self.post_status(
                    crate::i18n::format(
                        "files-couldn-t-save-the-trusted-projects-d0a99264",
                        &[("error", (error))],
                    ),
                    ToastTone::Error,
                );
                Task::none()
            }
        }
    }
}
