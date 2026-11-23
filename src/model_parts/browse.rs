// Model construction, listing, view settings, and sorting.

use super::*;

impl Model {
    pub(crate) fn new(home: PathBuf) -> Self {
        let places = services::default_places(&home);
        let mut model = Self {
            cwd: home.clone(),
            home,
            screen: Screen::Directory,
            entries: Vec::new(),
            load_error: None,
            load_generation: 0,
            back: Vec::new(),
            forward: Vec::new(),
            places,
            bookmarks: Vec::new(),
            pin_retained_rows: 0,
            pin_retained_bytes: 0,
            pins_refused: None,
            pin_durability_debt: false,
            drives: Vec::new(),
            storage_locations: Vec::new(),
            storage_error: None,
            storage_ready: false,
            storage_capture: false,
            storage_busy: Default::default(),
            trash_entries: Vec::new(),
            trash_payloads: std::collections::HashMap::new(),
            folder_sizes: std::collections::HashMap::new(),
            trash_listing_incomplete: None,
            view_mode: ViewMode::List,
            default_view_mode: ViewMode::List,
            sort_key: SortKey::Name,
            sort_ascending: true,
            sort_folders_first: true,
            default_sort_key: SortKey::Name,
            default_sort_ascending: true,
            sort_overrides: enzui_core::persist::Tracked::new(std::collections::BTreeMap::new()),
            show_hidden: false,
            colored_file_icons: true,
            list_zoom: list_zoom_ladder().default_index(),
            grid_zoom: grid_zoom_ladder().default_index(),
            terminal_command: String::new(),
            templates: crate::templates::Templates::default(),
            folder_colors: enzui_core::persist::Tracked::new(Default::default()),
            folder_identities: std::collections::HashMap::new(),
            folder_color_dialog: None,
            history: None,
            history_available: false,
            history_excluded: Vec::new(),
            history_fixture: false,
            tabs: crate::tabs::Tabs::default(),
            tab_list_open: false,
            visible_columns: [true, true, true],
            grid_info: [false, false, false],
            mime_defaults: std::collections::HashMap::new(),
            selection: ListSelection::default(),
            search_active: false,
            search_query: String::new(),
            search_scope: SearchScope::Folder,
            search_results: Vec::new(),
            search_generation: 0,
            search_filter_type: SearchFilterType::Any,
            search_filter_date: SearchFilterDate::Any,
            filters_open: false,
            path_edit_active: false,
            path_edit_draft: String::new(),
            path_suggestions: Vec::new(),
            // Seeded below by `refresh_path_display`.
            path_display: String::new(),
            details_open: false,
            properties: None,
            inspector_properties: None,
            inspector_pending: None,
            properties_perms_expanded: false,
            settings_menu: None,
            toolbar_overflow_open: false,
            view_settings_open: false,
            settings_page: SettingsSection::default(),
            settings_page_open: false,
            downloads_settings_open: false,
            shortcut_capture: None,
            shortcut_feedback: None,
            shortcuts_notice: None,
            nav: NavSplitState::new(),
            bulk_rename: None,
            inline_rename: None,
            inline_rename_blurred: false,
            rename_after_load: None,
            removable_devices: Vec::new(),
            phones: Vec::new(),
            pending_device_confirm: None,
            mount_question: None,
            unlock: None,
            location_dialog: None,
            format: None,
            cloud: None,
            cloud_providers: Vec::new(),
            focus_after_load: None,
            conflicts: None,
            pending_copy_folders: Vec::new(),
            clipboard_pending: false,
            context_menu: None,
            journal: crate::journal::OpJournal::new(),
            journal_prompts: Vec::new(),
            conflicts_busy: false,
            bulk_rename_busy: false,
            status_message: None,
            open_with: None,
            attach: None,
            single_click_open: false,
            select_matching_open: false,
            shortcuts_open: false,
            all_activity_open: false,
            pending_permanent_delete: None,
            pending_trash_restore: None,
            delete_confirm_state: enzui_core::confirm::ConfirmState::new(None),
            select_matching_draft: String::new(),
            downloads: crate::downloads::DownloadsState::default(),
            features: crate::features::Features::default(),
            extensions: crate::extensions::Catalog::default(),
            features_refusal: None,
            features_durability_debt: false,
            network: crate::network::NetworkState::default(),
            thumbnails: std::collections::HashMap::new(),
            visible_provider: FileEntryProvider::default(),
            list_scroll_y: 0.0,
            grid_scroll_y: 0.0,
            scroll_reset_owed: false,
            listing_pending: false,
            listing_slow: false,
            downloads_scroll_y: 0.0,
            directory_view_bounds: std::cell::Cell::new(None),
            grid_layout: std::cell::Cell::new(enzui_core::virtual_grid::ResolvedTileGrid::default()),
            breadcrumb_edit_bounds: std::cell::Cell::new(None),
            pin_drop_hover: None,
            last_row_press: None,
            device_row_selected: None,
            last_device_press: None,
            project: crate::project::Project::default(),
        };
        // Every arm that changes `cwd`, `home` or `screen` must refresh this too.
        model.refresh_path_display();
        model
    }

    // Recomputes [`Self::path_display`] from `cwd`, `home` and `screen`, within
    // [`PATH_DISPLAY_BUDGET_CHARS`].
    pub(crate) fn refresh_path_display(&mut self) {
        self.path_display =
            collapse_display_path(&self.full_display_path(), PATH_DISPLAY_BUDGET_CHARS);
    }

    // The uncollapsed location string, such as `"Home/Documents"`, `"/etc"` or
    // `"Computer"`.
    //
    // Copy path reads `cwd` instead, so a collapsed `…/` never reaches the clipboard.
    pub(crate) fn full_display_path(&self) -> String {
        let segments = self.breadcrumb_segments();
        if segments.is_empty() {
            return "/".to_owned();
        }
        let labels: Vec<String> = segments.into_iter().map(|(l, _)| l).collect();
        labels.join("/").replace("//", "/")
    }

    // Allocates and returns the next directory-read generation.
    //
    // `DirLoaded` applies a reply only while its stamp equals `load_generation`, so
    // a superseded load cannot overwrite the current folder.
    pub(crate) fn begin_load(&mut self) -> u64 {
        self.load_generation = self.load_generation.wrapping_add(1);
        self.load_generation
    }

    // Allocates the next recursive-search request or invalidation generation.
    pub(crate) fn begin_search(&mut self) -> u64 {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_generation
    }

    // Replaces the directory inventory, removes missing selections, and refreshes its projection.
    pub(crate) fn set_entries(&mut self, entries: Vec<FileEntry>) {
        self.entries = entries;
        self.load_error = None;
        self.prune_selection();
        self.refresh_inspector_after_listing();
        self.refresh_visible_provider();
    }

    // After a new listing the details pane asks again for fresh details, but
    // keeps showing the current ones meanwhile, so a folder that reloads
    // often (a download writing into it) never blanks the pane.
    fn refresh_inspector_after_listing(&mut self) {
        self.inspector_pending = None;
        if self
            .inspector_properties
            .as_ref()
            .is_some_and(|shown| !self.selection.contains(&shown.path))
        {
            self.inspector_properties = None;
        }
    }

    // Resolves a provider item back to the model-owned entry.
    pub(crate) fn projected_entry(&self, row: &ProjectedEntry) -> Option<&FileEntry> {
        match row.source {
            EntrySource::Directory => self.entries.get(row.index),
            EntrySource::Search => self.search_results.get(row.index),
        }
    }

    pub(crate) fn entry_for_path(&self, path: &Path) -> Option<&FileEntry> {
        let key = SequenceKey::new(crate::drag_drop::path_to_id(path).as_str());
        let index = self.visible_provider.index_of(&key)?;
        let row = self.visible_provider.rows.get(index as usize)?;
        self.projected_entry(row).filter(|entry| entry.path == path)
    }

    pub(crate) fn projected_entries(&self) -> Vec<&FileEntry> {
        self.visible_provider
            .rows
            .iter()
            .filter_map(|row| self.projected_entry(row))
            .collect()
    }

    // Rebuilds the filtered, sorted keyed index after a data or policy change.
    //
    // View construction reads only the realised window.
    pub(crate) fn refresh_visible_provider(&mut self) {
        let policy = ProjectionPolicy::from_model(self);
        let recursive = policy.recursive();
        let source_kind = if recursive {
            EntrySource::Search
        } else {
            EntrySource::Directory
        };
        let source = if recursive {
            &self.search_results
        } else {
            &self.entries
        };
        self.visible_provider =
            FileEntryProvider::build(source, source_kind, &policy, &self.visible_provider);
    }

    pub(crate) fn set_prepared_entries(
        &mut self,
        entries: Vec<FileEntry>,
        provider: FileEntryProvider,
    ) {
        self.entries = entries;
        self.visible_provider = provider;
        self.load_error = None;
        self.prune_selection();
        self.refresh_inspector_after_listing();
    }

    // Drops selected paths no longer present in `entries`.
    fn prune_selection(&mut self) {
        if self.selection.is_empty()
            && self.selection.anchor().is_none()
            && self.selection.active().is_none()
        {
            return;
        }
        let paths: std::collections::HashSet<_> =
            self.entries.iter().map(|entry| &entry.path).collect();
        let selected: Vec<_> = self
            .selection
            .iter()
            .filter(|path| paths.contains(path))
            .cloned()
            .collect();
        let anchor = self
            .selection
            .anchor()
            .filter(|path| paths.contains(path))
            .cloned();
        let active = self
            .selection
            .active()
            .filter(|path| paths.contains(path))
            .cloned();
        self.selection.replace(selected, anchor, active);
    }

    pub(crate) fn column_visible(&self, column: MetaColumn) -> bool {
        self.visible_columns[column.index()]
    }

    pub(crate) fn grid_info_visible(&self, column: MetaColumn) -> bool {
        self.grid_info[column.index()]
    }

    // Moves the keyboard cursor in display order and returns the destination index.
    //
    // Grid vertical moves advance by `columns`. Shift extends from the anchor; a
    // plain arrow selects only the destination.
    pub(crate) fn move_selection(
        &mut self,
        direction: SelectionDirection,
        columns: usize,
        extend: bool,
    ) -> Option<usize> {
        let click_model = self.visible_provider.selection_order.as_ref()?;
        let order = click_model.order();
        if order.is_empty() {
            return None;
        }
        let index_of = |path: &PathBuf| click_model.index_of(path);
        let current = self
            .selection
            .active()
            .and_then(index_of)
            .or_else(|| self.selection.iter().filter_map(index_of).min());
        let stride = match (self.view_mode, direction) {
            (ViewMode::Grid, SelectionDirection::Up | SelectionDirection::Down) => {
                columns.max(1) as isize
            }
            _ => 1,
        };
        let delta = match direction {
            SelectionDirection::Left | SelectionDirection::Up => -stride,
            SelectionDirection::Right | SelectionDirection::Down => stride,
        };
        let start = current.unwrap_or_else(|| if delta < 0 { order.len() - 1 } else { 0 });
        let destination = if current.is_none() {
            start
        } else {
            (start as isize + delta).clamp(0, order.len() as isize - 1) as usize
        };
        let path = order[destination].clone();
        if extend {
            self.selection
                .click(click_model, &path, ClickModifiers::RANGE);
        } else {
            self.selection
                .replace([path.clone()], Some(path.clone()), Some(path));
        }
        Some(destination)
    }

    // The colour tag of the listed folder at `path`, once its identity is known.
    pub(crate) fn folder_color(&self, path: &Path) -> Option<enzui_theme::WorkspaceAccentKey> {
        let identity = self.folder_identities.get(path)?;
        self.folder_colors.get().color_for(path, identity)
    }

    pub(crate) fn list_zoom_factor(&self) -> f32 {
        list_zoom_ladder().factor(self.list_zoom)
    }

    pub(crate) fn grid_zoom_factor(&self) -> f32 {
        grid_zoom_ladder().factor(self.grid_zoom)
    }

    // Steps the current view's zoom level, returning whether it changed.
    pub(crate) fn zoom(&mut self, step: enzui_core::zoom_levels::ZoomStep) -> bool {
        let (ladder, level) = match self.view_mode {
            ViewMode::List => (list_zoom_ladder(), &mut self.list_zoom),
            ViewMode::Grid => (grid_zoom_ladder(), &mut self.grid_zoom),
        };
        let next = ladder.apply(*level, step);
        let moved = next != *level;
        *level = next;
        moved
    }

    // Applies a persisted view-settings record at boot.
    //
    // Call after `cwd` is set: the current folder's sort is resolved before the
    // first directory read lands.
    pub(crate) fn apply_view_settings(
        &mut self,
        settings: &crate::view_settings_store::ViewSettings,
    ) {
        self.view_mode = settings.view_mode;
        self.default_view_mode = settings.view_mode;
        self.default_sort_key = settings.default_sort_key;
        self.default_sort_ascending = settings.default_sort_ascending;
        self.sort_folders_first = settings.sort_folders_first;
        self.show_hidden = settings.show_hidden;
        self.colored_file_icons = settings.colored_file_icons;
        self.list_zoom = list_zoom_ladder().clamp(settings.list_zoom);
        self.grid_zoom = grid_zoom_ladder().clamp(settings.grid_zoom);
        self.terminal_command.clone_from(&settings.terminal_command);
        self.folder_colors.set(settings.folder_colors.clone());
        self.single_click_open = settings.single_click_open;
        self.visible_columns = settings.visible_columns;
        self.grid_info = settings.grid_info;
        self.sort_overrides.set(settings.sort_overrides.clone());
        self.apply_resolved_folder_sort();
    }

    // Column-header sort: the active key flips direction, a new key sorts ascending.
    //
    // View Settings radios use [`Model::set_sort_key`] and
    // [`Model::set_sort_ascending`], which never flip.
    pub(crate) fn sort_by(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_key = key;
            self.sort_ascending = true;
        }
        // Remembered for this folder, not promoted to the settings default.
        self.remember_folder_sort();
        self.refresh_visible_provider();
    }

    // Sets the default sort key without flipping direction.
    //
    // Writes only [`Self::default_sort_key`], so the open folder is not pinned; it
    // then re-resolves, keeping a manual override if it has one.
    pub(crate) fn set_sort_key(&mut self, key: SortKey) {
        self.default_sort_key = key;
        self.apply_resolved_folder_sort();
        self.refresh_visible_provider();
    }

    // Sets the default sort direction; like [`Self::set_sort_key`], it creates no
    // per-folder override.
    pub(crate) fn set_sort_ascending(&mut self, ascending: bool) {
        self.default_sort_ascending = ascending;
        self.apply_resolved_folder_sort();
        self.refresh_visible_provider();
    }

    // Records the current sort as this directory's override; no-op off a
    // `Directory` screen.
    fn remember_folder_sort(&mut self) {
        if self.screen == Screen::Directory {
            self.sort_overrides
                .get_mut()
                .insert(self.cwd.clone(), (self.sort_key, self.sort_ascending));
        }
    }

    // Resolves the live `sort_key` and `sort_ascending` for this screen.
    //
    // A `Directory` uses its override or the default; virtual screens use the
    // default.
    pub(crate) fn apply_resolved_folder_sort(&mut self) {
        let (key, ascending) = if self.screen == Screen::Directory {
            self.sort_overrides
                .get()
                .get(&self.cwd)
                .copied()
                .unwrap_or((self.default_sort_key, self.default_sort_ascending))
        } else {
            (self.default_sort_key, self.default_sort_ascending)
        };
        self.sort_key = key;
        self.sort_ascending = ascending;
    }

    // The search summary `"N matches for \"query\""`;
    // `None` when search is inactive or the query is empty.
    //
    // Not yet read by any view.

    // Breadcrumb segments for the current directory, root first; virtual screens
    // get one synthetic crumb.
    pub(crate) fn breadcrumb_segments(&self) -> Vec<(String, PathBuf)> {
        match self.screen {
            Screen::Computer => {
                return vec![(crate::i18n::tr("Computer").into(), self.cwd.clone())];
            }
            Screen::Recent => return vec![(crate::i18n::tr("Recent").into(), self.cwd.clone())],
            Screen::Trash => return vec![(crate::i18n::tr("Trash").into(), self.cwd.clone())],
            Screen::Downloads => {
                // Not "Downloads": that is the folder in the home directory.
                return vec![(crate::i18n::tr("Download Manager").into(), self.cwd.clone())];
            }
            Screen::Network => {
                let mut crumbs = vec![(
                    crate::i18n::tr("Network").into(),
                    PathBuf::from(services::NETWORK_VIEW_ROOT),
                )];
                if let Some(server) = self.network.servers.get(&self.cwd) {
                    crumbs.push((server.name.clone(), self.cwd.clone()));
                }
                return crumbs;
            }
            Screen::Directory => {}
        }
        let mut out = Vec::new();
        // The home prefix folds to one "Home" crumb, which also keeps sandboxed
        // captures byte-identical. A `/` fallback home would swallow every path, so
        // only a real home folds.
        let home_is_real = !self.home.as_os_str().is_empty() && self.home != Path::new("/");
        let below_home: Option<&Path> = if home_is_real {
            self.cwd.strip_prefix(&self.home).ok()
        } else {
            None
        };
        // A mounted drive or share folds the same way, to its name, as GNOME
        // Files and Finder show it; the innermost mount wins.
        let mounted = self
            .storage_locations
            .iter()
            .filter_map(|location| Some((location.mount.as_deref()?, location)))
            .filter(|(root, _)| {
                root.parent().is_some()
                    && self.cwd.starts_with(root)
                    && !self.home.starts_with(root)
            })
            .max_by_key(|(root, _)| root.components().count());
        let (mut acc, tail) = if let Some(rest) = below_home {
            out.push((crate::i18n::tr("Home").to_owned(), self.home.clone()));
            (self.home.clone(), Some(rest))
        } else if let Some((root, location)) = mounted {
            out.push((location.name.clone(), root.to_path_buf()));
            (root.to_path_buf(), self.cwd.strip_prefix(root).ok())
        } else {
            (PathBuf::new(), None)
        };
        // Below a folded home, walk only the components under it.
        let walk: PathBuf = match tail {
            Some(rest) => rest.to_path_buf(),
            None => self.cwd.clone(),
        };
        for comp in walk.components() {
            use std::path::Component;
            match comp {
                Component::RootDir => {
                    acc.push("/");
                    out.push(("/".to_owned(), acc.clone()));
                }
                Component::Normal(seg) => {
                    acc.push(seg);
                    out.push((seg.to_string_lossy().into_owned(), acc.clone()));
                }
                _ => {}
            }
        }
        if out.is_empty() {
            out.push(("/".to_owned(), PathBuf::from("/")));
        }
        out
    }
}
