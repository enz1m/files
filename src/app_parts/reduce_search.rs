// Search, selection, and marquee reducer arms.

use super::*;

impl App {
    pub(crate) fn reduce_toggle_search(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleSearch = message else {
            unreachable!("misrouted message in reduce_toggle_search");
        };
        self.model.search_active = !self.model.search_active;
        if !self.model.search_active {
            self.model.search_query.clear();
            // Ending the search ends its filter session, so no stale facet
            // narrows the plain listing.
            self.model.search_filter_type = model::SearchFilterType::Any;
            self.model.search_filter_date = model::SearchFilterDate::Any;
            self.model.filters_open = false;
            self.model.search_scope = model::SearchScope::Folder;
            self.model.search_results.clear();
            self.model.begin_search();
            // Closing edge: do not steal focus onto a field that is no
            // longer rendered.
            Task::none()
        } else {
            self.model.path_edit_active = false;
            self.model.path_suggestions.clear();
            // The mounted search field owns its initial focus.
            Task::none()
        }
    }

    pub(crate) fn reduce_search_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SearchChanged(q) = message else {
            unreachable!("misrouted message in reduce_search_changed");
        };
        self.model.search_query = q;
        // Folder scope filters loaded entries and needs no task.
        if self.model.search_scope == model::SearchScope::Everywhere {
            Task::batch([self.recursive_search_task(), self.main_window_scope()])
        } else {
            self.model.begin_search();
            self.main_window_scope()
        }
    }

    pub(crate) fn reduce_set_search_scope(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetSearchScope(recursive) = message else {
            unreachable!("misrouted message in reduce_set_search_scope");
        };
        self.model.search_scope = if recursive {
            model::SearchScope::Everywhere
        } else {
            model::SearchScope::Folder
        };
        if recursive {
            self.recursive_search_task()
        } else {
            // Back to folder scope: drop the stale recursive hits so
            // `visible_entries` reads the current folder again.
            self.model.begin_search();
            self.model.search_results.clear();
            Task::none()
        }
    }

    pub(crate) fn reduce_search_results_ready(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SearchResultsReady {
            generation,
            root,
            query,
            hits,
        } = message
        else {
            unreachable!("misrouted message in reduce_search_results_ready");
        };
        if self.model.search_active
            && self.model.search_scope == model::SearchScope::Everywhere
            && generation == self.model.search_generation
        {
            debug_assert_eq!(
                self.model.cwd, root,
                "current search generation must retain its root"
            );
            debug_assert_eq!(
                self.model.search_query, query,
                "current search generation must retain its query"
            );
            self.model.search_results = hits;
        }
        Task::none()
    }

    pub(crate) fn reduce_toggle_filters(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleFilters = message else {
            unreachable!("misrouted message in reduce_toggle_filters");
        };
        self.model.filters_open = !self.model.filters_open;
        Task::none()
    }

    pub(crate) fn reduce_search_filters_open_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SearchFiltersOpenChanged(open) = message else {
            unreachable!("misrouted message in reduce_search_filters_open_changed");
        };
        self.model.filters_open = open;
        Task::none()
    }

    pub(crate) fn reduce_set_filter_type(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetFilterType(facet) = message else {
            unreachable!("misrouted message in reduce_set_filter_type");
        };
        self.model.search_filter_type = facet;
        Task::none()
    }

    pub(crate) fn reduce_set_filter_date(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SetFilterDate(facet) = message else {
            unreachable!("misrouted message in reduce_set_filter_date");
        };
        self.model.search_filter_date = facet;
        Task::none()
    }

    pub(crate) fn reduce_begin_path_edit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginPathEdit = message else {
            unreachable!("misrouted message in reduce_begin_path_edit");
        };
        let end_search = if self.model.search_active {
            self.reduce_toggle_search(Msg::ToggleSearch)
        } else {
            Task::none()
        };
        self.model.path_edit_active = true;
        self.model.path_suggestions.clear();
        self.model.path_edit_draft = self.model.cwd.to_string_lossy().into_owned();
        Task::batch([
            end_search,
            breadcrumb_focus_task(&BreadcrumbEditEvent::Begin),
        ])
    }

    pub(crate) fn reduce_path_draft_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::PathDraftChanged(d) = message else {
            unreachable!("misrouted message in reduce_path_draft_changed");
        };
        self.model.path_edit_draft.clone_from(&d);
        self.model.path_suggestions.clear();
        let home = self.model.home.clone();
        Task::perform(
            crate::shell_work::run(move || {
                let paths = services::path_suggestions(&d, &home);
                (d, paths)
            }),
            |(query, paths)| Msg::PathSuggestionsReady { query, paths },
        )
    }

    pub(crate) fn reduce_path_suggestion(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::PathSuggestionsReady { query, paths } => {
                if self.model.path_edit_active && self.model.path_edit_draft == query {
                    self.model.path_suggestions = paths;
                }
                Task::none()
            }
            Msg::PathSuggestionAccepted(path) => {
                self.model.path_edit_draft = path.to_string_lossy().into_owned();
                self.reduce_commit_path_edit(Msg::CommitPathEdit)
            }
            _ => unreachable!("misrouted path suggestion"),
        }
    }

    pub(crate) fn reduce_commit_path_edit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CommitPathEdit = message else {
            unreachable!("misrouted message in reduce_commit_path_edit");
        };
        let target = services::expand_user_path(&self.model.path_edit_draft, &self.model.home);
        self.model.path_edit_active = false;
        // Checked on a worker: a typed path on a hung mount must not freeze
        // the window.
        let check = Task::perform(
            crate::shell_work::run(move || {
                let is_dir = target.is_dir();
                (target, is_dir)
            }),
            |(path, is_dir)| Msg::TypedPathChecked { path, is_dir },
        );
        Task::batch([check, breadcrumb_focus_task(&BreadcrumbEditEvent::Commit)])
    }

    pub(crate) fn reduce_typed_path_checked(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TypedPathChecked { path, is_dir } = message else {
            unreachable!("misrouted message in reduce_typed_path_checked");
        };
        if !is_dir {
            return Task::none();
        }
        // The regular navigation, which also moves the folder watcher.
        self.nav_navigate_to(Msg::NavigateTo(path))
    }

    pub(crate) fn reduce_cancel_path_edit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelPathEdit = message else {
            unreachable!("misrouted message in reduce_cancel_path_edit");
        };
        self.model.path_edit_active = false;
        breadcrumb_focus_task(&BreadcrumbEditEvent::Cancel)
    }

    pub(crate) fn reduce_dismiss_path_edit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::DismissPathEdit = message else {
            unreachable!("misrouted message in reduce_dismiss_path_edit");
        };
        // Outside presses and focus transfers already chose their next
        // target. Close the editor without restoring breadcrumb focus.
        self.model.path_edit_active = false;
        Task::none()
    }

    pub(crate) fn reduce_copy_cwd_path(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CopyCwdPath = message else {
            unreachable!("misrouted message in reduce_copy_cwd_path");
        };
        let text = self.model.cwd.to_string_lossy().into_owned();
        iced::clipboard::write(text).discard()
    }

    pub(crate) fn reduce_copy_row_path(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CopyRowPath(path) = message else {
            unreachable!("misrouted message in reduce_copy_row_path");
        };
        let text = path.to_string_lossy().into_owned();
        iced::clipboard::write(text).discard()
    }

    pub(crate) fn reduce_row_click_primary(&mut self, message: Msg) -> Task<Msg> {
        let Msg::RowClickPrimary {
            path,
            kind,
            command,
            shift,
        } = message
        else {
            unreachable!("misrouted message in reduce_row_click_primary");
        };
        // A modifier click never activates and never latches an
        // activation seed: it edits the selection instead.
        if command {
            self.model.last_row_press = None;
            return self.update(Msg::ToggleSelect(path));
        }
        if shift {
            self.model.last_row_press = None;
            return self.update(Msg::SelectRange(path));
        }
        // Single-click mode already opened on the first click, so a second
        // click must not open again.
        if self.model.single_click_open {
            self.model.last_row_press = None;
            return self.update(Msg::OpenEntry(path, kind));
        }
        // Two clicks on the same path within the framework's multi-click
        // interval are a double-click. Row identity replaces `ClickDetector`'s
        // position deadband because this message carries no position; Enter
        // and Space never seed the latch. `<=` matches
        // `ClickDetector::is_consecutive`.
        if let Some((last_path, last_at)) = &self.model.last_row_press
            && last_path == &path
            && last_at.elapsed() <= enzui_core::click::DEFAULT_MULTI_CLICK_INTERVAL
        {
            self.model.last_row_press = None;
            return self.update(Msg::OpenEntry(path, kind));
        }
        self.model.last_row_press = Some((path.clone(), std::time::Instant::now()));
        self.model.select_only(path);
        Task::none()
    }

    pub(crate) fn reduce_select_only(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectOnly(p) = message else {
            unreachable!("misrouted message in reduce_select_only");
        };
        self.model.select_only(p);
        Task::none()
    }

    pub(crate) fn reduce_toggle_select(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ToggleSelect(p) = message else {
            unreachable!("misrouted message in reduce_toggle_select");
        };
        self.model.toggle_select(p);
        Task::none()
    }

    pub(crate) fn reduce_select_range(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectRange(p) = message else {
            unreachable!("misrouted message in reduce_select_range");
        };
        self.model.select_range_to(p);
        Task::none()
    }

    pub(crate) fn reduce_select_all(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectAll = message else {
            unreachable!("misrouted message in reduce_select_all");
        };
        let paths: Vec<_> = self
            .model
            .projected_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        for p in paths {
            self.model.selection.insert(p);
        }
        Task::none()
    }

    pub(crate) fn reduce_clear_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ClearSelection = message else {
            unreachable!("misrouted message in reduce_clear_selection");
        };
        self.model.clear_selection();
        Task::none()
    }

    pub(crate) fn reduce_move_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MoveSelection { direction, extend } = message else {
            unreachable!("misrouted message in reduce_move_selection");
        };
        // Arrow keys under a dialog belong to the dialog, never the list.
        if self.model.screen != Screen::Directory || self.modal_owns_keyboard() {
            return Task::none();
        }
        let columns = self.model.grid_layout.get().columns.max(1);
        let Some(index) = self.model.move_selection(direction, columns, extend) else {
            return Task::none();
        };
        self.reveal_active_row(index)
    }

    // Scroll the visible row at `index` into view and move widget focus onto
    // the selection's active row, so the focus ring and the selection name
    // the same row. Shared by arrow keys and type-ahead.
    pub(crate) fn reveal_active_row(&mut self, index: usize) -> Task<Msg> {
        let columns = self.model.grid_layout.get().columns.max(1);
        // Focus follows the active endpoint so Space and Enter act on the
        // selected row. The id derives from the full path, as the rows' ids
        // do, because recursive search can show two rows with one basename.
        let focus_target = self
            .model
            .selection
            .active()
            .map(|path| match self.model.view_mode {
                model::ViewMode::List => context_menu::list_row_id(path),
                model::ViewMode::Grid => context_menu::grid_tile_id(path),
            });
        let fallback = self.window_height
            - enzui_widgets::title_bar::DEFAULT_TITLEBAR_HEIGHT
            - if self.model.tabs().len() > 1 {
                enzui_widgets::title_bar::DEFAULT_TITLEBAR_HEIGHT
            } else {
                0.0
            }
            - if self.model.view_mode == model::ViewMode::List {
                view::list::ROW_HEIGHT
            } else {
                0.0
            };
        let viewport = self
            .model
            .directory_view_bounds
            .get()
            .map_or(fallback, |bounds| bounds.height)
            .max(1.0);
        match self.model.view_mode {
            model::ViewMode::List => {
                let shell_width =
                    view::shell_geometry(&self.model, self.themes.theme(), self.window_width)
                        .content_width;
                let content_width = view::shell::directory_view_width(&self.model, shell_width);
                let row_height =
                    view::list::row_height_for(&self.model, self.themes.theme(), content_width);
                let top = index as f32 * row_height;
                let bottom = top + row_height;
                if top < self.model.list_scroll_y {
                    self.model.list_scroll_y = top;
                } else if bottom > self.model.list_scroll_y + viewport {
                    self.model.list_scroll_y = (bottom - viewport).max(0.0);
                }
            }
            model::ViewMode::Grid => {
                let height = view::grid::grid_tile_height(&self.model, self.themes.theme());
                let row = index / columns.max(1);
                let top = view::widgets::GRID_GAP + row as f32 * (height + view::widgets::GRID_GAP);
                let bottom = top + height;
                if top < self.model.grid_scroll_y {
                    self.model.grid_scroll_y = top;
                } else if bottom > self.model.grid_scroll_y + viewport {
                    self.model.grid_scroll_y = (bottom - viewport).max(0.0);
                }
            }
        }
        let (scroller, offset) = match self.model.view_mode {
            model::ViewMode::List => (view::list::LIST_SCROLLER_ID, self.model.list_scroll_y),
            model::ViewMode::Grid => (view::grid::GRID_SCROLLER_ID, self.model.grid_scroll_y),
        };
        Task::batch([
            iced::widget::operation::scroll_to(
                iced::widget::Id::new(scroller),
                iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
                iced::advanced::widget::operation::Animation::Instant,
            ),
            focus_target.map_or_else(Task::none, iced::widget::operation::focus),
        ])
    }

    pub(crate) fn reduce_type_ahead(&mut self, message: Msg) -> Task<Msg> {
        let Msg::TypeAhead(ch) = message else {
            unreachable!("misrouted message in reduce_type_ahead");
        };
        // Keys that reach the window under a dialog (its field lost focus)
        // never start a search behind it.
        if self.model.screen != Screen::Directory || self.modal_owns_keyboard() {
            return Task::none();
        }
        // Typing in a folder filters it, as a search of this folder: the
        // first character opens the search with itself as the query, and the
        // field, focused as it mounts, takes the rest.
        self.model.search_active = true;
        let mut query = self.model.search_query.clone();
        query.push(ch);
        self.update(Msg::SearchChanged(query))
    }

    pub(crate) fn reduce_open_select_matching(&mut self, message: Msg) -> Task<Msg> {
        let Msg::OpenSelectMatching = message else {
            unreachable!("misrouted message in reduce_open_select_matching");
        };
        self.model.open_select_matching();
        Task::none()
    }

    pub(crate) fn reduce_close_select_matching(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CloseSelectMatching = message else {
            unreachable!("misrouted message in reduce_close_select_matching");
        };
        self.model.close_select_matching();
        Task::none()
    }

    pub(crate) fn reduce_select_matching_draft_changed(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectMatchingDraftChanged(v) = message else {
            unreachable!("misrouted message in reduce_select_matching_draft_changed");
        };
        self.model.select_matching_draft = v;
        Task::none()
    }

    pub(crate) fn reduce_select_matching_submit(&mut self, message: Msg) -> Task<Msg> {
        let Msg::SelectMatchingSubmit = message else {
            unreachable!("misrouted message in reduce_select_matching_submit");
        };
        self.model.apply_select_matching();
        Task::none()
    }

    pub(crate) fn reduce_marquee_resolved(&mut self, message: Msg) -> Task<Msg> {
        let (Msg::MarqueeResolved(ids) | Msg::MarqueePreview(ids)) = message else {
            unreachable!("misrouted message in reduce_marquee_resolved");
        };
        self.model.apply_marquee_resolved(&ids);
        Task::none()
    }
}
