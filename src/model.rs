// Pure application state and the reducer's pure helpers.
//
// Everything here is render-free and filesystem-free; `services` does the I/O.

use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use enzui_core::list_selection::{ClickModifiers, IndexedClickModel, ListSelection};
use enzui_core::virtual_grid::TileProvider;
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};
use enzui_widgets::enz_text::TextSize;
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::text_measure::{MeasureTypography, enz_text_measure_width};

use crate::bulk_rename::BulkRename;
use crate::conflict::ConflictQueue;
use crate::services;

#[path = "model_parts/browse.rs"]
mod browse;
#[path = "model_parts/entries.rs"]
mod entries;
#[path = "model_parts/properties_info.rs"]
mod properties_info;
pub(crate) use entries::ProjectionPolicy;
pub(crate) use entries::{
    EntryKind, EntrySource, FileEntry, FileEntryProvider, ProjectedEntry, ThumbnailState,
};
pub(crate) use properties_info::PropertiesInfo;

// Which page of the multi-page View Settings dialog is shown.
//
// The `_ID` strings are the single mapping between framework `SettingsPage` ids,
// `.selected(..)` and the `on_select(String)` callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub(crate) enum SettingsSection {
    // How Files behaves: opening items, the terminal.
    #[default]
    General,
    ViewSettings,
    Extensions,
    Shortcuts,
    About,
}

impl SettingsSection {
    pub(crate) const GENERAL_ID: &'static str = "general";
    pub(crate) const VIEW_SETTINGS_ID: &'static str = "view-settings";
    pub(crate) const EXTENSIONS_ID: &'static str = "extensions";
    pub(crate) const SHORTCUTS_ID: &'static str = "shortcuts";
    pub(crate) const ABOUT_ID: &'static str = "about";

    #[must_use]
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::General => Self::GENERAL_ID,
            Self::ViewSettings => Self::VIEW_SETTINGS_ID,
            Self::Extensions => Self::EXTENSIONS_ID,
            Self::Shortcuts => Self::SHORTCUTS_ID,
            Self::About => Self::ABOUT_ID,
        }
    }

    #[must_use]
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            Self::GENERAL_ID => Some(Self::General),
            Self::VIEW_SETTINGS_ID => Some(Self::ViewSettings),
            Self::EXTENSIONS_ID => Some(Self::Extensions),
            Self::SHORTCUTS_ID => Some(Self::Shortcuts),
            Self::ABOUT_ID => Some(Self::About),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaceKind {
    Home,
    XdgDir,
    Bookmark,
    #[expect(
        dead_code,
        reason = "virtual recent-place mapping remains accepted without a recent-file provider"
    )]
    Recent,
    Trash,
    Computer,
    // The Downloads plugin's virtual panel (HTTP and optional torrent jobs).
    Downloads,
    // Network discovery and GVfs mounts.
    Network,
}

// How a `Drive` was classified from `/proc/mounts`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DriveKind {
    Local,
    Removable,
    Network,
    System,
}

// A mounted filesystem to render as a compact row in the Computer view.
#[derive(Clone, Debug)]
pub(crate) struct Drive {
    pub(crate) label: String,
    pub(crate) source: String,
    pub(crate) mount: PathBuf,
    pub(crate) filesystem: String,
    pub(crate) kind: DriveKind,
    pub(crate) total: u64,
    pub(crate) used: u64,
    pub(crate) free: u64,
}

impl Drive {
    // Percentage-used, clamped to `[0.0, 1.0]`.
    pub(crate) fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }

    // Whether this row may offer an "Unmount" action.
    //
    // Only removable media and network shares qualify; `System` and `Local` rows
    // hold the running system and the kernel would refuse.
    pub(crate) fn is_unmountable(&self) -> bool {
        matches!(self.kind, DriveKind::Removable | DriveKind::Network)
    }
}

// Stable collection identity for the reorderable pinned Places rail.
pub(crate) const SIDEBAR_PINS_COLLECTION_ID: &str = "files-app-sidebar-pins";

// A sidebar entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) id: String,
    pub(crate) kind: PlaceKind,
    pub(crate) label: String,
    // Lucide icon name — guaranteed registered (see `icon` module).
    pub(crate) icon: String,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewMode {
    List,
    Grid,
}

// Direction of keyboard movement through the current file view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectionDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

// The three toggleable trailing metadata columns of the list view, in render order.
//
// [`index`](MetaColumn::index) indexes both [`Model::visible_columns`] and the
// `[f32; 3]` width array in `view::list`, so visibility, width and header stay aligned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetaColumn {
    Size,
    Modified,
    Kind,
}

impl MetaColumn {
    // Every toggleable column, in render order.
    pub(crate) const ALL: [MetaColumn; 3] =
        [MetaColumn::Size, MetaColumn::Modified, MetaColumn::Kind];

    // The column's index into [`Model::visible_columns`] and the width array.
    pub(crate) fn index(self) -> usize {
        match self {
            MetaColumn::Size => 0,
            MetaColumn::Modified => 1,
            MetaColumn::Kind => 2,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            MetaColumn::Size => crate::i18n::tr("Size"),
            MetaColumn::Modified => crate::i18n::tr("Modified"),
            MetaColumn::Kind => crate::i18n::tr("Type"),
        }
    }
}

// The type facet of the search Filters. `Any` accepts every hit; the other buckets
// classify by `EntryKind` and MIME. Applied only while a search is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchFilterType {
    Any,
    Folder,
    Image,
    Document,
    Audio,
    Video,
}

impl SearchFilterType {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SearchFilterType::Any => crate::i18n::tr("Any type"),
            SearchFilterType::Folder => crate::i18n::tr("Folders"),
            SearchFilterType::Image => crate::i18n::tr("Images"),
            SearchFilterType::Document => crate::i18n::tr("Documents"),
            SearchFilterType::Audio => crate::i18n::tr("Audio"),
            SearchFilterType::Video => crate::i18n::tr("Video"),
        }
    }

    // True if `entry` falls in this type bucket. `Any` accepts everything.
    pub(crate) fn matches(self, entry: &FileEntry) -> bool {
        match self {
            SearchFilterType::Any => true,
            SearchFilterType::Folder => matches!(entry.kind, EntryKind::Dir),
            SearchFilterType::Image => entry.mime.starts_with("image/"),
            SearchFilterType::Audio => entry.mime.starts_with("audio/"),
            SearchFilterType::Video => entry.mime.starts_with("video/"),
            SearchFilterType::Document => {
                if entry.mime.starts_with("text/") {
                    return true;
                }
                matches!(
                    entry.mime.as_str(),
                    "application/pdf"
                        | "application/msword"
                        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                        | "application/vnd.oasis.opendocument.text"
                        | "application/vnd.ms-excel"
                        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                        | "application/rtf"
                        | "application/epub+zip"
                )
            }
        }
    }
}

// Entries without a modification time fail every non-`Any` date filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchFilterDate {
    Any,
    Today,
    Week,
    Month,
}

impl SearchFilterDate {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SearchFilterDate::Any => crate::i18n::tr("Any date"),
            SearchFilterDate::Today => crate::i18n::tr("Today"),
            SearchFilterDate::Week => crate::i18n::tr("This week"),
            SearchFilterDate::Month => crate::i18n::tr("This month"),
        }
    }

    // True if `entry.modified` lies within the bucket relative to `now`.
    pub(crate) fn matches(self, entry: &FileEntry, now: SystemTime) -> bool {
        if matches!(self, SearchFilterDate::Any) {
            return true;
        }
        let Some(m) = entry.modified else {
            return false;
        };
        let Ok(elapsed) = now.duration_since(m) else {
            return true;
        };
        let secs = elapsed.as_secs();
        match self {
            SearchFilterDate::Any => true,
            SearchFilterDate::Today => secs < 24 * 3600,
            SearchFilterDate::Week => secs < 7 * 24 * 3600,
            SearchFilterDate::Month => secs < 30 * 24 * 3600,
        }
    }
}

// The scope a search targets.
//
// `Folder` filters the loaded entries without I/O. `Everywhere` walks the subtree
// rooted at `cwd` off the reducer and draws hits from [`Model::search_results`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub(crate) enum SearchScope {
    // Filter the current folder's loaded entries only.
    #[default]
    Folder,
    // Recursive walk of the subtree rooted at `cwd`.
    Everywhere,
}

impl SortKey {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SortKey::Name => crate::i18n::tr("Name"),
            SortKey::Size => crate::i18n::tr("Size"),
            SortKey::Modified => crate::i18n::tr("Modified"),
            SortKey::Kind => crate::i18n::tr("Type"),
        }
    }
}

// Which content the pane is showing. Virtual locations are their own screens
// rather than sentinel paths shimmed through `read_directory`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Screen {
    // A real directory listing.
    Directory,
    // The Computer / My PC drive listing.
    Computer,
    // The Recent virtual listing.
    Recent,
    // The Trash virtual listing.
    Trash,
    // The Downloads plugin panel (HTTP and optional torrent job list).
    Downloads,
    // The Network discovery and mount panel.
    Network,
}

// Path-free incompleteness outcome of a sidebar Trash scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrashListingIncomplete {
    // The `<trash>/files` directory itself was unreadable.
    Unavailable,
    // The `<trash>/files` iterator failed on a directory entry.
    IteratorFailed,
    // The scan stopped at [`crate::trash::MAX_RETAINED_TRASH_ENTRIES`].
    EntryCapReached,
    // The scan visited [`crate::trash::MAX_CANDIDATES_SCANNED`] candidates.
    CandidateCapReached,
    // `n` `.trashinfo` twins were refused; the payload rows remain.
    RefusedMetadata(usize),
}

#[path = "model_parts/interaction.rs"]
mod interaction;

// List zoom levels. 1.0 is the tuned 30 px row with an 18 px icon;
// the smallest level still clears the 24 px hit-target floor.
pub(crate) const LIST_ZOOM_FACTORS: &[f32] = &[0.9, 1.0, 1.25, 1.5, 2.0];
// Grid zoom levels. 1.0 is the `icon_xxl` tile; the glyph band,
// thumbnail and tile footprint scale together.
pub(crate) const GRID_ZOOM_FACTORS: &[f32] = &[0.75, 1.0, 1.5, 2.0, 3.0];

pub(crate) fn list_zoom_ladder() -> enzui_core::zoom_levels::ZoomLadder {
    enzui_core::zoom_levels::ZoomLadder::new(LIST_ZOOM_FACTORS, 1)
}

pub(crate) fn grid_zoom_ladder() -> enzui_core::zoom_levels::ZoomLadder {
    enzui_core::zoom_levels::ZoomLadder::new(GRID_ZOOM_FACTORS, 1)
}

// The whole app state. A flat struct, mutated by the reducer.
#[derive(Debug)]

pub(crate) struct Model {
    pub(crate) cwd: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) screen: Screen,
    // Entries for the current directory, before sorting and filtering.
    pub(crate) entries: Vec<FileEntry>,
    pub(crate) pending_copy_folders: Vec<PendingCopyFolder>,
    pub(crate) load_error: Option<String>,
    // Id of the most recently dispatched directory read, from [`Model::begin_load`].
    //
    // Only a `DirLoaded` carrying this value is applied, so a superseded read of
    // the same path cannot overwrite the current entries.
    pub(crate) load_generation: u64,

    pub(crate) back: Vec<PathBuf>,
    pub(crate) forward: Vec<PathBuf>,

    pub(crate) places: Vec<Place>,
    // Ordered, reorderable sidebar pins, seeded with Home and XDG folders on first run.
    //
    // Persisted to `$XDG_CONFIG_HOME/files-app/bookmarks`; `App::new` disables persistence.
    pub(crate) bookmarks: Vec<Place>,

    // Retained count of admitted pin rows, for diagnostic counters.
    pub(crate) pin_retained_rows: usize,
    // Retained byte count of admitted label+path content.
    pub(crate) pin_retained_bytes: usize,

    // Why the pins file was refused. While set, the sidebar shows Retry/Retire and
    // every pin mutation and save is blocked.
    pub(crate) pins_refused: Option<services::PinRefusal>,

    // Set when publication reached the destination name but durability or late
    // validation failed (`PublishCustody::Rotated`). Retry republishes the retained
    // mutation; success clears it.
    pub(crate) pin_durability_debt: bool,

    pub(crate) drives: Vec<Drive>,
    pub(crate) storage_locations: Vec<crate::storage::Location>,
    pub(crate) storage_error: Option<String>,
    pub(crate) storage_ready: bool,
    pub(crate) storage_capture: bool,
    pub(crate) storage_busy: std::collections::HashSet<String>,

    // The Trash listing: one row per `<trash>/files/NAME` with its `.trashinfo` origin.
    pub(crate) trash_entries: Vec<crate::trash::TrashEntry>,
    // What each listed Trash payload is (folder, or file and size), by
    // trashed path: the Size and Type columns.
    pub(crate) trash_payloads: std::collections::HashMap<PathBuf, crate::trash::TrashPayload>,
    // Folders in the current folder whose size was measured or is being
    // measured, by path. Leaving the folder drops them.
    pub(crate) folder_sizes: std::collections::HashMap<PathBuf, crate::folder_size::FolderSize>,
    // Incompleteness outcome of the most recent Trash scan.
    pub(crate) trash_listing_incomplete: Option<TrashListingIncomplete>,

    // The view new and restored tabs open in, and the one saved: changed
    // only by choosing a view, never by switching to a tab that shows another.
    pub(crate) default_view_mode: ViewMode,
    pub(crate) view_mode: ViewMode,
    pub(crate) sort_key: SortKey,
    pub(crate) sort_ascending: bool,
    pub(crate) sort_folders_first: bool,
    pub(crate) show_hidden: bool,
    pub(crate) colored_file_icons: bool,
    // The list view's zoom level, an index into [`LIST_ZOOM_FACTORS`].
    pub(crate) list_zoom: usize,
    // The grid view's zoom level, an index into [`GRID_ZOOM_FACTORS`].
    pub(crate) grid_zoom: usize,
    // The user's terminal command for Open in Terminal; empty means
    // the first installed of `terminal::CANDIDATES`.
    pub(crate) terminal_command: String,
    // The last Templates folder scan, refreshed on each background
    // menu opening.
    pub(crate) templates: crate::templates::Templates,
    // Folder colour tags, persisted with the view settings.
    pub(crate) folder_colors: enzui_core::persist::Tracked<crate::folder_colors::FolderColors>,
    // The identity of each listed folder, from the scan that follows
    // every directory load.
    pub(crate) folder_identities:
        std::collections::HashMap<PathBuf, crate::folder_colors::FolderIdentity>,
    // The folder whose colour chooser is open.
    pub(crate) folder_color_dialog: Option<PathBuf>,
    // Previous versions or a folder as it was.
    pub(crate) history: Option<crate::history::Dialog>,
    // The history service is installed; the person's excluded folders.
    pub(crate) history_available: bool,
    pub(crate) history_excluded: Vec<PathBuf>,
    // Capture-only sample history (`FILES_APP_OPEN`); the live check never overrides it.
    pub(crate) history_fixture: bool,
    // The window's tabs; the active one's state is the fields above.
    pub(crate) tabs: crate::tabs::Tabs,
    pub(crate) tab_list_open: bool,

    // The sort axis a folder without its own override uses.
    //
    // The View Settings radios read and write the defaults; a column-header re-sort
    // does not. Persisted with the other view settings in one versioned record (see
    // `crate::view_settings_store`); the live `sort_key` is derived, not persisted.
    pub(crate) default_sort_key: SortKey,
    pub(crate) default_sort_ascending: bool,
    // Per-directory sort chosen by a column-header sort ([`Self::sort_by`]).
    //
    // Persisted with the view settings. [`Tracked`](enzui_core::persist::Tracked)
    // gives the save gate an O(1) revision, and every `&mut` access bumps it.
    pub(crate) sort_overrides:
        enzui_core::persist::Tracked<std::collections::BTreeMap<PathBuf, (SortKey, bool)>>,

    // Which of the Size, Modified and Type list columns are shown, indexed by
    // [`MetaColumn::index`]. A hidden column is absent from the table. All default on.
    pub(crate) visible_columns: [bool; 3],

    // Metadata shown below names in Grid view, indexed by [`MetaColumn`].
    // Off by default so the compact icon view stays compact until requested.
    pub(crate) grid_info: [bool; 3],

    // Per-MIME application chosen with the Open With dialog's "Always this app".
    //
    // Values are structured launches whose argv is fixed when chosen. This is the
    // in-session cache; the durable write goes through `MimeQuery::set_default_application`.
    pub(crate) mime_defaults: std::collections::HashMap<String, crate::launch::LaunchSpec>,

    // The selection set, keyed by path (stable across re-sorts).
    pub(crate) selection: ListSelection<PathBuf>,

    // Search state. `search_active` flips the titlebar into a search field.
    pub(crate) search_active: bool,
    pub(crate) search_query: String,
    // Whether the query targets the current folder or its subtree; meaningful only
    // while `search_active`.
    pub(crate) search_scope: SearchScope,
    // Subtree search hits remain empty until the current walk finishes.
    pub(crate) search_results: Vec<FileEntry>,
    // Monotonic identity of the latest recursive search request or
    // invalidation. Replies apply only while their generation is current.
    pub(crate) search_generation: u64,

    // The search Filters type facet, applied only while `search_active`.
    pub(crate) search_filter_type: SearchFilterType,
    pub(crate) search_filter_date: SearchFilterDate,
    pub(crate) filters_open: bool,

    pub(crate) path_edit_active: bool,
    pub(crate) path_edit_draft: String,
    pub(crate) path_suggestions: Vec<PathBuf>,

    // The collapsed display of `cwd` shown by the idle path field.
    //
    // Lives on the model because `EnzTextField::value` borrows a `&str`. Every reducer
    // arm that changes `cwd`, `home` or `screen` must call
    // [`Model::refresh_path_display`].
    pub(crate) path_display: String,

    pub(crate) details_open: bool,

    // The Properties dialog target, or `None` when closed.
    //
    // A snapshot, so the dialog does not re-read the filesystem every frame and the
    // permission checkboxes bind to the nine mode bits.
    pub(crate) properties: Option<PropertiesInfo>,
    pub(crate) inspector_properties: Option<PropertiesInfo>,
    pub(crate) inspector_pending: Option<PathBuf>,

    // Which View Settings value menu is open, if any: one at a time, by
    // construction.
    pub(crate) settings_menu: Option<SettingsMenu>,

    // Whether the Properties Permissions section is expanded; kept outside
    // `PropertiesInfo` so it survives an Apply re-read.
    pub(crate) properties_perms_expanded: bool,

    // Whether the framework titlebar's measured command overflow is open.
    // Geometry remains view-derived; this is only the controlled popup state.
    pub(crate) toolbar_overflow_open: bool,

    pub(crate) view_settings_open: bool,

    pub(crate) settings_page: SettingsSection,

    // Whether a page is open over the page list in the narrow-window
    // (stacked) settings navigation; Back returns to the list.
    pub(crate) settings_page_open: bool,

    // Whether the Extensions page shows the Downloads settings subpage.
    pub(crate) downloads_settings_open: bool,

    // The shortcut action whose chip is recording, its refusal feedback as
    // `(action, reason)`, and a load or save problem with the overrides file.
    pub(crate) shortcut_capture: Option<String>,
    pub(crate) shortcut_feedback: Option<(String, String)>,
    pub(crate) shortcuts_notice: Option<String>,

    // The navigation-shell state read by `enz_navigation_split_view`.
    //
    // The only sidebar state: collapse comes from `nav_split::resolve_for_width` and
    // toggling from `NavSplitState::toggle_for_presentation`.
    pub(crate) nav: NavSplitState,

    // The bulk-rename dialog, or `None` when closed. Its `plan()` drives both the
    // preview and the executor.
    pub(crate) bulk_rename: Option<BulkRename>,

    // A one-row rename edited in place in the list or grid. The same
    // single planner as the dialog, so validation and conflicts cannot drift.
    pub(crate) inline_rename: Option<BulkRename>,
    // The in-place edit settled by a click elsewhere: the click already chose
    // what is selected next, so the finished rename leaves selection alone.
    pub(crate) inline_rename_blurred: bool,
    // A just-created entry that opens in place for naming once it is listed.
    pub(crate) rename_after_load: Option<PathBuf>,

    // Removable filesystems for the Computer view.
    pub(crate) removable_devices: Vec<crate::devices::RemovableDevice>,
    // Phones and cameras that share files over MTP.
    pub(crate) phones: Vec<crate::devices::phones::Phone>,
    // An Eject or Safely Remove awaiting confirmation, by device id.
    pub(crate) pending_device_confirm: Option<(String, crate::devices::DeviceOp)>,
    // A mount's open question (an unknown SFTP host key), waiting
    // for the user. Only the user's answer resolves it.
    pub(crate) mount_question: Option<crate::network::AskedQuestion>,
    // The passphrase prompt for a locked encrypted drive.
    pub(crate) unlock: Option<crate::mount_service::UnlockState>,
    // Rename, share picker or confirmation for a mount service location.
    pub(crate) location_dialog: Option<crate::mount_service::LocationDialog>,
    // The Format dialog for a removable drive's volume.
    pub(crate) format: Option<crate::mount_service::FormatState>,
    // The Add a Cloud Drive dialog.
    pub(crate) cloud: Option<crate::mount_service::CloudState>,
    // The cloud services the mount service can add; empty without it.
    pub(crate) cloud_providers: Vec<crate::mount_service::CloudProvider>,

    // The entry to select and focus once the next directory load
    // lands, such as the name an inline rename just published.
    pub(crate) focus_after_load: Option<PathBuf>,

    // The paused paste awaiting conflict decisions, or `None`.
    pub(crate) conflicts: Option<ConflictQueue>,

    pub(crate) clipboard_pending: bool,

    // The open right-click context menu, or `None` when none is up.
    // Carries the right-clicked path, the raw anchor point and the roving
    // keyboard cursor. Clamping and dismissal belong to the
    // `enz_positioned_overlay` in `App::view`, not here.
    pub(crate) context_menu: Option<crate::context_menu::OpenContextMenu>,

    // Session-only Undo/Redo journal. Owned by `Model` so the
    // view can read `peek_undo` / `peek_redo` at menu-open time.
    pub(crate) journal: crate::journal::OpJournal,

    // Live prompts published by Undo/Redo attempts that returned
    // a typed conflict. Each prompt's captured `entry_generation`
    // is what the reducer's stale-id guard consults.
    pub(crate) journal_prompts: Vec<crate::journal::JournalPrompt>,

    // Whether an off-thread conflict batch is in flight; the dialog's actions are
    // disabled meanwhile so one conflict cannot be resolved twice.
    pub(crate) conflicts_busy: bool,

    pub(crate) bulk_rename_busy: bool,

    // A reducer-observable copy of the most recent transient outcome, for tests.
    //
    // The rendered and accessible surface is `App::toasts` ([`ToastHost`]). Write only
    // through `App::post_status`; `no_invisible_status_producer_remains` forbids
    // direct writes.
    //
    // [`ToastHost`]: enzui_widgets::toast_host::ToastHost
    pub(crate) status_message: Option<String>,

    // The Open With dialog, or `None` when closed.
    pub(crate) open_with: Option<crate::open_with::OpenWithState>,

    // The Attach-network-location dialog, or `None` when closed. Passwords render
    // only through `enz_secure_field`.
    pub(crate) attach: Option<crate::network::AttachState>,

    pub(crate) single_click_open: bool,

    pub(crate) select_matching_open: bool,

    // Whether the keyboard-shortcuts dialog (F1) listing `keybindings::ALL_BINDINGS`
    // is open.
    pub(crate) shortcuts_open: bool,

    //
    // It lists every held row in attention order, so the activity surface's `+N more`
    // overflow always has a destination that contains the hidden work.
    pub(crate) all_activity_open: bool,

    // The pending permanent-delete confirmation.
    //
    // The typed request distinguishes ordinary selected paths from Trash
    // entries, whose `.trashinfo` twins must be removed by the same confirmed
    // act. `None` means no destructive confirmation is up.
    pub(crate) pending_permanent_delete: Option<crate::trash::DeleteRequest>,

    // A Trash entry currently using the existing conflict queue because its
    // original path is occupied. The queue remains the sole Replace/Merge/
    // Skip policy; this value only remembers which `.trashinfo` to clean after
    // a successful move.
    pub(crate) pending_trash_restore: Option<crate::trash::TrashEntry>,

    // Backing state for the permanent-delete `enz_confirm_dialog`; reset on re-open.
    pub(crate) delete_confirm_state: enzui_core::confirm::ConfirmState,

    // The glob draft in the Select items matching field.
    pub(crate) select_matching_draft: String,

    // The Downloads plugin panel: backend status, the job list and the
    // Add-download dialog. Reached from the "Download Manager" sidebar row.
    pub(crate) downloads: crate::downloads::DownloadsState,

    // Optional module enablement, read by every surface and work scheduler.
    pub(crate) features: crate::features::Features,
    pub(crate) extensions: crate::extensions::Catalog,
    // Why the feature-state source was refused at load or last publication.
    // While set, toggles are disabled and the Extensions page surfaces the
    // refusal with Retry/Replace actions.
    pub(crate) features_refusal: Option<crate::features::FeaturesRefusal>,
    // A Rotated publication means the intended document is the live model
    // and the authority owns the new inode, but durability failed. Retry
    // republishes the exact retained bytes.
    pub(crate) features_durability_debt: bool,
    // Network-tab discovery state, independent of the named-server dialog.
    pub(crate) network: crate::network::NetworkState,

    // App-owned thumbnail lifecycle keyed by canonical entry path.
    pub(crate) thumbnails: std::collections::HashMap<PathBuf, ThumbnailState>,
    // Application-owned, keyed ordering for the virtual list and grid.
    pub(crate) visible_provider: FileEntryProvider,
    // Scroll offsets in content coordinates. They drive both realisation and
    // the bounded thumbnail request window.
    pub(crate) list_scroll_y: f32,
    pub(crate) grid_scroll_y: f32,
    // A navigation zeroed the offsets above; the next load also scrolls
    // the views' own scrollables back to the top.
    pub(crate) scroll_reset_owed: bool,
    // A navigation cleared the old folder's entries and the new folder's
    // listing has not landed yet; the view shows neither the old files nor
    // "empty". `listing_slow` turns on once the wait outlasts a short delay,
    // so a loading state shows only when it is worth seeing.
    pub(crate) listing_pending: bool,
    pub(crate) listing_slow: bool,
    // Downloads list scroll offset, feeding the virtual list's realization
    // window.
    pub(crate) downloads_scroll_y: f32,
    // Laid-out viewport of the displayed list or grid, in logical pixels.
    pub(crate) directory_view_bounds: enzui_widgets::LayoutBounds,
    // Last adaptive geometry resolved from the virtual grid's real allocation.
    pub(crate) grid_layout: std::cell::Cell<enzui_core::virtual_grid::ResolvedTileGrid>,
    // Live editor bounds used by the root dismissal scope.
    pub(crate) breadcrumb_edit_bounds: enzui_widgets::breadcrumb::EditBounds,

    // The OS file drag hovering the window, which shows the sidebar's "Drop to pin"
    // target. Driven by iced's `FileHovered` / `FilesHoveredLeft` / `FileDropped`.
    pub(crate) pin_drop_hover: Option<PathBuf>,

    // The most recent primary click on a row or tile, for double-click detection.
    //
    // A second click on the same path within
    // `enzui_core::click::DEFAULT_MULTI_CLICK_INTERVAL` opens it. Any activation
    // or modifier click clears it. Not persisted. `Instant` because wall-clock time
    // can jump backwards.
    pub(crate) last_row_press: Option<(PathBuf, std::time::Instant)>,
    // The selected row on the Computer and Network pages, and its last press
    // for double-click detection.
    pub(crate) device_row_selected: Option<String>,
    pub(crate) last_device_press: Option<(String, std::time::Instant)>,
    // The open folder's repository and project tools.
    pub(crate) project: crate::project::Project,
}

// Character budget for the idle path field's collapsed display.
//
// Longer paths collapse to a leading `…/` plus the deepest components that fit.
pub(crate) const PATH_DISPLAY_BUDGET_CHARS: usize = 40;

// Collapses a breadcrumb-style path to at most `budget` characters.
//
// A path within budget is unchanged; otherwise leading segments become `…/`.
pub(crate) fn collapse_display_path(full: &str, budget: usize) -> String {
    if full.chars().count() <= budget {
        return full.to_owned();
    }
    let parts: Vec<&str> = full.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return full.to_owned();
    }
    for n in (1..parts.len()).rev() {
        let tail = &parts[parts.len() - n..];
        let candidate = format!("…/{}", tail.join("/"));
        if candidate.chars().count() <= budget {
            return candidate;
        }
    }
    format!("…/{}", parts.last().copied().unwrap_or(""))
}

// Folders sort before files (priority 0 vs 1) when folders-first is on.
fn dir_priority(entry: &FileEntry) -> u8 {
    match entry.kind {
        EntryKind::Dir => 0,
        _ => 1,
    }
}

// Natural ordering: digit runs compare numerically, the rest case-insensitively.
//
// The framework comparator is a total order: names differing only in case or
// zero-padding never tie, so a refresh cannot reorder a folder.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    enzui_core::file_browser::natural_cmp(a, b)
}

// A View Settings row that presents its choices behind a value menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsMenu {
    SortBy,
    SortOrder,
    View,
}

#[derive(Debug)]
pub(crate) struct PendingCopyFolder {
    pub(crate) path: PathBuf,
    pub(crate) activity: Option<crate::activity::FileActivity>,
}
