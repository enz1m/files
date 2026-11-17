// Browser tabs.
//
// The live tab's state is the ordinary [`Model`] fields; each inactive tab holds
// a [`TabSnapshot`] of the per-tab fields, swapped in on switch. File operations
// are app-wide, so closing a tab never orphans one; a completion reloads only the
// folder on screen. Per-view transients (context menu, inline rename, marquee,
// type-ahead) are dropped on switch. Locations persist with the view settings.

use std::path::PathBuf;

use enzui_core::list_selection::ListSelection;

use crate::model::{
    FileEntry, Model, Screen, SearchFilterDate, SearchFilterType, SearchScope, ViewMode,
};

pub(crate) const MAX_TABS: usize = 16;

// An inactive tab's browsing state.
#[derive(Debug, Clone)]
pub(crate) struct TabSnapshot {
    pub(crate) cwd: PathBuf,
    screen: Screen,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    selection: ListSelection<PathBuf>,
    view_mode: ViewMode,
    list_scroll_y: f32,
    grid_scroll_y: f32,
    entries: Vec<FileEntry>,
    load_error: Option<String>,
    search_active: bool,
    search_query: String,
    search_scope: SearchScope,
    search_results: Vec<FileEntry>,
    search_filter_type: SearchFilterType,
    search_filter_date: SearchFilterDate,
}

impl TabSnapshot {
    // A fresh tab at `cwd`.
    pub(crate) fn at(cwd: PathBuf, view_mode: ViewMode) -> Self {
        Self {
            cwd,
            screen: Screen::Directory,
            back: Vec::new(),
            forward: Vec::new(),
            selection: ListSelection::default(),
            view_mode,
            list_scroll_y: 0.0,
            grid_scroll_y: 0.0,
            entries: Vec::new(),
            load_error: None,
            search_active: false,
            search_query: String::new(),
            search_scope: SearchScope::Folder,
            search_results: Vec::new(),
            search_filter_type: SearchFilterType::Any,
            search_filter_date: SearchFilterDate::Any,
        }
    }
}

// One tab: the live one carries no snapshot.
#[derive(Debug, Clone)]
pub(crate) struct BrowserTab {
    pub(crate) id: u64,
    pub(crate) snapshot: Option<TabSnapshot>,
}

// Every tab of the window, in strip order.
#[derive(Debug, Clone)]
pub(crate) struct Tabs {
    pub(crate) tabs: Vec<BrowserTab>,
    pub(crate) active: usize,
    next_id: u64,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            tabs: vec![BrowserTab {
                id: 0,
                snapshot: None,
            }],
            active: 0,
            next_id: 1,
        }
    }
}

impl Tabs {
    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn mint(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Model {
    // Move the live tab's state out, leaving a fresh tab at `cwd` behind.
    fn take_live(&mut self, next: TabSnapshot) -> TabSnapshot {
        let live = TabSnapshot {
            cwd: std::mem::replace(&mut self.cwd, next.cwd),
            screen: std::mem::replace(&mut self.screen, next.screen),
            back: std::mem::replace(&mut self.back, next.back),
            forward: std::mem::replace(&mut self.forward, next.forward),
            selection: std::mem::replace(&mut self.selection, next.selection),
            view_mode: std::mem::replace(&mut self.view_mode, next.view_mode),
            list_scroll_y: std::mem::replace(&mut self.list_scroll_y, next.list_scroll_y),
            grid_scroll_y: std::mem::replace(&mut self.grid_scroll_y, next.grid_scroll_y),
            entries: std::mem::replace(&mut self.entries, next.entries),
            load_error: std::mem::replace(&mut self.load_error, next.load_error),
            search_active: std::mem::replace(&mut self.search_active, next.search_active),
            search_query: std::mem::replace(&mut self.search_query, next.search_query),
            search_scope: std::mem::replace(&mut self.search_scope, next.search_scope),
            search_results: std::mem::replace(&mut self.search_results, next.search_results),
            search_filter_type: std::mem::replace(
                &mut self.search_filter_type,
                next.search_filter_type,
            ),
            search_filter_date: std::mem::replace(
                &mut self.search_filter_date,
                next.search_filter_date,
            ),
        };
        // Per-view transients belong to the folder that was on screen.
        self.context_menu = None;
        self.inline_rename = None;
        self.inline_rename_blurred = false;
        self.focus_after_load = None;
        self.rename_after_load = None;
        self.path_edit_active = false;
        // Bumping the generation drops the left tab's late search results.
        self.search_generation = self.search_generation.wrapping_add(1);
        self.refresh_path_display();
        self.apply_resolved_folder_sort();
        self.refresh_visible_provider();
        live
    }

    pub(crate) fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    // The location of the tab at `index`.
    pub(crate) fn tab_location(&self, index: usize) -> Option<&std::path::Path> {
        let tab = self.tabs.tabs.get(index)?;
        Some(match &tab.snapshot {
            Some(snapshot) => snapshot.cwd.as_path(),
            None => self.cwd.as_path(),
        })
    }

    // The strip title of the tab at `index`: its folder's name, or the
    // screen it shows.
    pub(crate) fn tab_title(&self, index: usize) -> String {
        let Some(tab) = self.tabs.tabs.get(index) else {
            return String::new();
        };
        let (screen, cwd) = match &tab.snapshot {
            Some(snapshot) => (&snapshot.screen, snapshot.cwd.as_path()),
            None => (&self.screen, self.cwd.as_path()),
        };
        match screen {
            Screen::Directory if cwd == self.home => crate::i18n::tr("Home").to_owned(),
            Screen::Directory => cwd.file_name().map_or_else(
                || cwd.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            Screen::Computer => crate::i18n::tr("Computer").to_owned(),
            Screen::Recent => crate::i18n::tr("Recent").to_owned(),
            Screen::Trash => crate::i18n::tr("Trash").to_owned(),
            Screen::Downloads => crate::i18n::tr("Download Manager").to_owned(),
            Screen::Network => crate::i18n::tr("Network").to_owned(),
        }
    }

    // Opens a tab at the current location beside the active one and switches to
    // it; `false` at [`MAX_TABS`].
    pub(crate) fn new_tab(&mut self) -> bool {
        if self.tabs.len() >= MAX_TABS {
            return false;
        }
        let fresh = TabSnapshot::at(self.cwd.clone(), self.default_view_mode);
        let left = self.take_live(fresh);
        let active = self.tabs.active;
        self.tabs.tabs[active].snapshot = Some(left);
        let id = self.tabs.mint();
        self.tabs
            .tabs
            .insert(active + 1, BrowserTab { id, snapshot: None });
        self.tabs.active = active + 1;
        true
    }

    // Make the tab at `index` the live one. Returns whether it changed.
    pub(crate) fn switch_tab(&mut self, index: usize) -> bool {
        if index == self.tabs.active || index >= self.tabs.len() {
            return false;
        }
        let Some(target) = self.tabs.tabs[index].snapshot.take() else {
            return false;
        };
        let left = self.take_live(target);
        let active = self.tabs.active;
        self.tabs.tabs[active].snapshot = Some(left);
        self.tabs.active = index;
        true
    }

    // Closes the tab at `index`; the last tab never closes. Closing the live tab
    // activates its right neighbour, or its left one at the end.
    pub(crate) fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return false;
        }
        if index == self.tabs.active {
            let next = if index + 1 < self.tabs.len() {
                index + 1
            } else {
                index - 1
            };
            let Some(target) = self.tabs.tabs[next].snapshot.take() else {
                return false;
            };
            let _closed = self.take_live(target);
            self.tabs.tabs.remove(index);
            self.tabs.active = if next > index { next - 1 } else { next };
        } else {
            self.tabs.tabs.remove(index);
            if index < self.tabs.active {
                self.tabs.active -= 1;
            }
        }
        true
    }

    // The tab locations in strip order and the active index, for the saved
    // session.
    pub(crate) fn session(&self) -> (Vec<PathBuf>, usize) {
        let locations = (0..self.tabs.len())
            .filter_map(|index| self.tab_location(index).map(std::path::Path::to_path_buf))
            .collect();
        (locations, self.tabs.active)
    }

    // Restores a saved session. Virtual or missing entries are skipped; an empty
    // session changes nothing.
    pub(crate) fn restore_session(&mut self, locations: &[PathBuf], active: usize) {
        let keep = |path: &&PathBuf| path.is_absolute() && !crate::services::is_virtual(path);
        let kept: Vec<&PathBuf> = locations.iter().take(MAX_TABS).filter(keep).collect();
        if kept.is_empty() {
            return;
        }
        // The saved index counts skipped tabs too: the active tab is the one
        // that was active, or the nearest kept tab before it.
        let skipped_before = locations
            .iter()
            .take(MAX_TABS)
            .take(active.saturating_add(1))
            .filter(|path| !keep(path))
            .count();
        let active = active.saturating_sub(skipped_before).min(kept.len() - 1);
        let mut tabs = Vec::with_capacity(kept.len());
        let mut next_id = 0;
        for (index, path) in kept.iter().enumerate() {
            let snapshot =
                (index != active).then(|| TabSnapshot::at((*path).clone(), self.default_view_mode));
            tabs.push(BrowserTab {
                id: next_id,
                snapshot,
            });
            next_id += 1;
        }
        self.tabs = Tabs {
            tabs,
            active,
            next_id,
        };
        if self.cwd != *kept[active] {
            let _ = self.take_live(TabSnapshot::at(
                kept[active].clone(),
                self.default_view_mode,
            ));
        }
    }
}

#[cfg(test)]
impl Tabs {}
