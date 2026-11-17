// Point anchors survive virtual-list scrolling; menus show actions supported by the target.

use enzui_core::context_menu::{InvocationKind, Point as CorePoint};
use enzui_widgets::context_menu_host::{ContextMenu, ContextMenuItem, FocusRestoreTarget};
use iced::Point;
use std::path::{Path, PathBuf};

use crate::message::Msg;
use crate::model::{EntryKind, Model};

#[path = "context_menu/ids.rs"]
mod ids;
#[path = "context_menu/models.rs"]
mod models;
#[path = "context_menu/rows.rs"]
mod rows;

pub(crate) use ids::{grid_tile_id, list_context_id, list_row_id, trash_context_id, trash_row_id};
use models::preview_mime_of;
pub(crate) use models::{
    background_menu_model, command_allowed, items_for_target, location_menu_model, menu_label,
    menu_model, sidebar_menu_model, trash_menu_model,
};
use rows::RowGroup;
pub(crate) use rows::RowKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextMenuOrigin {
    Entry,
    Sidebar,
    Background,
    // A mount service network location, keyed by its storage id.
    Location,
}

// A command in the context menu for unoccupied listing space: creation,
// clipboard and selection, history, then the terminal. Sorting and the view
// mode live in the toolbar and Settings, not here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackgroundRowKind {
    NewFolder,
    NewFile,
    Paste,
    // Link the desktop clipboard's files into this folder.
    PasteAsLink,
    SelectAll,
    Undo,
    Redo,
    // Open a terminal in the current folder.
    OpenInTerminal,
}

impl BackgroundRowKind {
    // Whether a separator comes before this row: each group after the first
    // starts with one.
    const fn starts_group(self) -> bool {
        matches!(self, Self::Paste | Self::Undo | Self::OpenInTerminal)
    }

    pub(crate) const ALL: [Self; 8] = [
        Self::NewFolder,
        Self::NewFile,
        Self::Paste,
        Self::PasteAsLink,
        Self::SelectAll,
        Self::Undo,
        Self::Redo,
        Self::OpenInTerminal,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NewFolder => crate::i18n::tr("New Folder"),
            Self::NewFile => crate::i18n::tr("New File"),
            Self::Paste => crate::i18n::tr("Paste"),
            Self::PasteAsLink => crate::i18n::tr("Paste as Link"),
            Self::SelectAll => crate::i18n::tr("Select All"),
            // `background_menu_model` labels the stack top when one exists.
            Self::Undo => crate::i18n::tr("Undo"),
            Self::Redo => crate::i18n::tr("Redo"),
            Self::OpenInTerminal => crate::i18n::tr("Open in Terminal"),
        }
    }

    const fn icon(self) -> &'static str {
        match self {
            Self::NewFolder => "folder-plus",
            Self::NewFile => "file-plus",
            Self::Paste => "clipboard-paste",
            Self::PasteAsLink => "link",
            Self::SelectAll => "list-checks",
            Self::Undo => "undo-2",
            Self::Redo => "redo-2",
            Self::OpenInTerminal => "terminal",
        }
    }

    pub(crate) fn message(self) -> Msg {
        match self {
            Self::NewFolder => Msg::NewFolder,
            Self::NewFile => Msg::NewFile,
            Self::Paste => Msg::PasteHere,
            Self::PasteAsLink => Msg::PasteAsLink,
            Self::SelectAll => Msg::SelectAll,
            Self::Undo => Msg::UndoLastOperation,
            Self::Redo => Msg::RedoLastUndone,
            Self::OpenInTerminal => Msg::OpenInTerminal(None),
        }
    }
}

// The Backup submenu (enzimOS history service).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackupAction {
    // Earlier versions of a file.
    Versions,
    // A folder as it was at an earlier time.
    Browse,
    // Leave the folder or file out of backups and history from now on.
    Exclude,
    // Back in backups.
    Include,
    // Start a backup to the backup drive.
    BackUpNow,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Row(RowKind),
    Backup(BackupAction),
    Background(BackgroundRowKind),
    Template(PathBuf),
    Extension { extension: String, action: String },
    Location(crate::mount_service::LocationAction),
}

#[derive(Debug)]
pub(crate) struct OpenContextMenu {
    pub(crate) path: PathBuf,
    pub(crate) origin: ContextMenuOrigin,
    pub(crate) at: Point,
    pub(crate) generation: u64,
    pub(crate) menu: ContextMenu<PathBuf, Command>,
    invoker: Option<iced::advanced::widget::Id>,
}

impl OpenContextMenu {
    pub(crate) fn new_with_restore(
        path: impl Into<PathBuf>,
        at: Point,
        items: Vec<ContextMenuItem<Command>>,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let mut open = Self {
            path: path.into(),
            origin: ContextMenuOrigin::Entry,
            at,
            generation: 0,
            menu: ContextMenu::new(),
            invoker,
        };
        open.refresh(items);
        Some(open)
    }

    pub(crate) fn refresh(&mut self, items: Vec<ContextMenuItem<Command>>) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        // Queued events from a replaced inventory cannot select a newly numbered row.
        self.generation = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = InvocationKind::Pointer(CorePoint::new(self.at.x, self.at.y));
        if let Some(invoker) = &self.invoker {
            self.menu.open_restoring(
                self.path.clone(),
                at,
                items,
                FocusRestoreTarget::widget(invoker.clone()),
            );
        } else {
            self.menu.open(self.path.clone(), at, items);
        }
    }

    pub(crate) fn sidebar(
        path: impl Into<PathBuf>,
        at: Point,
        items: Vec<ContextMenuItem<Command>>,
    ) -> Option<Self> {
        Self::new_with_restore(path, at, items, None).map(|mut open| {
            open.origin = ContextMenuOrigin::Sidebar;
            open
        })
    }

    pub(crate) fn location(
        id: &str,
        at: Point,
        items: Vec<ContextMenuItem<Command>>,
    ) -> Option<Self> {
        Self::new_with_restore(id, at, items, None).map(|mut open| {
            open.origin = ContextMenuOrigin::Location;
            open
        })
    }

    pub(crate) fn background(
        path: impl Into<PathBuf>,
        at: Point,
        items: Vec<ContextMenuItem<Command>>,
    ) -> Option<Self> {
        Self::new_with_restore(path, at, items, None).map(|mut open| {
            open.origin = ContextMenuOrigin::Background;
            open
        })
    }
}

pub(crate) fn message_for(row: RowKind, path: &Path, kind: EntryKind) -> Msg {
    match row {
        RowKind::MountImage => Msg::MountDiskImage(path.to_path_buf()),
        RowKind::Open => Msg::OpenEntry(path.to_path_buf(), kind),
        RowKind::OpenInTerminal => Msg::OpenInTerminal(Some(path.to_path_buf())),
        RowKind::RevealParent => Msg::NavigateTo(
            path.parent()
                .map_or_else(|| path.to_path_buf(), Path::to_path_buf),
        ),
        RowKind::Cut => Msg::CutSelection,
        RowKind::Copy => Msg::CopySelection,
        // The right-clicked path, not the selection.
        RowKind::CopyPath => Msg::CopyRowPath(path.to_path_buf()),
        RowKind::CopyTo => Msg::BeginCopySelectionToFolder,
        RowKind::MoveTo => Msg::BeginMoveSelectionToFolder,
        RowKind::Duplicate => Msg::DuplicateSelection,
        RowKind::CreateLink => Msg::LinkSelection,
        RowKind::Rename | RowKind::RenameMultiple => Msg::BeginBulkRename,
        RowKind::OpenWith => Msg::ShowOpenWith(path.to_path_buf()),
        RowKind::Properties => Msg::ShowProperties(path.to_path_buf()),
        RowKind::FolderColor => Msg::ShowFolderColor(path.to_path_buf()),
        RowKind::DownloadTorrent => Msg::EnqueueTorrent(path.to_path_buf()),
        RowKind::OpenPreview => Msg::OpenPreview(path.to_path_buf()),
        // Destructive verbs act on the whole selection, not the clicked row.
        RowKind::ExtractHere => Msg::ExtractArchive {
            archive: path.to_path_buf(),
            dest_dir: None,
        },
        RowKind::ExtractTo => Msg::BeginExtractArchiveTo(path.to_path_buf()),
        RowKind::CompressZip => Msg::CompressSelection(crate::archive::ArchiveFormat::Zip),
        RowKind::CompressTarGz => Msg::CompressSelection(crate::archive::ArchiveFormat::TarGz),
        RowKind::MoveToTrash => Msg::TrashSelection,
        RowKind::DeletePermanently => Msg::AskPermanentDelete,
        RowKind::RestoreFromTrash => Msg::RestoreFromTrash(path.to_path_buf()),
        RowKind::DeleteTrashPermanently => Msg::AskDeleteTrashEntry(path.to_path_buf()),
        // Pinning is unary: it addresses the right-clicked path.
        RowKind::PinToSidebar => Msg::PinToSidebar(path.to_path_buf()),
        RowKind::UnpinFromSidebar => Msg::UnpinFromSidebar(path.to_path_buf()),
    }
}

// Resolve a Trash-only row to its real reducer action.
pub(crate) fn message_for_trash(row: RowKind, trashed_path: &Path) -> Option<Msg> {
    match row {
        RowKind::RestoreFromTrash => Some(Msg::RestoreFromTrash(trashed_path.to_path_buf())),
        RowKind::DeleteTrashPermanently => {
            Some(Msg::AskDeleteTrashEntry(trashed_path.to_path_buf()))
        }
        _ => None,
    }
}

fn is_regular_torrent_entry(model: &Model, path: &Path, is_dir: bool) -> bool {
    // Symlinks and special entries must not reach a backend as local metainfo;
    // enqueue re-checks the filesystem at activation.
    !is_dir
        && model
            .entry_for_path(path)
            .is_some_and(|entry| entry.kind == EntryKind::File)
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
}
