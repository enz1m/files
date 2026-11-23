// Model navigation, pins, selection, marquee, and clipboard.

use super::entries::glob_match;
use super::*;

impl Model {
    // Navigate to a real directory, pushing the current cwd onto the back
    // stack and clearing forward. Virtual screens are handled by
    // [`Model::navigate_screen`].
    pub(crate) fn navigate_to(&mut self, path: PathBuf) {
        if self.screen == Screen::Directory && path == self.cwd {
            return;
        }
        if self.screen == Screen::Directory || crate::network::is_server_path(&self.cwd) {
            self.back.push(self.cwd.clone());
        }
        self.forward.clear();
        self.cwd = path;
        self.screen = Screen::Directory;
        self.reset_transient();
    }

    // Move the built-in Home and XDG folder rows into the ordered pin model,
    // so they can be reordered or removed like user bookmarks. Called once at
    // construction.
    pub(crate) fn promote_default_places_to_pins(&mut self) {
        let mut defaults = Vec::new();
        self.places.retain(|place| {
            if matches!(place.kind, PlaceKind::Home | PlaceKind::XdgDir) {
                defaults.push(place.clone());
                false
            } else {
                true
            }
        });
        defaults.append(&mut self.bookmarks);
        self.bookmarks = defaults;
    }

    // Apply one stable-id reorder request to the pinned Places rail.
    // Invalid, cross-collection, non-Move and no-op requests are ignored.
    pub(crate) fn reorder_sidebar_pins(&mut self, request: &enzui_core::drag::DropRequest) -> bool {
        use enzui_core::drag::{CollectionId, DragOperation, ItemId, reorder};

        let collection = CollectionId::new(SIDEBAR_PINS_COLLECTION_ID);
        if request.source != collection
            || request.destination != collection
            || request.operation != DragOperation::Move
        {
            return false;
        }

        let order: Vec<ItemId> = self
            .bookmarks
            .iter()
            .map(|place| ItemId::new(place.id.clone()))
            .collect();
        let Ok(diff) = reorder(&order, &request.items, request.slot) else {
            return false;
        };
        if !diff.changed {
            return false;
        }

        let Some(reordered) = diff
            .order
            .iter()
            .map(|id| {
                self.bookmarks
                    .iter()
                    .find(|place| place.id == id.as_str())
                    .cloned()
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        self.bookmarks = reordered;
        true
    }

    // Apply a modifier-aware click against Trash's own display order.
    // Directory entries intentionally do not participate: a virtual Trash
    // screen can retain the last directory's rows while its listing loads.
    pub(crate) fn click_trash_selection(&mut self, path: PathBuf, modifiers: ClickModifiers) {
        let order = IndexedClickModel::new(
            self.trash_entries
                .iter()
                .map(|entry| entry.trashed_path.clone()),
        );
        self.selection.click(&order, &path, modifiers);
    }

    // Begin or continue an OS file drag hovering the window.
    //
    // Only a directory can be pinned, so a hover carrying a regular file
    // records no target and the sidebar offers no drop affordance.
    pub(crate) fn pin_hover_begin(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.pin_drop_hover = Some(path);
        }
    }

    // The drag left the window without dropping.
    pub(crate) fn pin_hover_end(&mut self) {
        self.pin_drop_hover = None;
    }

    pub(crate) fn pin_drop_active(&self) -> bool {
        self.pin_drop_hover.is_some()
    }

    // Pin `path` as a bookmark, the observable effect of a completed drop.
    //
    // Returns `Ok(true)` when a new bookmark was added, `Ok(false)` for a
    // no-op (non-dir, duplicate), or `Err` when adding the pin would exceed
    // the admission bounds the loader enforces.
    pub(crate) fn pin_path(&mut self, path: PathBuf) -> Result<bool, services::PinRefusal> {
        use std::os::unix::ffi::OsStrExt;

        self.pin_drop_hover = None;
        if !path.is_dir() {
            return Ok(false);
        }
        if self.bookmarks.iter().any(|b| b.path == path) {
            return Ok(false);
        }
        if self.bookmarks.len() >= services::PIN_COUNT_CAP {
            return Err(services::PinRefusal::TooManyPins {
                pins: self.bookmarks.len() + 1,
                cap: services::PIN_COUNT_CAP,
            });
        }
        let label = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let sanitized_label = label.replace(['\t', '\n'], " ");
        if sanitized_label.len() > services::PIN_LABEL_BYTE_CAP {
            return Err(services::PinRefusal::LabelTooLong {
                len: sanitized_label.len(),
                cap: services::PIN_LABEL_BYTE_CAP,
            });
        }
        let native_path_bytes = path.as_os_str().as_bytes().len();
        if native_path_bytes > services::PIN_PATH_BYTE_CAP {
            return Err(services::PinRefusal::PathTooLong {
                len: native_path_bytes,
                cap: services::PIN_PATH_BYTE_CAP,
            });
        }

        // The retained-metadata bound covers current plus proposed bytes.
        let new_entry_bytes = sanitized_label.len() + native_path_bytes;
        let proposed_aggregate = self.pin_retained_bytes + new_entry_bytes;
        if proposed_aggregate as u64 > services::PIN_FILE_BYTE_CAP {
            return Err(services::PinRefusal::FileTooLarge {
                bytes: proposed_aggregate as u64,
                cap: services::PIN_FILE_BYTE_CAP,
            });
        }

        // Serialization is validated before mutation.
        let candidate = Place {
            id: format!("bookmark:{}", path.to_string_lossy()),
            kind: PlaceKind::Bookmark,
            label,
            icon: "folder".into(),
            path,
        };
        let mut trial = self.bookmarks.clone();
        trial.push(candidate.clone());
        services::serialize_bookmarks(&trial)?;

        self.bookmarks.push(candidate);
        self.refresh_pin_counters();
        Ok(true)
    }

    // Recompute retained-row/byte counters from the current bookmarks.
    pub(crate) fn refresh_pin_counters(&mut self) {
        use std::os::unix::ffi::OsStrExt;
        self.pin_retained_rows = self.bookmarks.len();
        self.pin_retained_bytes = self
            .bookmarks
            .iter()
            .map(|p| p.label.len() + p.path.as_os_str().as_bytes().len())
            .sum();
    }

    // Whether `path` is already pinned to the Places sidebar. The context
    // menu's Pin/Unpin label and effect both read this predicate.
    pub(crate) fn is_pinned(&self, path: &std::path::Path) -> bool {
        self.bookmarks.iter().any(|b| b.path == path)
    }

    // Remove the pin for `path`, returning whether a bookmark was removed.
    // An already-unpinned path is a no-op.
    pub(crate) fn unpin_path(&mut self, path: &std::path::Path) -> bool {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.path != path);
        before != self.bookmarks.len()
    }

    // Navigate to a virtual screen (Computer, Recent, Trash), pushing
    // `sentinel` on the back stack so Back returns here.
    pub(crate) fn navigate_screen(&mut self, screen: Screen, sentinel: PathBuf) {
        if self.screen == screen && self.cwd == sentinel {
            return;
        }
        self.back.push(self.cwd.clone());
        self.forward.clear();
        // A page's row selection does not follow to another page.
        self.device_row_selected = None;
        self.last_device_press = None;
        self.cwd = sentinel;
        self.screen = screen;
        self.reset_transient();
    }

    // Resolve the [`Screen`] a place points at from its `path`.
    pub(crate) fn screen_for_place(place: &Place) -> Screen {
        match place.kind {
            PlaceKind::Computer => Screen::Computer,
            PlaceKind::Recent => Screen::Recent,
            PlaceKind::Trash => Screen::Trash,
            PlaceKind::Downloads => Screen::Downloads,
            PlaceKind::Network => Screen::Network,
            _ => Screen::Directory,
        }
    }

    pub(crate) fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub(crate) fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub(crate) fn can_go_up(&self) -> bool {
        (self.screen == Screen::Directory && self.cwd.parent().is_some())
            || crate::network::is_server_path(&self.cwd)
    }

    // Go back one entry. Returns the path that became cwd, or `None`.
    pub(crate) fn go_back(&mut self) -> Option<(PathBuf, Screen)> {
        let prev = self.back.pop()?;
        self.forward.push(self.cwd.clone());
        self.cwd.clone_from(&prev);
        self.screen = Self::screen_for_path(&prev);
        self.reset_transient();
        Some((prev, self.screen.clone()))
    }

    pub(crate) fn go_forward(&mut self) -> Option<(PathBuf, Screen)> {
        let next = self.forward.pop()?;
        self.back.push(self.cwd.clone());
        self.cwd.clone_from(&next);
        self.screen = Self::screen_for_path(&next);
        self.reset_transient();
        Some((next, self.screen.clone()))
    }

    pub(crate) fn go_up(&mut self) -> Option<PathBuf> {
        if self.screen != Screen::Directory {
            return None;
        }
        let parent = self.cwd.parent()?.to_path_buf();
        self.back.push(self.cwd.clone());
        self.forward.clear();
        self.cwd.clone_from(&parent);
        self.reset_transient();
        Some(parent)
    }

    // Which screen a (possibly sentinel) path represents.
    fn screen_for_path(path: &Path) -> Screen {
        let s = path.to_string_lossy();
        if s == services::COMPUTER_VIEW_ROOT {
            Screen::Computer
        } else if s == services::RECENT_VIEW_ROOT {
            Screen::Recent
        } else if s == services::TRASH_VIEW_ROOT {
            Screen::Trash
        } else if s == services::DOWNLOADS_VIEW_ROOT {
            Screen::Downloads
        } else if s == services::NETWORK_VIEW_ROOT || crate::network::is_server_path(path) {
            Screen::Network
        } else {
            Screen::Directory
        }
    }

    // Clear per-directory transient UI state on any navigation.
    //
    // The last-loaded rows stay on screen until `Msg::DirLoaded` replaces them
    // through [`Self::set_entries`], so navigation does not flash the empty
    // page; selection, anchor, search, filters and path editing
    // are cleared so no row of the old directory survives as a selection.
    // `load_error` is left for the load result to replace, so the old view
    // never flashes as though it had loaded.
    fn reset_transient(&mut self) {
        for (path, page) in &mut self.network.servers {
            if *path != self.cwd {
                page.plan.login = None;
            }
        }
        self.selection.clear();
        // The old folder's files must not stay on screen, or act, under the
        // new folder's path while its listing is on its way.
        self.entries.clear();
        self.load_error = None;
        self.listing_pending = true;
        self.listing_slow = false;
        // An in-place rename belongs to the folder it was opened in; one left
        // open with a refused name would otherwise follow, invisible, and
        // keep refusing drags.
        self.inline_rename = None;
        self.inline_rename_blurred = false;
        self.rename_after_load = None;
        self.search_active = false;
        self.search_query.clear();
        // Filters live with the search session: a navigation ends the search,
        // so the facets reset to Any and the Filters dialog closes.
        self.search_filter_type = SearchFilterType::Any;
        self.search_filter_date = SearchFilterDate::Any;
        self.filters_open = false;
        // Navigation ends the recursive session and supersedes every walk
        // dispatched for the previous folder, including a repeated query.
        self.begin_search();
        self.search_scope = SearchScope::Folder;
        self.search_results.clear();
        self.path_edit_active = false;
        // Realised rows derive from these offsets, so a folder opened after a
        // deep scroll would otherwise realise no rows and look empty.
        self.list_scroll_y = 0.0;
        self.grid_scroll_y = 0.0;
        self.directory_view_bounds.set(None);
        self.scroll_reset_owed = true;
        // Every cwd and screen change funnels through here, so the folder's
        // remembered sort is resolved once before its entries load.
        self.apply_resolved_folder_sort();
        // `path_display` must track `cwd`; every back, forward, up and
        // navigation path funnels through here.
        self.refresh_path_display();
    }

    pub(crate) fn is_selected(&self, path: &Path) -> bool {
        self.selection.contains(path)
    }

    fn click_selection(&mut self, path: PathBuf, modifiers: ClickModifiers) {
        if let Some(order) = &self.visible_provider.selection_order {
            self.selection.click(order, &path, modifiers);
        }
    }

    // Plain click: select exactly `path`, resetting the shared anchor.
    pub(crate) fn select_only(&mut self, path: PathBuf) {
        if self.entry_for_path(&path).is_some()
            || self
                .entries
                .iter()
                .chain(&self.search_results)
                .any(|entry| entry.path == path)
        {
            self.selection
                .replace([path.clone()], Some(path.clone()), Some(path));
        }
    }

    // Ctrl/Cmd-click toggles one item without moving the shared range anchor.
    pub(crate) fn toggle_select(&mut self, path: PathBuf) {
        self.click_selection(path, ClickModifiers::TOGGLE);
    }

    // Shift-click extends from the shared range anchor in display order.
    pub(crate) fn select_range_to(&mut self, path: PathBuf) {
        self.click_selection(path, ClickModifiers::RANGE);
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection.clear();
    }

    // Open the Select items matching dialog, seeding `*` when the draft is
    // empty so Enter selects everything.
    pub(crate) fn open_select_matching(&mut self) {
        self.select_matching_open = true;
        if self.select_matching_draft.is_empty() {
            "*".clone_into(&mut self.select_matching_draft);
        }
    }

    // Close the dialog without changing the selection.
    pub(crate) fn close_select_matching(&mut self) {
        self.select_matching_open = false;
    }

    // Replace the selection with every visible entry whose name matches the
    // glob draft, anchor on the first match, and close the dialog.
    //
    // Only `visible_entries` are matched, so hidden or filtered rows are never
    // selected; no match leaves an empty selection and no anchor.
    pub(crate) fn apply_select_matching(&mut self) {
        self.refresh_visible_provider();
        let pat = self.select_matching_draft.clone();
        let matches: Vec<PathBuf> = self
            .projected_entries()
            .into_iter()
            .filter(|e| glob_match(&pat, &e.name))
            .map(|e| e.path.clone())
            .collect();
        self.selection.replace(
            matches.clone(),
            matches.first().cloned(),
            matches.last().cloned(),
        );
        self.select_matching_open = false;
    }

    // The shared host has already applied the press-time selection modifier.
    pub(crate) fn apply_marquee_resolved(&mut self, ids: &[enzui_core::drag::ItemId]) {
        // Ids are reversible byte encodings from `drag_drop::path_to_id`, so
        // non-UTF-8 names survive; a malformed id is skipped.
        self.selection.replace(
            ids.iter().filter_map(crate::drag_drop::id_to_path),
            None,
            None,
        );
        // A marquee gesture supersedes any prior range endpoint.
    }

    // The paths a bulk rename would act on: the selection in display order,
    // so `[N]` numbers files as shown. An empty selection renames nothing.
    pub(crate) fn bulk_rename_targets(&self) -> Vec<PathBuf> {
        if self.screen != Screen::Directory {
            return Vec::new();
        }
        self.projected_entries()
            .into_iter()
            .filter(|e| self.selection.contains(&e.path))
            .map(|e| e.path.clone())
            .collect()
    }

    // Snapshot the selection as top-level `DeleteItem`s for a permanent-delete
    // confirmation.
    //
    // Names and kinds come from the loaded listing without walking any tree,
    // and the snapshot is fixed, so a selection change while the dialog is
    // open cannot change what deletion removes.
    pub(crate) fn delete_items(&self) -> Vec<crate::trash::DeleteItem> {
        if self.screen != Screen::Directory {
            return Vec::new();
        }
        self.projected_entries()
            .into_iter()
            .filter(|e| self.selection.contains(&e.path))
            .map(|e| crate::trash::DeleteItem {
                path: e.path.clone(),
                name: e.name.clone(),
                is_dir: matches!(e.kind, EntryKind::Dir),
            })
            .collect()
    }

    pub(crate) fn close_bulk_rename(&mut self) {
        self.bulk_rename = None;
    }

    // The paths a Cut / Copy would capture: the current selection in display
    // order, same authority as [`Self::bulk_rename_targets`].
    pub(crate) fn clipboard_sources(&self) -> Vec<PathBuf> {
        self.bulk_rename_targets()
    }

    // Whether the current directory can request a desktop clipboard paste.
    // Clipboard content is read afresh when invoked, not cached between apps.
    pub(crate) fn can_paste(&self) -> bool {
        self.screen == Screen::Directory && !self.clipboard_pending
    }

    pub(crate) fn open_properties(&mut self, info: PropertiesInfo) {
        self.properties = Some(info);
    }

    pub(crate) fn close_properties(&mut self) {
        self.properties = None;
    }

    // Whether the open Properties dialog's permission grid differs from the
    // mode last read from disk; `false` with no dialog open.
    //
    // Apply Permissions is enabled only when this is true. A successful Apply
    // re-reads the mode, which resets the baseline.
    pub(crate) fn permissions_dirty(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|info| info.mode_bits() != info.baseline_mode)
    }

    // Flip the permission bit at `(row, col)` of the open Properties dialog,
    // where `row` 0/1/2 = owner/group/other and `col` 0/1/2 = read/write/exec.
    // Returns whether a bit was toggled.
    pub(crate) fn toggle_permission_bit(&mut self, row: usize, col: usize) -> bool {
        let Some(info) = self.properties.as_mut() else {
            return false;
        };
        let Some(cur) = info.get_bit(row, col) else {
            return false;
        };
        info.set_bit(row, col, !cur)
    }

    // Store the recursive-size text on the open Properties dialog, unless it
    // was closed or now shows a different `path`.
    pub(crate) fn set_recursive_size(&mut self, path: &Path, text: String) {
        if let Some(info) = self.properties.as_mut()
            && info.path == path
        {
            info.recursive_size_text = text;
        }
    }
}
