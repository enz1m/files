// Load, scroll, and navigation reducer arms.

use super::*;

impl App {
    pub(crate) fn nav_acceptance(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Acceptance(_) = message else {
            unreachable!("misrouted message in nav_acceptance");
        };
        Task::none()
    }

    pub(crate) fn nav_theme(&mut self, message: Msg) -> Task<Msg> {
        let Msg::Theme(event) = message else {
            unreachable!("misrouted message in nav_theme");
        };
        let before = self.themes.theme_generation();
        let task = self.themes.update(*event);
        debug_assert!(
            self.themes.theme_generation() >= before,
            "theme generations must be monotonic"
        );
        task.map(|event| Msg::Theme(Box::new(event)))
    }

    pub(crate) fn nav_a11y_wake(&mut self, message: Msg) -> Task<Msg> {
        let Msg::A11yWake(receipt) = message else {
            unreachable!("misrouted message in nav_a11y_wake");
        };
        enzui_widgets::a11y_actions::apply_accessibility_wake::<Msg>(receipt)
    }

    pub(crate) fn nav_dir_prepared(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DirPrepared {
            path,
            generation,
            policy,
            mtime,
            result,
        } = message
        else {
            unreachable!("misrouted directory snapshot");
        };
        let listed_path = path.clone();
        let generation_seen = generation;
        let (result, provider) = match result {
            Ok((entries, provider)) => (
                Ok(entries),
                (policy == model::ProjectionPolicy::from_model(&self.model) && !policy.recursive())
                    .then_some(provider),
            ),
            Err(error) => (Err(error), None),
        };
        let accepted_now = self.model.screen == Screen::Directory
            && self.model.cwd == listed_path
            && self
                .listings_in_flight
                .get(&listed_path)
                .map_or(generation_seen, |adopted| (*adopted).max(generation_seen))
                == self.model.load_generation;
        let task = self.apply_dir_loaded(
            Msg::DirLoaded {
                path,
                generation,
                result,
            },
            provider,
        );
        // Keep the accepted listing's modification time, and run the check
        // its watch asked for.
        if accepted_now {
            self.listed_mtime = Some((listed_path, mtime));
            if std::mem::take(&mut self.verify_after_listing) {
                return Task::batch([task, self.verify_listing()]);
            }
        }
        task
    }

    pub(crate) fn nav_dir_loaded(&mut self, message: Msg) -> Task<Msg> {
        self.apply_dir_loaded(message, None)
    }

    fn apply_dir_loaded(
        &mut self,
        message: Msg,
        provider: Option<model::FileEntryProvider>,
    ) -> Task<Msg> {
        let Msg::DirLoaded {
            path,
            generation,
            result,
        } = message
        else {
            unreachable!("misrouted message in nav_dir_loaded");
        };
        // Path and screen equality cannot reject an earlier read of the same
        // folder, so only the latest `generation` may overwrite or clear entries.
        // A reload that waited for this listing adopted it under its newer
        // generation.
        let generation = self
            .listings_in_flight
            .remove(&path)
            .map_or(generation, |adopted| adopted.max(generation));
        let accepted = self.model.screen == Screen::Directory
            && self.model.cwd == path
            && generation == self.model.load_generation;
        if accepted {
            self.dir_load_in_flight = false;
            self.model.listing_pending = false;
            self.model.listing_slow = false;
            match result {
                Ok(entries) => match provider {
                    Some(provider) => self.model.set_prepared_entries(entries, provider),
                    None => self.model.set_entries(entries),
                },
                Err(e) => {
                    self.model.entries.clear();
                    self.model.load_error = Some(e);
                    self.model.refresh_visible_provider();
                }
            }
            self.model.pending_copy_folders.retain(|folder| {
                folder.activity.is_some()
                    || (self.dir_watch_pending_rescan
                        && folder.path.parent() == Some(path.as_path())
                        && !self
                            .model
                            .entries
                            .iter()
                            .any(|entry| entry.path == folder.path))
            });
            if self.dir_watch_pending_rescan {
                self.dir_watch_pending_rescan = false;
                let reload = self.reload();
                return Task::batch([reload, self.main_window_scope()]);
            }
        }

        if accepted {
            // Select and focus an entry a completed operation named.
            let focus = match self.model.focus_after_load.take() {
                Some(path) if self.model.entries.iter().any(|entry| entry.path == path) => {
                    let index = self.model.visible_provider.rows.iter().position(|row| {
                        self.model
                            .projected_entry(row)
                            .is_some_and(|entry| entry.path == path)
                    });
                    let rename = self.model.rename_after_load.take() == Some(path.clone());
                    self.model.select_only(path);
                    let reveal =
                        index.map_or_else(Task::none, |index| self.reveal_active_row(index));
                    if rename {
                        // After the reveal, whose row focus would otherwise
                        // take the keyboard from the name field.
                        reveal.chain(self.update(Msg::BeginBulkRename))
                    } else {
                        reveal
                    }
                }
                _ => {
                    self.model.rename_after_load = None;
                    Task::none()
                }
            };
            // Learn the listed folders' identities for their colour tags.
            let has_tags = !self.model.folder_colors.get().records().is_empty();
            let folders: Vec<PathBuf> = self
                .model
                .entries
                .iter()
                .filter(|entry| has_tags && entry.kind == model::EntryKind::Dir)
                .map(|entry| entry.path.clone())
                .collect();
            let identities = if folders.is_empty() {
                Task::none()
            } else {
                let dir = self.model.cwd.clone();
                Task::perform(crate::folder_colors::identify(folders), move |identities| {
                    Msg::FolderIdentitiesScanned { dir, identities }
                })
            };
            // Read without running anything the folder configures.
            let project = self.project_probe();
            // Preview windows show pinned files and never observe the directory listing.
            return Task::batch([focus, self.main_window_scope(), identities, project]);
        }
        Task::none()
    }

    pub(crate) fn nav_drives_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DrivesLoaded(drives) = message else {
            unreachable!("misrouted message in nav_drives_loaded");
        };
        self.model.drives = drives;
        // Removable media refresh with the mounted drives.
        self.enumerate_devices()
    }

    pub(crate) fn nav_list_scrolled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ListScrolled(offset) = message else {
            unreachable!("misrouted message in nav_list_scrolled");
        };
        self.model.list_scroll_y = offset.max(0.0);
        Task::none()
    }

    pub(crate) fn nav_grid_scrolled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::GridScrolled(offset) = message else {
            unreachable!("misrouted message in nav_grid_scrolled");
        };
        self.model.grid_scroll_y = offset.max(0.0);
        Task::none()
    }

    pub(crate) fn nav_downloads_scrolled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DownloadsScrolled(offset) = message else {
            unreachable!("misrouted message in nav_downloads_scrolled");
        };
        self.model.downloads_scroll_y = offset.max(0.0);
        Task::none()
    }

    pub(crate) fn nav_thumbnail_loaded(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ThumbnailLoaded(outcome) = message else {
            unreachable!("misrouted message in nav_thumbnail_loaded");
        };
        self.thumbnail_operations.remove(&outcome.stamp.path);
        // Ignore an edited/reloaded file's stale reply.
        let current = self
            .model
            .entries
            .iter()
            .find(|entry| entry.path == outcome.stamp.path)
            .and_then(|entry| thumbnail::stamp(entry.path.clone(), entry.modified));
        if current.as_ref() == Some(&outcome.stamp) {
            let state = match outcome.result {
                Ok(handle) => {
                    self.preview_cache.insert(
                        (outcome.stamp.path.clone(), outcome.stamp.mtime),
                        handle.clone(),
                    );
                    model::ThumbnailState::Ready {
                        mtime: outcome.stamp.mtime,
                        handle,
                    }
                }
                Err(_) => model::ThumbnailState::Failed(outcome.stamp.mtime),
            };
            self.model.thumbnails.insert(outcome.stamp.path, state);
        }
        Task::none()
    }

    pub(crate) fn nav_capture_prepared(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CapturePrepared {
            entries,
            thumbnails,
        } = message
        else {
            unreachable!("misrouted message in nav_capture_prepared");
        };
        self.model.set_entries(entries);
        for outcome in thumbnails {
            if let Ok(handle) = outcome.result {
                self.model.thumbnails.insert(
                    outcome.stamp.path,
                    model::ThumbnailState::Ready {
                        mtime: outcome.stamp.mtime,
                        handle,
                    },
                );
            }
        }
        let mut settings = desktop_identity().window_settings(MAIN_LAYOUT);
        if let Some(p) = self.placement.get_for(&WindowRole::Main) {
            settings.size = p.to_iced_size();
        }
        let (_id, open) = iced::window::open(settings);
        open.map(Msg::MainWindowOpened)
    }

    pub(crate) fn nav_navigate_to(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NavigateTo(path) = message else {
            unreachable!("misrouted message in nav_navigate_to");
        };
        // Closes an overlay drawer in the same gesture; an inline sidebar is
        // unaffected.
        self.model.nav.dismiss_overlay();
        self.model.navigate_to(path);
        self.arm_dir_watch();
        Task::batch([self.reload(), self.main_window_scope()])
    }

    pub(crate) fn nav_navigate_screen(&mut self, message: Msg) -> Task<Msg> {
        let Msg::NavigateScreen(screen, sentinel) = message else {
            unreachable!("misrouted message in nav_navigate_screen");
        };
        // Same drawer contract for virtual destinations.
        self.model.nav.dismiss_overlay();
        self.model.navigate_screen(screen.clone(), sentinel);
        self.after_screen_change(screen)
    }

    pub(crate) fn nav_go_back(&mut self, message: Msg) -> Task<Msg> {
        let Msg::GoBack = message else {
            unreachable!("misrouted message in nav_go_back");
        };
        if let Some((_, screen)) = self.model.go_back() {
            self.after_screen_change(screen)
        } else {
            Task::none()
        }
    }

    pub(crate) fn nav_go_forward(&mut self, message: Msg) -> Task<Msg> {
        let Msg::GoForward = message else {
            unreachable!("misrouted message in nav_go_forward");
        };
        if let Some((_, screen)) = self.model.go_forward() {
            self.after_screen_change(screen)
        } else {
            Task::none()
        }
    }

    pub(crate) fn nav_go_up(&mut self, message: Msg) -> Task<Msg> {
        let Msg::GoUp = message else {
            unreachable!("misrouted message in nav_go_up");
        };
        if self.model.go_up().is_some() {
            self.arm_dir_watch();
            Task::batch([self.reload(), self.main_window_scope()])
        } else {
            Task::none()
        }
    }

    pub(crate) fn nav_open_entry(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenEntry(path, kind) = message else {
            unreachable!("misrouted message in nav_open_entry");
        };
        if matches!(kind, model::EntryKind::Dir) {
            self.model.navigate_to(path);
            self.arm_dir_watch();
            Task::batch([self.reload(), self.main_window_scope()])
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("enzfiles"))
        {
            self.reduce_extensions(Msg::ReviewExtension(path))
        } else if crate::archive::ArchiveFormat::detect(&path).is_some() {
            // A sandbox's application database cannot establish the host's default.
            if self.open_route == portal_open::OpenRoute::Portal {
                return self.nav_open_file(path, true);
            }
            let mime = enzui_icons::file_type::FileTypeTable::host()
                .classify(&path.file_name().unwrap_or_default().to_string_lossy())
                .mime;
            if self
                .model
                .mime_defaults
                .get(&mime)
                .is_some_and(|spec| spec.program() != "gio")
            {
                return self.nav_open_file(path, true);
            }
            let query = std::sync::Arc::clone(&self.mime_query);
            let (generation, cancel) = self.begin_launch();
            let target = path;
            let task = Task::perform(
                crate::shell_work::run(move || {
                    query.default_application_cancellable(&mime, &cancel)
                }),
                move |result| Msg::ArchiveHandlerResolved {
                    generation,
                    path: target,
                    result,
                },
            );
            self.own_launch(generation, task)
        } else {
            self.nav_open_file(path, true)
        }
    }

    pub(crate) fn nav_archive_handler_resolved(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ArchiveHandlerResolved {
            generation,
            path,
            result,
        } = message
        else {
            unreachable!("misrouted archive handler result");
        };
        if !self.finish_launch(generation) || self.close_pending.is_some() {
            return Task::none();
        }
        match result {
            Ok(None) => self.reduce_extract_archive(Msg::ExtractArchive {
                archive: path,
                dest_dir: None,
            }),
            Ok(Some(id)) if id == format!("{}.desktop", crate::APP_ID) => self
                .reduce_extract_archive(Msg::ExtractArchive {
                    archive: path,
                    dest_dir: None,
                }),
            Ok(Some(_)) => self.nav_open_file(path, false),
            Err(error) => {
                self.post_status(
                    crate::i18n::format(
                        "files-could-not-find-the-archive-application-ce631b24",
                        &[("error", (error))],
                    ),
                    ToastTone::Error,
                );
                Task::none()
            }
        }
    }

    pub(crate) fn nav_open_file(&mut self, path: PathBuf, use_cached: bool) -> Task<Msg> {
        // A persisted Open With choice launches its structured command; the
        // default path uses the portal's OpenURI without a toast.
        let chosen = use_cached
            .then(|| {
                self.model
                    .entries
                    .iter()
                    .find(|e| e.path == path)
                    .and_then(|e| self.model.mime_defaults.get(&e.mime).cloned())
            })
            .flatten();
        if let Some(spec) = chosen {
            let launcher = std::sync::Arc::clone(&self.launcher);
            let (generation, cancel) = self.begin_launch();
            let task = Task::perform(
                crate::shell_work::run(move || {
                    launcher.run_command_cancellable(&spec, &path, &cancel)
                }),
                move |result| Msg::OpenLaunched { generation, result },
            );
            self.own_launch(generation, task)
        } else if self.open_route == portal_open::OpenRoute::DesktopLauncher {
            // A native install uses the desktop launcher: the portal cannot ask
            // an unparented, id-less caller which app to use. See
            // `portal_open::OpenRoute`.
            self.open_with_default_handler(path)
        } else {
            match self.file_opener.start(path.clone()) {
                Ok(_) => Task::none(),
                Err(error) if portal_open::should_fallback(&error) => {
                    self.open_with_default_handler(path)
                }
                Err(error) => {
                    if let Some(text) = portal_open::failure_text(&path, &Err(error)) {
                        self.post_status(text, ToastTone::Error);
                    }
                    Task::none()
                }
            }
        }
    }
}
