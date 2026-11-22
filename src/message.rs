// The single message enum driving the elm loop.

use std::path::PathBuf;

use enzui_core::drag::{DropRequest, ItemDropRequest, ItemId};

use crate::bulk_rename::BulkRenameMode;
use crate::conflict::ConflictChoice;
use crate::model::{
    EntryKind, Screen, SearchFilterDate, SearchFilterType, SelectionDirection, SortKey, ViewMode,
};

#[path = "message_parts/message_kind.rs"]
mod message_kind;

#[derive(Clone, Debug)]
pub(crate) enum Msg {
    // Shared XDG theme-file, watcher, and desktop appearance event.
    Theme(Box<enzui_core::theme_file::integration::ThemeEvent>),

    // Row capture owns double-click timing; selection modifiers clear the activation latch.
    RowClickPrimary {
        path: PathBuf,
        kind: EntryKind,
        command: bool,
        shift: bool,
    },

    // A background directory read completed for `path`.
    //
    // `generation` is the [`crate::model::Model::begin_load`] id stamped when
    // this read was dispatched. The reducer applies the reply only while it
    // still equals the model's `load_generation`, so a read superseded by a
    // newer navigation (rapid back and forth to the same folder) is dropped
    // instead of overwriting or clearing the current folder.
    DirLoaded {
        path: PathBuf,
        generation: u64,
        result: Result<Vec<crate::model::FileEntry>, String>,
    },
    // The folder's modification time, checked after its watch was armed.
    ListingVerified {
        path: PathBuf,
        mtime: Option<std::time::SystemTime>,
    },
    // A drag or keyboard drop, validated off the UI thread.
    ItemDropValidated(Box<Result<crate::drag_drop::PlannedDrop, crate::drag_drop::DropRefusal>>),
    // Folder changes gathered for a moment are due for one listing.
    WatchRescanDue {
        generation: u64,
    },
    // A path typed in the location bar was checked off the UI thread.
    TypedPathChecked {
        path: PathBuf,
        is_dir: bool,
    },
    // A navigation's listing is still out after a short delay: show loading.
    ListingSlow {
        generation: u64,
    },
    DirPrepared {
        path: PathBuf,
        generation: u64,
        policy: crate::model::ProjectionPolicy,
        // The folder's modification time just before the read.
        mtime: Option<std::time::SystemTime>,
        result: Result<
            (
                Vec<crate::model::FileEntry>,
                crate::model::FileEntryProvider,
            ),
            String,
        >,
    },
    // The drive list finished enumerating (Computer view).
    DrivesLoaded(Vec<crate::model::Drive>),
    // List viewport moved; used to realise only its visible keyed rows.
    ListScrolled(f32),
    // Grid viewport moved; used to request only the visible thumbnail window.
    GridScrolled(f32),
    // Downloads list viewport moved; only the visible retained-history rows
    // are projected.
    DownloadsScrolled(f32),
    // One off-thread bounded thumbnail request completed.
    ThumbnailLoaded(crate::thumbnail::Outcome),
    // Capture-only boot gate: directory and image previews are ready before
    // the main window is opened, preventing a first-paint race in visual QA.
    CapturePrepared {
        entries: Vec<crate::model::FileEntry>,
        thumbnails: Vec<crate::thumbnail::Outcome>,
    },

    NavigateTo(PathBuf),
    // One enzui-center connector event.
    #[cfg(feature = "center")]
    Center(enzui_agent::center::ConnectorEvent),
    // A desktop launch reached this instance through the single-instance
    // bridge (`org.freedesktop.Application`).
    #[cfg(feature = "center")]
    ApplicationWake,
    // A task started before the window opened could not run.
    #[cfg(feature = "center")]
    BackgroundTaskRefused(String),

    NavigateScreen(Screen, PathBuf),
    GoBack,
    GoForward,
    GoUp,
    OpenEntry(PathBuf, crate::model::EntryKind),
    // A bounded desktop command finished.
    //
    // This covers explicit Open With commands and the native GIO fallback
    // used when OpenURI is unavailable. Success is silent; failure is shown.
    OpenLaunched {
        generation: u64,
        result: crate::launch::LaunchResult,
    },
    // The Open With launch finished; an error is shown.
    OpenWithLaunched {
        generation: u64,
        result: crate::launch::LaunchResult,
    },

    // Move the selection to the freedesktop Trash (`Delete`, or the context
    // menu's Move to Trash), on a worker through
    // [`crate::trash::trash_batch_with`].
    TrashSelection,
    // The trash batch finished.
    TrashFinished {
        activity: crate::activity::FileActivity,
        outcome: crate::trash::TrashOutcome,
    },
    // The Trash screen's listing loaded; a reply for an older generation is
    // dropped. The listing is bounded and may report itself incomplete.
    TrashLoaded {
        generation: u64,
        listing: crate::trash::TrashListing,
    },
    // Select a Trash row without performing a destructive action. Modifier
    // state is forwarded by the row so Ctrl/Cmd toggles and Shift extends in
    // Trash's own display order.
    SelectTrashEntry {
        path: PathBuf,
        command: bool,
        shift: bool,
    },
    // Restore the named payload from `Trash/files` to its `.trashinfo` path.
    RestoreFromTrash(PathBuf),
    // The off-thread restore planning pass completed. A free destination is
    // restored directly; an occupied one enters the existing conflict queue.
    TrashRestorePlanned {
        activity: crate::activity::FileActivity,
        entry: crate::trash::TrashEntry,
        plan: Result<Box<crate::conflict::PastePlan>, String>,
    },
    // A non-conflicting restore (or completed conflict restore) finished.
    TrashRestoreFinished {
        activity: crate::activity::FileActivity,
        entry: crate::trash::TrashEntry,
        result: Result<(), String>,
    },
    // Open the standard permanent-delete confirmation for one Trash row.
    AskDeleteTrashEntry(PathBuf),
    // A Trash row's context-menu command fired.
    TrashContextRowActivated(PathBuf, crate::context_menu::RowKind),
    // Ask to empty the whole Trash; nothing is deleted until the person
    // confirms.
    EmptyTrash,
    // Emptying the Trash finished; the listing reloads.
    TrashEmptied {
        activity: crate::activity::FileActivity,
        outcome: crate::trash::PermanentDeleteOutcome,
    },
    // Ask to permanently delete the selection (`Shift+Delete`, or the
    // context menu). Opens the confirmation; the deletion itself is
    // [`Self::ConfirmPermanentDelete`].
    AskPermanentDelete,
    ConfirmPermanentDelete,
    CancelPermanentDelete,
    // The permanent-delete confirmation's events: its default action
    // deletes, every other exit cancels.
    DeleteConfirmEvent(enzui_widgets::confirm_dialog::ConfirmDialogEvent),
    // The permanent-delete batch finished.
    PermanentDeleteFinished {
        activity: crate::activity::FileActivity,
        outcome: crate::trash::PermanentDeleteOutcome,
    },

    // Create a folder with an unused name in the current directory, on a
    // worker; the result arrives as [`Msg::CreationFinished`].
    NewFolder,
    // Create an empty file with an unused name in the current directory.
    NewFile,
    // A create finished: the new entry's path, or the error to show.
    CreationFinished(Result<std::path::PathBuf, String>),

    SetViewMode(ViewMode),
    SortBy(SortKey),
    // Set the sort key from View Settings; unlike `SortBy`, choosing the
    // active key keeps the direction.
    SetSortKey(SortKey),
    // Set the sort direction (View Settings "Sort Order" radio): `true`
    // ascending, `false` descending.
    SetSortAscending(bool),
    // Open one View Settings value menu (closing any other).
    OpenSettingsMenu(crate::model::SettingsMenu),
    // Close the open View Settings value menu.
    CloseSettingsMenus,
    ToggleHidden,
    ToggleFoldersFirst,
    ToggleColoredFileIcons,
    ToggleDetails,
    // Toggle whether a plain click opens an entry instead of selecting it.
    ToggleSingleClick,
    // Show or hide one list-view metadata column (Size, Modified, Type).
    ToggleColumn(crate::model::MetaColumn),
    // Toggle one optional metadata field below Grid-view item names.
    ToggleGridInfo(crate::model::MetaColumn),
    // A view-settings save finished. A failure is kept for diagnostics and
    // the next change retries it.
    ViewSettingsSaved(Result<(), enzui_core::persist::PersistError>),
    // A view-settings load settled.
    //
    // [`enzui_core::persist::LoadCompletion`] is not `Clone`, so it
    // travels in a take-once handoff: every clone of the message shares the
    // slot, and a take that finds it empty means a clone already consumed
    // it. Only [`enzui_core::persist::LoadController::settle`]
    // publishes the outcome; a stale settlement leaves the model untouched.
    ViewSettingsLoaded(std::sync::Arc<crate::view_settings_load::ViewSettingsLoadHandoff>),
    ToggleViewSettings,
    // The View Settings exit animation finished. Only the carried
    // presentation is retired, so reopening mid-exit is not torn down by the
    // earlier close.
    ViewSettingsExitSettled(enzui_widgets::modal_host::ModalPresentationKey),
    // Open or close the titlebar's overflow menu. The only message that
    // inverts the flag; every close uses [`Msg::CloseToolbarOverflow`].
    ToggleToolbarOverflow,
    // Close the titlebar's overflow menu, idempotently. The framework sends
    // it in the same batch as a row's command and on Escape or an outside
    // press, so a toggle here would reopen the menu
    // ([`enzui_widgets::title_bar::EnzTitleBar::action_dismiss`]).
    CloseToolbarOverflow,

    // Open the Properties dialog on `path`; for a folder, its total size is
    // counted on a worker.
    ShowProperties(PathBuf),
    PropertiesShortcut,

    // Expand / collapse the Properties dialog's Permissions section.
    TogglePermissionsSection(bool),
    // Flip one permission: `row` is owner, group, other (0–2); `col` is
    // read, write, execute (0–2).
    TogglePermissionBit {
        row: usize,
        col: usize,
    },
    // Apply the permission grid, then re-read it so the dialog shows what
    // the filesystem accepted.
    ApplyPermissions,
    // The background recursive-size walk finished for `path`.
    PropertiesRecursiveSizeReady {
        path: PathBuf,
        total: u64,
        count: usize,
    },

    // Open the bulk-rename dialog on the selection; nothing happens with an
    // empty selection.
    BeginBulkRename,
    CancelBulkRename,
    SetBulkRenameMode(BulkRenameMode),
    // The single-file `Rename to:` field, kept apart from the rule fields so
    // the two dialogs never share one.
    BulkRenameDirectChanged(String),
    // Removable media were enumerated.
    RemovableDevicesLoaded(Vec<crate::devices::RemovableDevice>),
    // The person chose an operation for the device with this id.
    DeviceAction(String, crate::devices::DeviceOp),
    // The pending Eject or Safely Remove was confirmed.
    ConfirmDeviceAction,
    // The pending Eject or Safely Remove was declined.
    CancelDeviceAction,
    // A device operation settled on its worker.
    DeviceActionFinished {
        id: String,
        op: crate::devices::DeviceOp,
        activity: crate::activity::FileActivity,
        result: Result<Option<PathBuf>, crate::devices::DeviceError>,
    },

    StorageLoaded(Result<crate::storage::Inventory, String>),
    MountDiskImage(PathBuf),
    OpenStorage(String),
    UnmountStorage(String),
    StorageOperationFinished {
        id: String,
        name: String,
        opening: bool,
        activity: crate::activity::FileActivity,
        result: Result<Option<PathBuf>, String>,
    },
    // A mount service operation on the storage location `id` settled; its
    // password, host-key and passphrase errors open the matching prompt.
    MountServiceFinished {
        id: String,
        name: String,
        opening: bool,
        activity: crate::activity::FileActivity,
        result: Result<Option<PathBuf>, crate::mount_service::ServiceError>,
        // The choices of a connection, for the prompts that retry it.
        plan: Option<crate::mount_service::ConnectPlan>,
        // Mounting at startup could not be set up after a successful mount.
        boot: Option<crate::mount_service::ServiceError>,
    },
    // Right-click on a mount service network location.
    OpenLocationContextMenu(String, iced::Point),
    // The ⋯ button of a mount service location: its context menu, anchored
    // at the button.
    ShowLocationMenu(String),
    // A press on a Computer or Network row: the first selects it, a second
    // within the double-click interval (or the first, with single-click
    // opening) sends `open`.
    DeviceRowPressed {
        key: String,
        open: Box<Msg>,
    },
    // A command for the mount service location `id`.
    LocationAction(String, crate::mount_service::LocationAction),
    // A change to a saved network location settled.
    LocationUpdated {
        id: String,
        name: String,
        action: crate::mount_service::LocationAction,
        result: Result<(), crate::mount_service::ServiceError>,
    },
    LocationRenameChanged(String),
    LocationShareChosen(usize),
    ConfirmLocationDialog,
    CloseLocationDialog,
    // The locked-drive passphrase field changed.
    UnlockPassphraseChanged(enzui_core::form::SecretText),
    ToggleUnlockRevealed,
    // Unlock the drive with the typed passphrase.
    ConfirmUnlock,
    CloseUnlock,
    // The Format dialog's volume name, file system and whole-drive choice.
    FormatLabelChanged(String),
    FormatFileSystemChosen(crate::mount_service::FileSystem),
    SetFormatWholeDrive(bool),
    // Format the volume as chosen.
    ConfirmFormat,
    CloseFormat,
    // Formatting the volume `id` settled; success carries the service id of
    // the formatted volume.
    FormatFinished {
        id: String,
        name: String,
        activity: crate::activity::FileActivity,
        result: Result<String, crate::mount_service::ServiceError>,
    },
    // Mount and open the volume the Format dialog just made.
    OpenFormatted,
    // Open the Add a Cloud Drive dialog.
    ShowAddCloud,
    CloudProviderChosen(crate::mount_service::CloudProvider),
    CloudServerChanged(String),
    // Sign in to the Nextcloud server typed so far.
    ConfirmCloudServer,
    // Back from the server address to the list of services.
    CloudBack,
    // The service started (or refused) a cloud sign-in.
    CloudAdded {
        provider: crate::mount_service::CloudProvider,
        result: Result<String, crate::mount_service::ServiceError>,
    },
    // Stop the browser sign-in the dialog follows.
    CancelCloudSignIn,
    // Open the cloud drive the dialog just added.
    OpenCloudDrive,
    CloseCloud,
    // The phones GIO knows about were listed.
    PhonesLoaded(Vec<crate::devices::phones::Phone>),
    // The person asked to open the phone with this id.
    OpenPhone(String),
    // Opening a phone settled.
    PhoneOpened {
        id: String,
        activity: crate::activity::FileActivity,
        result: Result<PathBuf, crate::devices::phones::PhoneError>,
    },
    // The in-place rename field changed.
    InlineRenameChanged(String),
    // Enter in the in-place rename field.
    CommitInlineRename,
    // The in-place field lost focus to a click elsewhere: commit, as Enter.
    InlineRenameBlurred,
    // Escape in the in-place rename field.
    CancelInlineRename,
    BulkRenameFindChanged(String),
    BulkRenameReplaceChanged(String),
    BulkRenameTemplateChanged(String),
    // The `Numbering` start value. `0` is honoured literally.
    BulkRenameStartChanged(u32),
    BulkRenameStepChanged(u32),
    BulkRenamePadChanged(u32),
    // Restore the rule fields, keeping the files being renamed.
    ResetBulkRename,
    // Apply the renames the preview shows, on a worker.
    ApplyBulkRename,
    // The rename batch finished. The activity center refuses a reply for a
    // superseded attempt.
    BulkRenameFinished {
        activity: crate::activity::FileActivity,
        outcome: crate::services::BulkRenameOutcome,
    },

    // Cut the selection, so the next Paste moves it.
    CutSelection,
    // Copy the current selection. Paste duplicates files/directories and keeps
    // this clipboard available for another destination.
    CopySelection,
    // Route Ctrl+V according to the active Files screen.
    PasteShortcut,
    // Text read from the system clipboard for Download Manager.
    DownloadLinkPasted(Result<std::sync::Arc<String>, iced::clipboard::Error>),
    // Paste the clipboard into the current directory.
    PasteHere,
    FileClipboardFinished(crate::file_clipboard::Receipt),
    CopyPreviewSelection(String),
    // Rows dragged onto a folder or the open folder were dropped. The drag
    // host's [`ItemDropRequest`] already settled move or copy and the
    // destination; the paste runs through `BeginPaste` like the keyboard
    // verbs.
    FileItemDropped(ItemDropRequest),
    // Move the selection into `dest`, validated exactly as a pointer drop.
    MoveSelectionToFolder(PathBuf),
    // Copy the selection into `dest`, validated exactly as a pointer drop.
    CopySelectionToFolder(PathBuf),
    // Extract `archive` into `dest_dir`, or beside the archive when `None`,
    // on the archive worker.
    ExtractArchive {
        archive: PathBuf,
        dest_dir: Option<PathBuf>,
    },
    // Choose a folder with the portal chooser, then extract `archive` into
    // it through [`Self::ExtractArchive`].
    BeginExtractArchiveTo(PathBuf),
    // Compress the selection into the current folder.
    CompressSelection(crate::archive::ArchiveFormat),
    // Measure a folder's total size (the list's size cell, the inspector).
    MeasureFolder(PathBuf),
    // Stop measuring a folder.
    CancelFolderMeasure(PathBuf),
    // A report from a folder measurement.
    FolderMeasure {
        path: PathBuf,
        generation: u64,
        event: crate::folder_size::MeasureEvent,
    },
    // Extract an explicit Files activation without resolving its MIME default again.
    OpenArchiveExternally(PathBuf),
    ArchiveHandlerResolved {
        generation: u64,
        path: PathBuf,
        result: Result<Option<String>, String>,
    },
    // An archive operation settled on its worker.
    ArchiveSettled {
        activity: crate::activity::FileActivity,
        verb: crate::archive::ArchiveVerb,
        open_output: bool,
        result: crate::archive::ArchiveResult,
    },
    // Move to… (`Ctrl+Shift+M`): pick the destination with the portal
    // folder chooser, then [`Self::MoveSelectionToFolder`].
    BeginMoveSelectionToFolder,
    // Copy to… (`Ctrl+Shift+G`): as [`Self::BeginMoveSelectionToFolder`],
    // copying.
    BeginCopySelectionToFolder,
    // Copy the row's absolute path as text. Unlike [`Self::CopySelection`]
    // it leaves the file clipboard alone.
    CopyRowPath(PathBuf),

    // Paste `sources` into `dest_dir`; planning runs on a worker and arrives
    // as `PastePlanned`.
    BeginPaste {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        copy: bool,
    },
    // The paste plan is ready: the free destinations as well as the
    // conflicts, since a paste where one file collides still moves the rest.
    PastePlanned {
        activity: crate::activity::FileActivity,
        dest_dir: PathBuf,
        copy: bool,
        plan: Box<crate::conflict::PastePlan>,
    },
    // The paste's non-conflicting moves or copies finished on the worker and
    // the conflict queue is built; what remains is the conflict prompt or
    // the summary.
    PasteExecuted {
        activity: crate::activity::FileActivity,
        queue: Box<crate::conflict::ConflictQueue>,
    },
    // Resolve the conflict on screen.
    ResolveConflict(ConflictChoice),
    SetApplyToAllConflicts(bool),
    PauseConflicts,
    ResumeConflicts,
    // Abandon the rest of the batch.
    CancelConflicts,
    // A batch of conflict decisions finished off-thread.
    ConflictBatchFinished {
        activity: crate::activity::FileActivity,
        queue: Box<crate::conflict::ConflictQueue>,
    },

    // Open the Open With dialog for `path` and query its applications on a
    // worker.
    ShowOpenWith(PathBuf),
    // The two independent MIME queries completed. `mime` guards against a
    // stale reply populating a newer dialog.
    OpenWithQueried {
        generation: u64,
        mime: String,
        outcome: crate::mime_query::MimeQueryOutcome,
    },
    CloseOpenWith,
    // Select the application at this index in the chooser.
    SelectOpenCandidate(usize),
    SetOpenBehavior(crate::open_with::OpenBehavior),
    // The "Or type a command" field changed.
    OpenWithCommandChanged(String),
    // Launch the chosen command on a worker and close the dialog; the result
    // arrives as `OpenWithLaunched`. With "Always this app" the choice also
    // becomes the type's default, for the session and on disk.
    ConfirmOpenWith,
    // Saving the default application finished. A failure is shown; the
    // session keeps the choice regardless.
    MimeDefaultPersisted {
        generation: u64,
        result: Result<(), String>,
    },

    ShowAttach,
    CloseAttach,
    SetAttachProtocol(crate::network::NetworkProtocol),
    AttachServerChanged(String),
    AttachShareChanged(String),
    AttachDomainChanged(String),
    AttachUsernameChanged(String),
    // The password field changed. The password is never logged.
    AttachPasswordChanged(String),
    ToggleAttachPasswordRevealed,
    // Keep the attach password in the keyring.
    SetAttachRememberPassword(bool),
    // Mount service connections: reconnect at login, mount at startup.
    SetAttachAutoConnect(bool),
    SetAttachMountAtBoot(bool),
    // Scan for SMB shares on a worker; the result arrives as
    // `AttachDiscovered`.
    DiscoverSmb,
    // The share scan finished: the shares found, or why it could not run.
    AttachDiscovered(crate::network::ScanResult),
    // Fill the attach form from the discovered share at this index.
    AdoptDiscovered(usize),
    // Mount the composed address on a worker; the result arrives as
    // `AttachMounted`.
    ConfirmAttach,
    // The mount finished: the mounted URI, or the error to show.
    AttachMounted(crate::network::MountResult),
    // A mount asked a question only the person may answer, such as whether
    // to trust an SFTP server's unknown host key.
    MountQuestionAsked(crate::network::AskedQuestion),
    // The user answered (or dismissed) the open mount question.
    MountQuestionAnswered(crate::network::MountAnswer),
    // Scan from the Network tab (separate from the attach dialog's scan).
    DiscoverNetwork,
    NetworkDiscovered(crate::network::ScanResult),
    MountDiscovered(usize),
    CancelNetworkOperation,

    ToggleSearch,
    SearchChanged(String),
    // Search subfolders too (`true`, starting the background walk) or only
    // the loaded folder.
    SetSearchScope(bool),
    // A recursive search walk finished. The generation tells repeated walks
    // of the same root and query apart.
    SearchResultsReady {
        generation: u64,
        root: PathBuf,
        query: String,
        hits: Vec<crate::model::FileEntry>,
    },
    ToggleFilters,
    // Synchronize the framework-owned anchored search surface state.
    SearchFiltersOpenChanged(bool),
    // Set the search type facet (Filters "Any type" radio).
    SetFilterType(SearchFilterType),
    // Set the search date facet (Filters "Any date" radio).
    SetFilterDate(SearchFilterDate),
    BeginPathEdit,
    InspectorPropertiesReady {
        path: PathBuf,
        info: Option<crate::model::PropertiesInfo>,
    },
    PathDraftChanged(String),
    PathSuggestionsReady {
        query: String,
        paths: Vec<PathBuf>,
    },
    PathSuggestionAccepted(PathBuf),
    CommitPathEdit,
    // Explicit Escape cancellation; returns focus to the idle breadcrumb.
    CancelPathEdit,
    // Outside press or focus transfer; closes without stealing destination focus.
    DismissPathEdit,
    // The path bar's Copy button: copy the open folder's path as text.
    CopyCwdPath,

    // A secondary press on a row: open its menu at the window-space `point`.
    OpenContextMenu(PathBuf, iced::Point),
    // A secondary press on a user-pinned Places row. This is separate from an
    // entry menu because the pinned folder need not be visible in the current
    // directory listing.
    OpenSidebarContextMenu(PathBuf, iced::Point),
    // A secondary press on listing space not occupied by a row or tile.
    OpenBackgroundContextMenu(iced::Point),
    // Close the context menu without acting (outside press or Escape).

    // A context-menu row fired. It acts on the right-clicked entry, not on
    // the selection.
    ContextRowActivated(
        PathBuf,
        crate::model::EntryKind,
        crate::context_menu::RowKind,
    ),
    // A command fired from the listing background menu.
    BackgroundContextRowActivated(crate::context_menu::BackgroundRowKind),
    // Every event the context-menu host reports: invocation, dismissal,
    // navigation, highlight and activation. Pointer and Escape dismissals
    // arrive alike, so they restore focus alike.
    ContextMenuEvent(u64, enzui_widgets::context_menu_host::ContextMenuEvent),

    SelectOnly(PathBuf),
    // Space on a focused row: select it and preview it, as Nautilus does.
    SpaceOnRow(PathBuf),
    ToggleSelect(PathBuf),
    SelectRange(PathBuf),
    // Select every visible entry (`Ctrl+A`).
    SelectAll,
    ClearSelection,
    // Arrow-key movement through the current file view. Shift extends range.
    MoveSelection {
        direction: SelectionDirection,
        extend: bool,
    },
    // A printable character typed while no text field has focus; starts
    // (or extends) a search of the open folder.
    TypeAhead(char),
    // Duplicate the selection beside itself.
    DuplicateSelection,
    // Create a link to each selected entry beside it.
    LinkSelection,
    // Read the desktop file clipboard and link its files here.
    PasteAsLink,
    // Create links in `dest_dir` to `sources`.
    LinkSources {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    // A duplicate or link batch finished.
    CreatedBatch {
        verb: CreateVerb,
        created: Vec<PathBuf>,
        failed: Vec<String>,
    },
    // Open a tab at the current location.
    NewTab,
    // Close this tab (the active one for `None`).
    CloseTab(Option<u64>),
    // Make this tab active.
    SwitchTab(u64),
    // The next tab, wrapping.
    NextTab,
    // The previous tab, wrapping.
    PreviousTab,
    // Open or close the list of every tab.
    ToggleTabList,
    // The listed folders' identities, for the folder `dir`.
    FolderIdentitiesScanned {
        dir: PathBuf,
        identities: Vec<(PathBuf, crate::folder_colors::FolderIdentity)>,
    },
    // Open the colour chooser for this folder.
    ShowFolderColor(PathBuf),
    // Close the colour chooser.
    CloseFolderColor,
    // Tag this folder with a colour, or clear it with `None`.
    SetFolderColor(PathBuf, Option<enzui_theme::WorkspaceAccentKey>),
    // The folder's identity was read for a tag change.
    FolderColorIdentified {
        path: PathBuf,
        color: Option<enzui_theme::WorkspaceAccentKey>,
        identity: Result<crate::folder_colors::FolderIdentity, String>,
    },
    // A Templates folder scan finished.
    TemplatesScanned(crate::templates::Templates),
    // Create a document in the current folder from this template.
    NewFromTemplate(PathBuf),
    // Open a terminal in this folder, or the current one for `None`.
    OpenInTerminal(Option<PathBuf>),
    // The open folder's repository and project tools.
    Project(crate::project::ProjectMsg),
    History(crate::history::HistoryMsg),
    // The terminal launch finished its settle window.
    TerminalLaunched(Result<(), String>),
    // The Settings terminal command field changed.
    SetTerminalCommand(String),
    // Step the current view's zoom level (Ctrl+plus/minus/0 or
    // Ctrl+wheel over the file view).
    Zoom(enzui_core::zoom_levels::ZoomStep),
    // Deferred post-layout pass after resize; refreshes thumbnail scheduling
    // from the geometry published by the newly rendered grid.
    GridLayoutSettled,

    // Open the "Select items matching…" glob dialog.
    OpenSelectMatching,
    // Close it without changing the selection.
    CloseSelectMatching,
    SelectMatchingDraftChanged(String),
    // Replace the selection with the glob's matches and close.
    SelectMatchingSubmit,

    // The marquee's final selection, with the press-time modifier already
    // applied.
    MarqueeResolved(Vec<ItemId>),
    // The marquee's live selection while the band is dragged; the final
    // selection replaces it.
    MarqueePreview(Vec<ItemId>),

    // Hide the inline sidebar on a wide window, or open the drawer on a
    // narrow one (`NavSplitState::toggle_for_presentation`).
    ToggleSidebar,
    SidebarState(enzui_widgets::nav_split::NavSplitState),

    Acceptance(enzui_widgets::app_shell::acceptance_probe::Event),
    // Tab: move keyboard focus to the next focusable, wrapping.
    FocusNext,
    // Shift+Tab: move focus backward.
    FocusPrevious,
    // Clear keyboard focus after an unhandled primary press on page content.
    ClearFocus(iced::window::Id),

    // A window was resized. Only the main window's size drives the
    // responsive layout; every window's full size is kept so it reopens at
    // that size.
    WindowResized(iced::window::Id, iced::Size),
    // The main window opened. It becomes the window ledger's primary, so
    // closing it closes the previews too.
    MainWindowOpened(iced::window::Id),
    ActivityWindowTagged(iced::window::Id, String, bool),
    // Off-thread preview classification/read completed; the window may now be
    // opened without blocking the reducer on a file or network mount.
    PreviewPrepared(Box<crate::preview::Preview>),
    PreviewMediaWake(iced::window::Id),
    PreviewMediaFrame(iced::window::Id, Box<enzui_widgets::video::ReadyFrame>),
    PreviewMediaControl(
        iced::window::Id,
        enzui_widgets::media_controls::MediaControlsMessage,
    ),
    PreviewPdfPage(iced::window::Id, u32),
    PreviewPdfLoaded(
        iced::window::Id,
        u32,
        Result<crate::preview_pdf::Page, String>,
    ),
    // A press on a preview's picture or name strip: move that window.
    PreviewWindow(
        iced::window::Id,
        enzui_widgets::title_bar_gesture::WindowCommand,
    ),
    // A preview window opened; its content is attached now that the id
    // exists.
    PreviewWindowOpened(iced::window::Id, Box<crate::preview::Preview>),
    // A window closed. The window ledger decides whether the process exits,
    // since a daemon outlives its last window.
    WindowClosed(iced::window::Id),
    // Open a detached preview window for one file.
    OpenPreview(std::path::PathBuf),
    // Space in a window: preview the selected file, or close the preview
    // window it was pressed in.
    PreviewSelection(iced::window::Id),

    // A titlebar gesture (close, move, maximize, minimize, system menu), run
    // through `WindowCommand::into_task`.
    Window(enzui_widgets::title_bar_gesture::WindowCommand),

    // A host event from the shared generic-dialog contract. Dismissal closes
    // only the active presentation; Tab/Shift+Tab are routed to its scoped
    // focus operation and never interpreted as close.
    DialogEvent(enzui_widgets::modal_host::ModalHostEvent),
    // The View Settings navigation chose a page, by the id that
    // [`crate::model::SettingsSection::from_id`] maps back.
    SelectSettingsPage(String),
    // Back from a settings page to the page list (narrow windows).
    SettingsStackedBack,

    ToggleFeature(crate::features::FeatureId),
    BeginInstallExtension,
    ReviewExtension(PathBuf),
    ExtensionReviewed(Result<std::sync::Arc<crate::extensions::Prepared>, String>),
    CancelExtensionReview,
    ConfirmInstallExtension,
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "production loads the extension catalog asynchronously; fixtures inject it directly"
        )
    )]
    ExtensionsLoaded(crate::extensions::Catalog),
    ChangeExtension(crate::extensions::Change),
    ExtensionsChanged(Result<crate::extensions::Catalog, String>),
    ExtensionDetails(String),
    // Open (`true`) or leave the Downloads settings subpage of Extensions.
    ShowDownloadsSettings(bool),
    // A report from the editable Shortcuts page.
    Shortcut(enzui_widgets::editable_shortcuts::EditableShortcutsEvent),
    ShortcutsSaved(Result<(), enzui_core::persist::PersistError>),
    ExtensionSettingEdited {
        extension: String,
        key: String,
        value: enzui_services::extension_package::DraftValue,
    },
    SaveExtensionSettings(String),
    AskRemoveExtension(String),
    ConfirmRemoveExtension,
    RunExtension {
        extension: String,
        action: String,
        path: PathBuf,
        background: bool,
    },
    ExtensionFinished(Result<crate::extensions::Response, String>),
    CancelExtensionAction,
    TorrentDownloadLimitChanged(String),
    TorrentUploadLimitChanged(String),
    DownloadConcurrencyChanged(String),
    ApplyDownloadSettings,
    // Re-attempt loading the refused download settings from the retained
    // authority, keeping the current one until reacquisition succeeds.
    RetryDownloadSettings,
    // Replace the refused download settings with canonical bounded defaults.
    // The refused source is overwritten only by this deliberate user action.
    RetireDownloadSettings,

    // The `aria2c` probe returned.
    Aria2Probed(crate::downloads::Aria2cProbeResult),
    ShowAddDownload,
    // Close the Add-download dialog without adding anything.
    CloseAddDownload,
    AddDownloadUrlChanged(String),
    ToggleDownloadAdvanced,
    AddDownloadLimitChanged(String),
    // Validate the typed URI and open the native Save dialog. Queue admission
    // waits for its portal grant.
    EnqueueDownload,
    // Add a local `.torrent` file to the Downloads module.
    EnqueueTorrent(PathBuf),
    // Pause, Resume, Cancel or Retry on one download, from the downloads
    // list or the activity surface.
    DownloadAction(
        enzui_core::download::DownloadId,
        enzui_core::download::DownloadAction,
    ),
    // One event from a running transfer, applied by
    // `DownloadQueue::apply_event`.
    TransferEvent(enzui_core::download::DownloadEvent),
    // Live bounded status parsed from aria2's console stream.
    TorrentTelemetry(crate::downloads::TorrentTelemetry),
    // A transfer thread ended. A late worker from an earlier attempt is
    // rejected; the typed outcome lets the panel show a missing backend
    // differently from a network error.
    TransferOutcome(
        enzui_core::download::DownloadId,
        enzui_core::download::Attempt,
        enzui_core::download::transfer::TransferOutcome,
    ),
    // Settlement of a bounded paused-download cleanup.
    PausedDownloadCleaned(crate::downloads::CleanupSettlement),
    CloseRequested(iced::window::Id),
    CleanupDrainComplete,
    // Publication reconciliation completed.
    PublicationReconciled(crate::downloads::PublicationSettlement),
    // The close-helper shutdown thread completed.
    ShutdownHelperDone(crate::downloads::ShutdownOutcome),
    // Coalesced retry tick for outstanding cleanup debts.
    CleanupRetryTick,
    // A control from the shared activity surface.
    ActivityControl(
        enzui_core::operation_activity::ActivityKey,
        enzui_core::operation_activity::ActivityControl,
    ),
    // The one armed terminal-linger timer fired.
    ActivityPrune,
    // The activity surface's `+N more` was activated. Hidden downloads alone
    // open the Downloads panel; anything else opens the all-activity dialog.
    ShowActivityOverflow(enzui_core::operation_activity::ActivityOverflowTarget),
    // Close the all-activity dialog.
    CloseAllActivity,

    // Unmount the filesystem at this mount point. The Computer view's per-row
    // "Unmount" action; only offered for rows that are actually unmountable.
    UnmountDrive(PathBuf),
    // The unmount finished: the unmounted path, or the error to show.
    DriveUnmounted {
        generation: u64,
        activity: crate::activity::FileActivity,
        result: Result<PathBuf, String>,
    },

    // An OS file drag is hovering the window (`window::Event::FileHovered`).
    PinDragHover(PathBuf),
    // The drag left without dropping (`window::Event::FilesHoveredLeft`).
    PinDragLeave,
    // A file was dropped (`window::Event::FileDropped`) — pin it.
    PinDrop(PathBuf),
    // A drag or Ctrl+Arrow reorder completed inside the pinned Places rail.
    ReorderSidebarPins(DropRequest),

    // Pin a folder to the sidebar from its context menu.
    PinToSidebar(PathBuf),
    // Remove the sidebar pin for `path`, from the folder's or the pin's own
    // context menu.
    UnpinFromSidebar(PathBuf),

    // Re-attempt loading the refused pins file from disk.
    RetryPins,
    // Replace the refused pins file with the current defaults, after
    // explicit user confirmation. The refused original is overwritten
    // only by this deliberate action.
    RetirePins,
    // Re-attempt loading the refused feature settings.
    RetryFeatureSettings,
    // Replace the feature-state file with canonical defaults. The refused
    // source is overwritten only by this deliberate user action.
    ReplaceFeatureSettings,
    // One event-driven wake that pumps all live portal consumers: folder and
    // download choosers plus default-application OpenURI requests. No clock
    // or compositor-frame polling is involved.
    PortalTick,
    DesktopLaunchWake,
    NativeFilesDropped {
        drop: enzui_widgets::file_drop::FileDrop,
        target: (PathBuf, u64),
    },
    NativeDropReceived(crate::native_drop::Delivery),
    NativeContentDropped {
        drop: enzui_widgets::file_drop::DataDrop,
        target: (std::path::PathBuf, u64),
    },
    NativeContentSaved(crate::native_drop::ContentDelivery),
    StackDialog(enzui_widgets::modal_host::DialogEvent<crate::ActiveGenericDialog>),
    DesktopLaunchActivated,
    PickerParentReady(crate::picker::PickerRequest, crate::picker::ParentReply),
    PickerFallback(enzui_widgets::file_chooser::FallbackEvent),
    DownloadDestinationFallback(enzui_widgets::file_chooser::FallbackEvent),
    // A portal chooser finished for this target. Cancelling is silent; every
    // failure is shown.
    PortalOutcome(
        crate::picker::PickerRequest,
        enzui_widgets::file_chooser::PortalOutcome<enzui_widgets::file_chooser::FileResult>,
    ),

    // Open the Keyboard Shortcuts dialog (`F1`).
    OpenShortcuts,
    CloseShortcuts,

    // Reload the current directory listing (`F5`).
    Refresh,

    // Directory-change deliveries from the framework subscription.
    TreeWatch(Vec<enzui_core::file_tree_watch::WatchDelivery>),

    // Completion from the blocking watch owner. Only the latest generation
    // can publish its [`enzui_core::file_tree_watch::WatchId`].
    WatchArmed {
        generation: u64,
        outcome: crate::WatchInstallOutcome,
    },

    // The toast host's lifecycle events: expiry, dismissal, hover pause and
    // resume.
    Toast(enzui_widgets::toast_host::ToastEvent),

    A11yWake(enzui_core::app_runtime::AccessibilityWake),

    // A keyboard command whose dispatch respects the current modal owner.
    ShellShortcut(Box<Msg>),

    // Undo and Redo replies from an earlier attempt are dropped.
    UndoLastOperation,
    RedoLastUndone,
    UndoOperationFinished {
        attempt: crate::journal::AttemptId,
        entry: Box<crate::journal::JournalEntry>,
        activity: crate::activity::FileActivity,
        result: crate::journal::InverseResult,
    },
    RedoOperationFinished {
        attempt: crate::journal::AttemptId,
        entry: Box<crate::journal::JournalEntry>,
        activity: crate::activity::FileActivity,
        result: crate::journal::InverseResult,
    },
    // From the journal-conflict banner; a stale generation does nothing.
    ResolveJournalConflict {
        entry_id: crate::journal::JournalId,
        entry_generation: u64,
        choice: crate::journal::JournalConflictChoice,
    },

    Noop,
}

// What a [`Msg::CreatedBatch`] made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateVerb {
    Duplicate,
    Link,
}
