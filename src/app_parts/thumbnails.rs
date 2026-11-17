// Thumbnail scheduling.

use super::*;

impl App {
    pub(crate) fn schedule_inspector(&mut self) -> Task<Msg> {
        if !self.model.details_open || self.model.selection.len() != 1 {
            return Task::none();
        }
        let Some(path) = self.model.selection.iter().next().cloned() else {
            return Task::none();
        };
        if self.model.inspector_pending.as_ref() == Some(&path) {
            return Task::none();
        }
        self.model.inspector_pending = Some(path.clone());
        if self
            .model
            .inspector_properties
            .as_ref()
            .is_some_and(|shown| shown.path != path)
        {
            self.model.inspector_properties = None;
        }
        Task::perform(
            crate::shell_work::run(move || {
                let info = services::build_properties(&path).ok();
                Msg::InspectorPropertiesReady { path, info }
            }),
            |message| message,
        )
    }

    pub(crate) fn schedule_thumbnails(&mut self) -> Task<Msg> {
        use std::collections::{BTreeMap, BTreeSet};
        if !self.model.features.enabled(features::FeatureId::Thumbnails) {
            for (_, handle) in self
                .thumbnail_operations
                .drain()
                .collect::<std::collections::BTreeMap<_, _>>()
            {
                handle.abort();
            }
            self.thumbnail_queue.clear();
            self.model.thumbnails.clear();
            self.preview_cache.clear();
            return Task::none();
        }
        // Only realised entries' metadata: a per-path `.find()` in the full
        // inventory would cost O(total) on every scroll.
        let mut desired = BTreeMap::new();
        if self.model.screen == Screen::Directory && self.model.view_mode == model::ViewMode::Grid {
            let viewport = self.window_height.max(480.0);
            let geometry = self.model.grid_layout.get();
            if !geometry.is_resolved() {
                return Task::none();
            }
            let realised = view::grid::realised_range_for_geometry(
                self.model.visible_provider.len(),
                geometry,
                viewport,
                self.model.grid_scroll_y,
            );
            for index in realised {
                let Some(projected) = self.model.visible_provider.item(index as u64) else {
                    continue;
                };
                if let Some(entry) = self.model.projected_entry(&projected) {
                    desired.insert(
                        entry.path.clone(),
                        (entry.mime.clone(), entry.size, entry.modified),
                    );
                }
            }
        }
        // The inspector is a second consumer even in list view.
        if self.model.details_open
            && self.model.selection.len() == 1
            && let Some(path) = self.model.selection.iter().next()
            && let Some(entry) = self.model.entry_for_path(path)
        {
            desired.insert(
                entry.path.clone(),
                (entry.mime.clone(), entry.size, entry.modified),
            );
        }

        let leaving: BTreeSet<_> = self
            .thumbnail_operations
            .keys()
            .chain(self.thumbnail_queue.keys())
            .chain(self.model.thumbnails.keys())
            .filter(|path| !desired.contains_key(*path))
            .cloned()
            .collect();
        for path in leaving {
            if let Some(handle) = self.thumbnail_operations.remove(&path) {
                handle.abort();
            }
            self.thumbnail_queue.remove(&path);
            // The view keeps only the realised tiles' previews; a ready one
            // stays in the byte-bounded preview cache, which decides how
            // many decoded pixels outlive their tiles.
            self.model.thumbnails.remove(&path);
        }

        let mut tasks = Vec::new();
        for (path, (mime, size, modified)) in desired {
            if !thumbnail::eligible(&path, &mime, size) {
                continue;
            }
            let Some(stamp) = thumbnail::stamp(path.clone(), modified) else {
                continue;
            };
            if self
                .model
                .thumbnails
                .get(&path)
                .is_some_and(|state| state.mtime() == stamp.mtime)
                || self.thumbnail_operations.contains_key(&path)
                || self.thumbnail_queue.contains_key(&path)
            {
                continue;
            }
            // Seen before at this mtime: shown at once, no decode.
            if let Some(handle) = self.preview_cache.get(&(path.clone(), stamp.mtime)) {
                self.model.thumbnails.insert(
                    path,
                    model::ThumbnailState::Ready {
                        mtime: stamp.mtime,
                        handle,
                    },
                );
                continue;
            }
            self.model
                .thumbnails
                .insert(path.clone(), model::ThumbnailState::Loading(stamp.mtime));
            self.thumbnail_queue
                .insert(path, thumbnail::Request { stamp, mime });
        }

        // Promotion is the only place a blocking worker is created, which caps
        // active GStreamer pipelines at three instead of one per tile.
        let busy = self
            .thumbnail_operations
            .len()
            .max(thumbnail::running_decodes());
        let available = thumbnail::MAX_CONCURRENT_DECODES.saturating_sub(busy);
        let promote: Vec<_> = self
            .thumbnail_queue
            .keys()
            .take(available)
            .cloned()
            .collect();
        // `None` means the cache authority is unavailable: decode in memory with
        // no cache I/O. Revalidation failure here is permanent, so no later pass
        // reacquires by pathname.
        let cache_root_opt = self.durable_cache_root_for_dispatch();
        for path in promote {
            let Some(request) = self.thumbnail_queue.remove(&path) else {
                continue;
            };
            let cache = cache_root_opt.clone();
            let (task, handle) = Task::perform(
                crate::shell_work::run_cancellable(
                    crate::shell_work::Pool::Background,
                    move |cancelled| thumbnail::load(request, cache, cancelled),
                ),
                Msg::ThumbnailLoaded,
            )
            .abortable();
            self.thumbnail_operations.insert(path, handle);
            tasks.push(task);
        }
        Task::batch(tasks)
    }
}
