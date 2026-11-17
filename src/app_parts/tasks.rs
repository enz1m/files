// Background task ownership: launch, mime, unmount, reload, and watches.

use super::*;

// How long folder changes are gathered before one listing answers them.
const WATCH_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

// How long a navigation's listing may take before the view says it is loading.
const LISTING_SLOW_AFTER: std::time::Duration = std::time::Duration::from_millis(400);

impl App {
    // Owns a new, independent desktop launch and returns its worker token.
    // Launches never cancel each other; only app drop or the matching
    // completion releases an entry.
    pub(crate) fn begin_launch(&mut self) -> (u64, CancelToken) {
        self.next_launch_generation = self
            .next_launch_generation
            .checked_add(1)
            .expect("desktop launch generation exhausted");
        let generation = self.next_launch_generation;
        let cancel = CancelToken::new();
        self.launch_operations.insert(
            generation,
            HelperOwner {
                cancel: cancel.share(),
                handle: None,
            },
        );
        (generation, cancel)
    }

    // Cancels and aborts every in-flight launch; used on app drop.
    pub(crate) fn cancel_all_launches(&mut self) {
        for (_, owner) in std::mem::take(&mut self.launch_operations) {
            owner.cancel.cancel(CancelCause::Cancelled);
            if let Some(handle) = owner.handle {
                handle.abort();
            }
        }
    }

    // Attaches the abortable handle to a begun launch; a stale generation
    // finds nothing.
    pub(crate) fn own_launch(&mut self, generation: u64, task: Task<Msg>) -> Task<Msg> {
        let (task, handle) = task.abortable();
        if let Some(owner) = self.launch_operations.get_mut(&generation) {
            owner.handle = Some(handle);
        }
        task
    }

    // Release launch ownership only for the operation that actually finished.
    // Returns false for a stale/unknown generation so its result is dropped.
    pub(crate) fn finish_launch(&mut self, generation: u64) -> bool {
        self.launch_operations.remove(&generation).is_some()
    }

    // Begins the replaceable `xdg-mime` chooser query, superseding any in
    // flight. Durable default writes are owned separately and unaffected.
    pub(crate) fn begin_mime_query(&mut self) -> (u64, CancelToken, network::OperationCancel) {
        self.cancel_mime_query();
        self.next_mime_generation = self
            .next_mime_generation
            .checked_add(1)
            .expect("desktop MIME generation exhausted");
        let generation = self.next_mime_generation;
        let process_cancel = CancelToken::new();
        let scan_cancel = network::OperationCancel::default();
        self.mime_query_cancel = Some((generation, process_cancel.share(), scan_cancel.clone()));
        (generation, process_cancel, scan_cancel)
    }

    // Cancels the in-flight chooser query; confirmed durable writes continue.
    pub(crate) fn cancel_mime_query(&mut self) {
        if let Some((_, process_cancel, scan_cancel)) = self.mime_query_cancel.take() {
            process_cancel.cancel(CancelCause::Cancelled);
            scan_cancel.cancel();
        }
        if let Some((_, handle)) = self.mime_query_operation.take() {
            handle.abort();
        }
    }

    // Make `task` the owned chooser query.
    pub(crate) fn own_mime_query(&mut self, generation: u64, task: Task<Msg>) -> Task<Msg> {
        let (task, handle) = task.abortable();
        self.mime_query_operation = Some((generation, handle));
        task
    }

    // Release chooser-query ownership only when its matching completion
    // arrives; a stale query reply is dropped.
    pub(crate) fn finish_mime_query(&mut self, generation: u64) -> bool {
        if !matches!(
            self.mime_query_cancel.as_ref(),
            Some((current, _, _)) if *current == generation
        ) {
            return false;
        }
        self.mime_query_operation = None;
        self.mime_query_cancel = None;
        true
    }

    // Begins an independent durable `xdg-mime default` write, cancelled only
    // by app drop.
    pub(crate) fn begin_mime_write(&mut self) -> (u64, CancelToken) {
        self.next_mime_generation = self
            .next_mime_generation
            .checked_add(1)
            .expect("desktop MIME generation exhausted");
        let generation = self.next_mime_generation;
        let cancel = CancelToken::new();
        self.mime_write_operations.insert(
            generation,
            HelperOwner {
                cancel: cancel.share(),
                handle: None,
            },
        );
        (generation, cancel)
    }

    // Attach the abortable task handle to an already-begun durable write.
    pub(crate) fn own_mime_write(&mut self, generation: u64, task: Task<Msg>) -> Task<Msg> {
        let (task, handle) = task.abortable();
        if let Some(owner) = self.mime_write_operations.get_mut(&generation) {
            owner.handle = Some(handle);
        }
        task
    }

    // Release durable-write ownership only for the write that finished.
    pub(crate) fn finish_mime_write(&mut self, generation: u64) -> bool {
        self.mime_write_operations.remove(&generation).is_some()
    }

    // Cancels and aborts every in-flight durable write; used on app drop.
    pub(crate) fn cancel_all_mime_writes(&mut self) {
        for (_, owner) in std::mem::take(&mut self.mime_write_operations) {
            owner.cancel.cancel(CancelCause::Cancelled);
            if let Some(handle) = owner.handle {
                handle.abort();
            }
        }
    }

    // Begins one independent unmount; the reducer keeps its token share until
    // the matching completion or app drop.
    pub(crate) fn begin_unmount(&mut self) -> (u64, CancelToken) {
        self.next_unmount_generation = self
            .next_unmount_generation
            .checked_add(1)
            .expect("unmount generation exhausted");
        let generation = self.next_unmount_generation;
        let cancel = CancelToken::new();
        self.unmount_operations.insert(
            generation,
            HelperOwner {
                cancel: cancel.share(),
                handle: None,
            },
        );
        (generation, cancel)
    }

    pub(crate) fn own_unmount(&mut self, generation: u64, task: Task<Msg>) -> Task<Msg> {
        let (task, handle) = task.abortable();
        if let Some(owner) = self.unmount_operations.get_mut(&generation) {
            owner.handle = Some(handle);
        }
        task
    }

    pub(crate) fn finish_unmount(&mut self, generation: u64) -> bool {
        self.unmount_operations.remove(&generation).is_some()
    }

    pub(crate) fn cancel_all_unmounts(&mut self) {
        for (_, owner) in std::mem::take(&mut self.unmount_operations) {
            owner.cancel.cancel(CancelCause::Cancelled);
            if let Some(handle) = owner.handle {
                handle.abort();
            }
        }
    }

    // Reloads the current directory off-thread under a new
    // [`Model::begin_load`] generation, so an older reply cannot clobber it.
    pub(crate) fn reload(&mut self) -> Task<Msg> {
        if self.model.screen != Screen::Directory {
            return Task::none();
        }
        let path = self.model.cwd.clone();
        let generation = self.model.begin_load();
        self.dir_load_in_flight = true;
        let policy = model::ProjectionPolicy::from_model(&self.model);
        let load = if let Some(adopted) = self.listings_in_flight.get_mut(&path) {
            // Already being listed: that listing answers for this generation,
            // and one more listing follows it in case it started too early.
            *adopted = generation;
            self.dir_watch_pending_rescan = true;
            Task::none()
        } else {
            self.listings_in_flight.insert(path.clone(), generation);
            Task::perform(
                load_prepared_dir(path.clone(), policy.clone()),
                move |(mtime, result)| Msg::DirPrepared {
                    path: path.clone(),
                    generation,
                    policy,
                    mtime,
                    result,
                },
            )
        };
        // A navigation's listing that takes a while shows a loading state.
        let load = if self.model.listing_pending {
            Task::batch([
                load,
                Task::perform(crate::shell_work::delay(LISTING_SLOW_AFTER), move |()| {
                    Msg::ListingSlow { generation }
                }),
            ])
        } else {
            load
        };
        if !std::mem::take(&mut self.model.scroll_reset_owed) {
            return load;
        }
        // The scrollables keep their own offset across a navigation.
        let top = iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 };
        Task::batch([
            iced::widget::operation::scroll_to(
                iced::widget::Id::new(crate::view::list::LIST_SCROLLER_ID),
                top,
                iced::advanced::widget::operation::Animation::Instant,
            ),
            iced::widget::operation::scroll_to(
                iced::widget::Id::new(crate::view::grid::GRID_SCROLLER_ID),
                top,
                iced::advanced::widget::operation::Animation::Instant,
            ),
            load,
        ])
    }

    // Clearing ownership before submitting makes every delivery from an older
    // generation inadmissible, including A→B→A reuse of the same pathname.
    fn submit_dir_watch_target(&mut self, target: Option<PathBuf>) {
        self.dir_watch_generation = self.dir_watch_generation.wrapping_add(1);
        self.current_dir_watch = None;
        self.dir_watch_pending_rescan = false;
        self.dir_watch.submit(dir_watch_worker::InstallRequest {
            generation: self.dir_watch_generation,
            target,
        });
    }

    // A refused or failed-revalidation home-Trash authority still bumps the
    // generation, surfaces the reason and schedules no I/O; it is never
    // reacquired by pathname.
    pub(crate) fn dispatch_trash_load(&mut self) -> Task<Msg> {
        self.trash_load_generation = self.trash_load_generation.wrapping_add(1);
        if !self.enforce_home_trash_authority() {
            return Task::none();
        }
        let generation = self.trash_load_generation;
        let authority = self.build_home_trash_root_authority();
        Task::perform(
            load_trash(self.home_data_dir.clone(), authority),
            move |listing| Msg::TrashLoaded {
                generation,
                listing,
            },
        )
    }

    // `None` when the home authority or either `files/`/`info/` sub-root is
    // unavailable; those rows are then refused per entry.
    fn build_home_trash_root_authority(&self) -> Option<private_data::TrashRootAuthority> {
        let trash_root = std::sync::Arc::clone(self.home_trash.root()?);
        let handles = private_data::TrashRootHandles::from_trash(trash_root).ok()?;
        Some(private_data::TrashRootAuthority::Home(handles))
    }

    // `None` fails closed: admitted production listings always carry `Some`.
    pub(crate) fn authority_ok_for(entry: &trash::TrashEntry) -> bool {
        entry
            .retained_root
            .as_ref()
            .is_some_and(|authority| authority.revalidate().is_ok())
    }

    // On revalidation failure the authority becomes `Unavailable` for good;
    // `None` skips durable cache I/O while in-memory decoding continues.
    pub(crate) fn durable_cache_root_for_dispatch(
        &mut self,
    ) -> Option<std::sync::Arc<private_data::ThumbnailNestedRoots>> {
        match &self.thumbnail_cache {
            private_data::ThumbnailCacheAuthority::Unavailable => None,
            private_data::ThumbnailCacheAuthority::Available(root) => match root.revalidate() {
                Ok(()) => self
                    .thumbnail_cache
                    .nested_roots(thumbnail::FAILURE_CACHE_NAMESPACE)
                    .map(std::sync::Arc::new),
                Err(_) => {
                    self.thumbnail_cache = private_data::ThumbnailCacheAuthority::Unavailable;
                    self.post_status(
                        crate::i18n::tr(private_data::THUMBNAIL_CACHE_REFUSED),
                        ToastTone::Warning,
                    );
                    None
                }
            },
        }
    }

    // On revalidation failure the authority becomes `Unavailable` and the
    // path-free reason is posted.
    fn enforce_home_trash_authority(&mut self) -> bool {
        let refusal_summary = match &self.home_trash {
            private_data::HomeTrashAuthority::Unavailable(reason) => Some(reason.user_summary()),
            private_data::HomeTrashAuthority::Available(root) => match root.revalidate() {
                Ok(()) => None,
                Err(_) => {
                    let reason = private_data::HomeTrashUnavailable::RootRefused;
                    let summary = reason.user_summary();
                    self.home_trash = private_data::HomeTrashAuthority::Unavailable(reason);
                    // In-flight scan replies become stale.
                    self.trash_load_generation = self.trash_load_generation.wrapping_add(1);
                    Some(summary)
                }
            },
        };
        if let Some(summary) = refusal_summary {
            self.post_status(summary, ToastTone::Warning);
            self.model.trash_entries.clear();
            return false;
        }
        true
    }

    pub(crate) fn arm_dir_watch(&mut self) {
        let target = (self.model.screen == Screen::Directory).then(|| self.model.cwd.clone());
        self.submit_dir_watch_target(target);
    }

    fn retire_dir_watch(&mut self) {
        self.submit_dir_watch_target(None);
    }

    pub(crate) fn consume_watch_armed(
        &mut self,
        generation: u64,
        outcome: WatchInstallOutcome,
    ) -> Task<Msg> {
        if generation != self.dir_watch_generation {
            // The worker alone owns handles; its next request retires this one.
            return Task::none();
        }
        match outcome {
            WatchInstallOutcome::Armed(id) => {
                self.current_dir_watch = Some(id);
                self.dir_watch_vanished = false;
                // Changes between the listing's read and the watch's
                // admission would be missed. The folder's modification time
                // tells: one stat after both, and a new listing only if it
                // moved, instead of always listing twice.
                if self.dir_load_in_flight {
                    self.verify_after_listing = true;
                    Task::none()
                } else {
                    self.verify_listing()
                }
            }
            WatchInstallOutcome::Refused(refusal) => {
                // The folder itself is readable: say live updates are off
                // (the inotify limit, a filesystem without them) and keep the
                // listing on screen.
                self.post_status(
                    crate::i18n::format(
                        "files-live-folder-updates-are-unavailable-refusal-1215ad11",
                        &[("refusal", (refusal).to_string())],
                    ),
                    ToastTone::Warning,
                );
                Task::none()
            }
            WatchInstallOutcome::NotOnDirectoryScreen => Task::none(),
        }
    }

    pub(crate) fn consume_watch_deliveries(
        &mut self,
        deliveries: Vec<enzui_core::file_tree_watch::WatchDelivery>,
    ) -> Task<Msg> {
        use enzui_core::file_tree_watch::WatchOutcome;
        let Some(current) = self.current_dir_watch else {
            return Task::none();
        };
        let mut needs_rescan = false;
        let mut vanished = false;
        for delivery in deliveries {
            if delivery.watch != current {
                continue;
            }
            match delivery.outcome {
                WatchOutcome::Changes(_) | WatchOutcome::Overflow => needs_rescan = true,
                WatchOutcome::Vanished => vanished = true,
                WatchOutcome::ProbeFailed(msg) => {
                    if self.dir_watch_warned != self.dir_watch_generation {
                        self.dir_watch_warned = self.dir_watch_generation;
                        self.post_status(msg, ToastTone::Warning);
                    }
                }
            }
        }
        if vanished {
            // A stale `DirLoaded` must not repopulate the deleted folder.
            let _new_generation = self.model.begin_load();
            self.dir_load_in_flight = false;
            self.dir_watch_vanished = true;
            self.retire_dir_watch();
            self.model.entries.clear();
            self.model.load_error =
                Some(crate::i18n::tr("This folder no longer exists.").to_owned());
            self.model.refresh_visible_provider();
            return Task::none();
        }
        if !needs_rescan {
            return Task::none();
        }
        if self.dir_load_in_flight {
            self.dir_watch_pending_rescan = true;
            return Task::none();
        }
        // Changes are gathered for a moment and answered with one listing, so
        // a file being written does not re-list the folder per write.
        if self.dir_watch_rescan_due {
            return Task::none();
        }
        self.dir_watch_rescan_due = true;
        let generation = self.dir_watch_generation;
        Task::perform(crate::shell_work::delay(WATCH_SETTLE), move |()| {
            Msg::WatchRescanDue { generation }
        })
    }

    // Stats the current folder off-thread to compare with its listing.
    pub(crate) fn verify_listing(&mut self) -> Task<Msg> {
        if self.model.screen != Screen::Directory {
            return Task::none();
        }
        let path = self.model.cwd.clone();
        Task::perform(
            crate::shell_work::run(move || {
                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                (path, mtime)
            }),
            |(path, mtime)| Msg::ListingVerified { path, mtime },
        )
    }

    pub(crate) fn consume_listing_verified(
        &mut self,
        path: PathBuf,
        mtime: Option<std::time::SystemTime>,
    ) -> Task<Msg> {
        if self.model.screen != Screen::Directory || path != self.model.cwd {
            return Task::none();
        }
        let listed = self
            .listed_mtime
            .as_ref()
            .filter(|(listed, _)| *listed == path);
        match (listed, mtime) {
            (Some((_, Some(before))), Some(now)) if *before == now => Task::none(),
            _ if self.dir_load_in_flight => {
                self.dir_watch_pending_rescan = true;
                Task::none()
            }
            _ => self.reload(),
        }
    }

    pub(crate) fn consume_watch_rescan_due(&mut self, generation: u64) -> Task<Msg> {
        self.dir_watch_rescan_due = false;
        if generation != self.dir_watch_generation || self.model.screen != Screen::Directory {
            return Task::none();
        }
        if self.dir_load_in_flight {
            self.dir_watch_pending_rescan = true;
            Task::none()
        } else {
            self.reload()
        }
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn install_real_dir_watch_backend(&mut self) {
        use enzui_core::file_tree_watch::{DirectoryWatchService, linux::LinuxInotifyBackend};
        let service = match LinuxInotifyBackend::new() {
            Ok(backend) => DirectoryWatchService::new(backend),
            Err(_) => DirectoryWatchService::new(enzui_core::file_tree_watch::NoopBackend),
        };
        self.dir_watch = dir_watch_worker::DirWatchWorker::spawn(std::sync::Arc::new(service));
    }
}
