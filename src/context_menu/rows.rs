// Context-menu rows: kinds, groups, and their rules.

use super::*;

// A row of the file context menu.
//
// Labels and effects come from matches on this one enum, so they cannot drift.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RowKind {
    // Open the entry; supported archives extract beside themselves.
    Open,
    // Mount a local ISO read-only through the desktop storage service.
    MountImage,
    // Navigate to the entry's parent directory.
    RevealParent,
    // Open a terminal in this folder (directories only).
    OpenInTerminal,
    // Cut the selection to the clipboard.
    Cut,
    // Copy the selection to the clipboard.
    Copy,
    // Copy the row's absolute path to the system text clipboard, unlike
    // [`Self::Copy`], which fills the file-operation paste buffer.
    CopyPath,
    // Choose a folder, then copy the selection into it.
    CopyTo,
    // Choose a folder, then move the selection into it.
    MoveTo,
    // Copy the selection beside itself as `Name (copy)`.
    Duplicate,
    // Create a symbolic link to each selected entry beside it.
    CreateLink,
    // Rename the single selected entry.
    Rename,
    // Open the batch-rename dialog on a multi-selection.
    RenameMultiple,
    // Open the "Open With…" chooser for the entry.
    OpenWith,
    // Choose or clear the folder's colour tag.
    FolderColor,
    // Show the properties dialog for the entry.
    Properties,
    // Hand a local `.torrent` file to the Downloads module.
    DownloadTorrent,
    // Open the entry in a detached preview window.
    OpenPreview,
    // Extract an archive into a new folder beside it.
    ExtractHere,
    // Choose a folder, then extract an archive into it.
    ExtractTo,
    // Compress the selection into a ZIP archive in this folder.
    CompressZip,
    // Compress the selection into a `.tar.gz` archive in this folder.
    CompressTarGz,
    // Move the entry to the Trash.
    MoveToTrash,
    // Permanently delete the entry — opens the confirmation dialog.
    DeletePermanently,
    // Restore a Trash payload to the path recorded in its `.trashinfo`.
    RestoreFromTrash,
    // Permanently delete one Trash payload and its `.trashinfo` twin.
    DeleteTrashPermanently,
    // Pin a directory that is not pinned yet to the Places sidebar.
    PinToSidebar,
    // Unpin a folder from the Places sidebar; label and effect both follow
    // [`Model::is_pinned`].
    UnpinFromSidebar,
}

// Separator-delimited groups of menu rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub(crate) enum RowGroup {
    // Open, Open With, Preview, Reveal Parent and the terminal.
    Navigate,
    // Clipboard verbs.
    Clipboard,
    // Extract and Compress.
    Archive,
    // Trash and permanent delete, separated from the file operations above.
    Destructive,
    // Folder colour, torrent hand-off and Properties.
    Inspect,
}

impl RowKind {
    // Every row, in menu order.
    //
    // Rows are disabled rather than dropped, so rows below never shift under the
    // cursor. Pin and Unpin are excluded: only one applies, so [`menu_model`]
    // inserts it in the Navigate group. Groups must stay monotonic for the
    // separator renderer.
    pub(crate) const ALL: [RowKind; 24] = [
        RowKind::Open,
        RowKind::OpenWith,
        RowKind::OpenPreview,
        RowKind::MountImage,
        RowKind::RevealParent,
        RowKind::OpenInTerminal,
        RowKind::Cut,
        RowKind::Copy,
        RowKind::CopyPath,
        RowKind::CopyTo,
        RowKind::MoveTo,
        RowKind::Duplicate,
        RowKind::CreateLink,
        RowKind::Rename,
        RowKind::RenameMultiple,
        RowKind::ExtractHere,
        RowKind::ExtractTo,
        RowKind::CompressZip,
        RowKind::CompressTarGz,
        RowKind::MoveToTrash,
        RowKind::DeletePermanently,
        RowKind::DownloadTorrent,
        RowKind::FolderColor,
        RowKind::Properties,
    ];

    // The Trash row menu. Kept separate from [`Self::ALL`] so the virtual
    // Trash surface cannot inherit ordinary file verbs such as Cut or Open.
    pub(crate) const TRASH_ALL: [RowKind; 2] =
        [RowKind::RestoreFromTrash, RowKind::DeleteTrashPermanently];

    // The row's stable `enz_menu` key, derived from the action so a reorder moves
    // the keyboard cursor with the row.

    // The row's group. Rules go between groups, never inside one.
    pub(crate) fn group(self) -> RowGroup {
        match self {
            RowKind::Open
            | RowKind::OpenWith
            | RowKind::OpenPreview
            | RowKind::MountImage
            | RowKind::RevealParent
            | RowKind::OpenInTerminal => RowGroup::Navigate,
            RowKind::Cut
            | RowKind::Copy
            | RowKind::CopyPath
            | RowKind::CopyTo
            | RowKind::MoveTo
            | RowKind::Duplicate
            | RowKind::CreateLink
            | RowKind::Rename
            | RowKind::RenameMultiple => RowGroup::Clipboard,
            RowKind::ExtractHere
            | RowKind::ExtractTo
            | RowKind::CompressZip
            | RowKind::CompressTarGz => RowGroup::Archive,
            RowKind::MoveToTrash
            | RowKind::DeletePermanently
            | RowKind::RestoreFromTrash
            | RowKind::DeleteTrashPermanently => RowGroup::Destructive,
            RowKind::DownloadTorrent | RowKind::FolderColor | RowKind::Properties => {
                RowGroup::Inspect
            }
            RowKind::PinToSidebar | RowKind::UnpinFromSidebar => RowGroup::Navigate,
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            RowKind::MountImage => "disc",
            RowKind::Open => "folder-open",
            RowKind::RevealParent => "corner-left-up",
            RowKind::OpenInTerminal => "terminal",
            RowKind::Cut => "scissors",
            RowKind::Copy => "copy",
            // Distinct from `copy`, so the path copy is told apart from the file copy.
            RowKind::CopyPath => "clipboard-copy",
            RowKind::CopyTo => "folder-input",
            RowKind::MoveTo => "folder-output",
            RowKind::Duplicate => "copy-plus",
            RowKind::CreateLink => "link",
            RowKind::Rename | RowKind::RenameMultiple => "pencil-line",
            RowKind::OpenWith => "app-window",
            RowKind::Properties => "info",
            RowKind::FolderColor => "palette",
            RowKind::DownloadTorrent => "download",
            RowKind::OpenPreview => "eye",
            RowKind::ExtractHere => "package-open",
            RowKind::ExtractTo => "folder-output",
            RowKind::CompressZip => "archive",
            RowKind::CompressTarGz => "file-archive",
            RowKind::MoveToTrash => "trash-2",
            RowKind::DeletePermanently => "trash",
            RowKind::RestoreFromTrash => "rotate-ccw",
            RowKind::DeleteTrashPermanently => "trash",
            // The same glyph as the sidebar's drop-to-pin target.
            RowKind::PinToSidebar => "bookmark-plus",
            RowKind::UnpinFromSidebar => "bookmark-minus",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            RowKind::MountImage => crate::i18n::tr("Mount"),
            RowKind::Open => crate::i18n::tr("Open"),
            RowKind::RevealParent => crate::i18n::tr("Reveal Parent Folder"),
            RowKind::OpenInTerminal => crate::i18n::tr("Open in Terminal"),
            RowKind::Cut => crate::i18n::tr("Cut"),
            RowKind::Copy => crate::i18n::tr("Copy"),
            RowKind::CopyPath => crate::i18n::tr("Copy Path"),
            RowKind::CopyTo => crate::i18n::tr("Copy to\u{2026}"),
            RowKind::MoveTo => crate::i18n::tr("Move to\u{2026}"),
            RowKind::Duplicate => crate::i18n::tr("Duplicate"),
            RowKind::CreateLink => crate::i18n::tr("Create Link…"),
            RowKind::Rename => crate::i18n::tr("Rename"),
            RowKind::RenameMultiple => crate::i18n::tr("Rename Multiple\u{2026}"),
            RowKind::OpenWith => crate::i18n::tr("Open With\u{2026}"),
            RowKind::Properties => crate::i18n::tr("Properties"),
            RowKind::FolderColor => crate::i18n::tr("Folder Color\u{2026}"),
            RowKind::DownloadTorrent => crate::i18n::tr("Download Torrent"),
            RowKind::OpenPreview => crate::i18n::tr("Preview"),
            RowKind::ExtractHere => crate::i18n::tr("Extract Here"),
            RowKind::ExtractTo => crate::i18n::tr("Extract To\u{2026}"),
            RowKind::CompressZip => crate::i18n::tr("Compress to ZIP"),
            RowKind::CompressTarGz => crate::i18n::tr("Compress to TAR.GZ"),
            RowKind::MoveToTrash => crate::i18n::tr("Move to Trash"),
            RowKind::DeletePermanently => crate::i18n::tr("Delete Permanently\u{2026}"),
            RowKind::RestoreFromTrash => crate::i18n::tr("Restore"),
            RowKind::DeleteTrashPermanently => crate::i18n::tr("Delete Permanently\u{2026}"),
            RowKind::PinToSidebar => crate::i18n::tr("Pin to Sidebar"),
            RowKind::UnpinFromSidebar => crate::i18n::tr("Unpin from Sidebar"),
        }
    }

    // The keyboard shortcut shown on the row: the current chord of its bound
    // action, so a rebind in Settings shows here too.
    pub(crate) fn shortcut(self) -> Option<String> {
        let action = match self {
            RowKind::Cut => "files.cut",
            RowKind::Copy => "files.copy",
            RowKind::CopyTo => "files.copy-to",
            RowKind::MoveTo => "files.move-to",
            RowKind::Rename | RowKind::RenameMultiple => "files.rename",
            RowKind::Properties => "files.properties",
            RowKind::MoveToTrash => "files.move-to-trash",
            RowKind::DeletePermanently => "files.delete-permanently",
            _ => return None,
        };
        crate::keybindings::label(action)
    }

    // Whether the row can act on the right-clicked entry; enabled exactly when the
    // reducer would do something.
    pub(crate) fn enabled(self, model: &Model, path: &Path, is_dir: bool) -> bool {
        match self {
            RowKind::MountImage => {
                !is_dir && crate::storage::is_disk_image(path) && !crate::services::is_virtual(path)
            }
            RowKind::RevealParent => path.parent().is_some(),
            RowKind::OpenInTerminal => is_dir && !crate::services::is_virtual(path),
            RowKind::FolderColor => is_dir && !crate::services::is_virtual(path),
            // Every one of these writes beside or from a real path.
            RowKind::CopyTo | RowKind::MoveTo | RowKind::Duplicate | RowKind::CreateLink => {
                !crate::services::is_virtual(path)
            }
            // The right-clicked row becomes the selection before activation;
            // `menu_model` chooses between the rename rows.
            RowKind::Rename => model.selection.len() <= 1,
            RowKind::RenameMultiple => model.selection.len() > 1,
            RowKind::OpenWith => !is_dir,
            RowKind::DownloadTorrent => {
                is_regular_torrent_entry(model, path, is_dir)
                    && model
                        .features
                        .enabled(crate::features::FeatureId::Downloads)
                    && model.downloads.backend == crate::downloads::Aria2Status::Ready
            }
            // Matches the decoders behind `services::is_previewable_mime`.
            RowKind::OpenPreview => {
                !is_dir && crate::services::is_previewable_mime(preview_mime_of(model, path))
            }
            // Virtual locations have no filesystem target.
            RowKind::MoveToTrash | RowKind::DeletePermanently => {
                !crate::services::is_virtual(&model.cwd)
            }
            RowKind::RestoreFromTrash | RowKind::DeleteTrashPermanently => true,
            // Extraction needs a real archive file; compression needs a
            // real folder to write the archive into.
            RowKind::ExtractHere | RowKind::ExtractTo => {
                !is_dir
                    && crate::archive::ArchiveFormat::detect(path).is_some()
                    && !crate::services::is_virtual(path)
            }
            RowKind::CompressZip | RowKind::CompressTarGz => {
                !crate::services::is_virtual(&model.cwd)
            }
            // Pins point at real folders; virtual locations are excluded.
            RowKind::PinToSidebar => is_dir && !crate::services::is_virtual(path),
            RowKind::UnpinFromSidebar => is_dir && !crate::services::is_virtual(path),
            // Always meaningful: the right-clicked path is a concrete entry.
            RowKind::CopyPath => true,
            RowKind::Open | RowKind::Cut | RowKind::Copy | RowKind::Properties => true,
        }
    }
}
