// Event classifiers and small filesystem helpers.

use super::*;

pub(crate) fn classify_event_with_status(
    event: &iced::Event,
    status: iced::event::Status,
    window: iced::window::Id,
) -> Option<Msg> {
    if let Some(traversal) = enzui_core::focus::traversal_from_event(event, status) {
        return Some(match traversal {
            Traversal::Next => Msg::FocusNext,
            Traversal::Previous => Msg::FocusPrevious,
        });
    }

    match event {
        // Mapped to `Noop` rather than dropped; see `subscription`.
        iced::Event::Window(iced::window::Event::Opened { .. }) => Some(Msg::Noop),
        // Carries the window id so the reducer can tell preview resizes apart.
        iced::Event::Window(iced::window::Event::Resized(size)) => {
            Some(Msg::WindowResized(window, *size))
        }
        // Drag-to-pin: hover arms the sidebar drop state, a drop pins, and
        // leaving clears it. These are the only drop events iced exposes.
        iced::Event::Window(iced::window::Event::FileHovered(path)) => {
            Some(Msg::PinDragHover(path.clone()))
        }
        iced::Event::Window(iced::window::Event::FileDropped(path)) => {
            Some(Msg::PinDrop(path.clone()))
        }
        iced::Event::Window(iced::window::Event::FilesHoveredLeft) => Some(Msg::PinDragLeave),
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
            if status == iced::event::Status::Ignored =>
        {
            Some(Msg::ClearFocus(window))
        }
        // No app-wide Escape arm: the context-menu host claims Escape only
        // while presenting and reports it through the same funnel as an outside
        // press, and dialogs mounted above it receive Escape untouched.
        //
        // F1 is a navigation key like Tab, so it stays out of `keybindings`.
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(key),
            modifiers,
            repeat: false,
            ..
        }) if status == iced::event::Status::Ignored
            && !modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo()
            && matches!(
                key,
                iced::keyboard::key::Named::ArrowLeft
                    | iced::keyboard::key::Named::ArrowRight
                    | iced::keyboard::key::Named::ArrowUp
                    | iced::keyboard::key::Named::ArrowDown
            ) =>
        {
            let direction = match key {
                iced::keyboard::key::Named::ArrowLeft => model::SelectionDirection::Left,
                iced::keyboard::key::Named::ArrowRight => model::SelectionDirection::Right,
                iced::keyboard::key::Named::ArrowUp => model::SelectionDirection::Up,
                iced::keyboard::key::Named::ArrowDown => model::SelectionDirection::Down,
                _ => unreachable!(),
            };
            Some(Msg::MoveSelection {
                direction,
                extend: modifiers.shift(),
            })
        }
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::F1),
            ..
        }) => Some(Msg::OpenShortcuts),
        // Command shortcuts, resolved against the single binding table.
        iced::Event::Keyboard(ke @ iced::keyboard::Event::KeyPressed { .. })
            if status == iced::event::Status::Ignored =>
        {
            keybindings::dispatch(ke)
                .map(|action| {
                    Msg::ShellShortcut(Box::new(match action {
                        keybindings::FilesAction::SelectMatching => Msg::OpenSelectMatching,
                        keybindings::FilesAction::Copy => Msg::CopySelection,
                        keybindings::FilesAction::Cut => Msg::CutSelection,
                        keybindings::FilesAction::Paste => Msg::PasteShortcut,
                        keybindings::FilesAction::MoveTo => Msg::BeginMoveSelectionToFolder,
                        keybindings::FilesAction::CopyTo => Msg::BeginCopySelectionToFolder,
                        keybindings::FilesAction::SelectAll => Msg::SelectAll,
                        keybindings::FilesAction::ToggleHidden => Msg::ToggleHidden,
                        keybindings::FilesAction::LocationBar => Msg::BeginPathEdit,
                        keybindings::FilesAction::Search => Msg::ToggleSearch,
                        keybindings::FilesAction::Rename => Msg::BeginBulkRename,
                        keybindings::FilesAction::Properties => Msg::PropertiesShortcut,
                        keybindings::FilesAction::Preview => Msg::PreviewSelection(window),
                        keybindings::FilesAction::Refresh => Msg::Refresh,
                        keybindings::FilesAction::GoBack => Msg::GoBack,
                        keybindings::FilesAction::GoForward => Msg::GoForward,
                        keybindings::FilesAction::GoUp => Msg::GoUp,
                        keybindings::FilesAction::NewFolder => Msg::NewFolder,
                        keybindings::FilesAction::NewFile => Msg::NewFile,
                        keybindings::FilesAction::MoveToTrash => Msg::TrashSelection,
                        keybindings::FilesAction::AskPermanentDelete => Msg::AskPermanentDelete,
                        keybindings::FilesAction::Undo => Msg::UndoLastOperation,
                        keybindings::FilesAction::Redo => Msg::RedoLastUndone,
                        keybindings::FilesAction::ZoomIn => {
                            Msg::Zoom(enzui_core::zoom_levels::ZoomStep::In)
                        }
                        keybindings::FilesAction::ZoomOut => {
                            Msg::Zoom(enzui_core::zoom_levels::ZoomStep::Out)
                        }
                        keybindings::FilesAction::ZoomReset => {
                            Msg::Zoom(enzui_core::zoom_levels::ZoomStep::Reset)
                        }
                        keybindings::FilesAction::NewTab => Msg::NewTab,
                        keybindings::FilesAction::CloseTab => Msg::CloseTab(None),
                        keybindings::FilesAction::NextTab => Msg::NextTab,
                        keybindings::FilesAction::PreviousTab => Msg::PreviousTab,
                    }))
                })
                .or_else(|| {
                    type_ahead_char(ke).map(|ch| Msg::ShellShortcut(Box::new(Msg::TypeAhead(ch))))
                })
        }
        _ => None,
    }
}

// The printable character a key press types, for type-ahead.
//
// Only unclaimed plain or Shift-typed presses count, so Ctrl/Alt/Super chords
// stay shortcuts.
pub(crate) fn type_ahead_char(event: &iced::keyboard::Event) -> Option<char> {
    let iced::keyboard::Event::KeyPressed {
        text: Some(text),
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let mut chars = text.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && !ch.is_control()).then_some(ch)
}

// Visual-QA boot preparation; opens the capture window only after fixture
// thumbnails resolve.
pub(crate) fn prepare_capture(
    path: PathBuf,
    cache_roots: Option<std::sync::Arc<private_data::ThumbnailNestedRoots>>,
) -> impl std::future::Future<
    Output = (
        Result<Vec<model::FileEntry>, String>,
        Vec<thumbnail::Outcome>,
    ),
> + Send
+ 'static {
    crate::shell_work::run(move || {
        crate::icon::initialize();
        let entries = match services::read_directory(&path) {
            Ok(entries) => entries,
            Err(error) => return (Err(error.to_string()), Vec::new()),
        };
        let thumbnails = entries
            .iter()
            .filter(|entry| thumbnail::eligible(&entry.path, &entry.mime, entry.size))
            .filter_map(|entry| {
                thumbnail::stamp(entry.path.clone(), entry.modified).map(|stamp| {
                    thumbnail::Request {
                        stamp,
                        mime: entry.mime.clone(),
                    }
                })
            })
            .map(|request| thumbnail::load(request, cache_roots.clone(), &|| false))
            .collect();
        (Ok(entries), thumbnails)
    })
}

// Read a directory off the render thread.
//
// Also returns the folder's modification time taken just before the read, so
// a later check can tell whether entries were added, removed or renamed
// since this listing.
pub(crate) async fn load_prepared_dir(
    path: PathBuf,
    policy: model::ProjectionPolicy,
) -> (
    Option<std::time::SystemTime>,
    Result<(Vec<model::FileEntry>, model::FileEntryProvider), String>,
) {
    let mut retry_delays = [200, 500, 1000, 2000].into_iter();
    loop {
        let path = path.clone();
        let policy = policy.clone();
        let (mtime, result) =
            crate::shell_work::run_cancellable(crate::shell_work::Pool::Listing, move |_| {
                crate::icon::initialize();
                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                let result = services::read_directory(&path).map(|entries| {
                    let provider = model::FileEntryProvider::build(
                        &entries,
                        model::EntrySource::Directory,
                        &policy,
                        &model::FileEntryProvider::default(),
                    );
                    (entries, provider)
                });
                (mtime, result)
            })
            .await;
        if result.as_ref().is_err_and(retryable_listing_error)
            && let Some(milliseconds) = retry_delays.next()
        {
            crate::shell_work::delay(std::time::Duration::from_millis(milliseconds)).await;
        } else {
            return (mtime, result.map_err(|error| error.to_string()));
        }
    }
}

fn retryable_listing_error(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        error.kind(),
        NotFound
            | Interrupted
            | WouldBlock
            | TimedOut
            | NotConnected
            | ConnectionAborted
            | ConnectionReset
            | HostUnreachable
            | NetworkDown
            | NetworkUnreachable
    ) || matches!(
        error.raw_os_error(),
        Some(libc::EIO | libc::ESTALE | libc::EREMOTEIO)
    )
}

pub(crate) fn load_drives() -> impl std::future::Future<Output = Vec<model::Drive>> + Send + 'static
{
    crate::shell_work::run(services::list_drives)
}

// Lists the home Trash off the render thread with a bounded, nofollow scan.
// Each entry carries the descriptor-bound authority that later restore and
// permanent delete check, rather than a re-derived path.
pub(crate) fn load_trash(
    home_data_dir: PathBuf,
    authority: Option<private_data::TrashRootAuthority>,
) -> impl std::future::Future<Output = crate::trash::TrashListing> + Send + 'static {
    crate::shell_work::run(move || match authority {
        Some(auth) => crate::trash::list_trash_admitted_with_authority(&home_data_dir, auth),
        None => crate::trash::list_trash_admitted(&home_data_dir),
    })
}

// Plans a restore off the render thread.
pub(crate) fn plan_trash_restore(
    entry: crate::trash::TrashEntry,
) -> impl std::future::Future<Output = Result<crate::conflict::PastePlan, String>> + Send + 'static
{
    crate::shell_work::run(move || crate::services::plan_trash_restore(&entry))
}

// Restore to a free original path without ever replacing a race-created file.
pub(crate) fn restore_trash_entry(
    entry: crate::trash::TrashEntry,
) -> impl std::future::Future<Output = Result<(), String>> + Send + 'static {
    crate::shell_work::run(move || {
        let name = entry.name.clone();
        crate::trash::restore_without_replacing(&entry).map_err(|e| {
            crate::i18n::format(
                "files-could-not-restore-name-e-9d89703e",
                &[("name", (name).clone()), ("e", (e).to_string())],
            )
        })
    })
}

// Returns the verdict paired with the carried entry so the reply
// arm can route the entry back to the right stack on any outcome.
pub(crate) fn apply_inverse_async(
    op: journal::InverseOp,
    entry: journal::JournalEntry,
) -> impl std::future::Future<Output = (journal::InverseResult, journal::JournalEntry)> + Send + 'static
{
    crate::shell_work::run(move || {
        let result = journal::apply_inverse(&op);
        (result, entry)
    })
}

// Precedence: unavailable > iterator failed > either cap reached >
// refused metadata. `None` means the scan retained everything it saw.
pub(crate) fn classify_trash_incomplete(
    listing: &crate::trash::TrashListing,
) -> Option<model::TrashListingIncomplete> {
    if listing.unavailable {
        return Some(model::TrashListingIncomplete::Unavailable);
    }
    if listing.iterator_failed {
        return Some(model::TrashListingIncomplete::IteratorFailed);
    }
    if listing.entry_cap_reached {
        return Some(model::TrashListingIncomplete::EntryCapReached);
    }
    if listing.candidate_cap_reached {
        return Some(model::TrashListingIncomplete::CandidateCapReached);
    }
    if listing.refused_metadata > 0 {
        return Some(model::TrashListingIncomplete::RefusedMetadata(
            listing.refused_metadata,
        ));
    }
    None
}

pub(crate) fn trash_listing_warning(
    state: Option<&model::TrashListingIncomplete>,
) -> Option<String> {
    match state {
        None => None,
        Some(model::TrashListingIncomplete::Unavailable) => {
            Some(crate::i18n::tr("Trash listing is unavailable").to_owned())
        }
        Some(model::TrashListingIncomplete::IteratorFailed) => Some(
            crate::i18n::tr("Trash listing skipped entries the directory iterator refused")
                .to_owned(),
        ),
        Some(model::TrashListingIncomplete::EntryCapReached) => Some(crate::i18n::format(
            "files-trash-listing-was-truncated-at-entries-309ef37f",
            &[(
                "arg0",
                (crate::trash::MAX_RETAINED_TRASH_ENTRIES).to_string(),
            )],
        )),
        Some(model::TrashListingIncomplete::CandidateCapReached) => Some(crate::i18n::format(
            "files-trash-listing-stopped-after-directory-candidates-ee286e34",
            &[("arg0", (crate::trash::MAX_CANDIDATES_SCANNED).to_string())],
        )),
        Some(model::TrashListingIncomplete::RefusedMetadata(n)) if *n == 1 => {
            Some(crate::i18n::tr("Trash listing: 1 entry has refused metadata").to_owned())
        }
        Some(model::TrashListingIncomplete::RefusedMetadata(n)) => Some(crate::i18n::format(
            "files-trash-listing-n-entries-have-refused-52d31b7a",
            &[("n", (n).to_string())],
        )),
    }
}

// freedesktop trash layout: `<trash>/files/NAME` <->
// `<trash>/info/NAME.trashinfo`.
pub(crate) fn trashinfo_path_of(trashed_path: &std::path::Path) -> std::path::PathBuf {
    let name = trashed_path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    let files_dir = trashed_path.parent();
    let trash_root = files_dir.and_then(|d| d.parent());
    match trash_root {
        Some(root) => {
            let mut info_name = std::ffi::OsString::from(&name);
            info_name.push(".trashinfo");
            root.join("info").join(info_name)
        }
        None => {
            let mut p = trashed_path.to_path_buf();
            let mut n = std::ffi::OsString::from(&name);
            n.push(".trashinfo");
            p.set_file_name(n);
            p
        }
    }
}

// One bounded, path-free warning per batch with per-reason counts, including
// batches whose forward op landed but whose Undo entry could not be built.
#[derive(Default, Debug)]
pub(crate) struct JournalAdmissionTally {
    admitted: usize,
    too_large: usize,
    identity_exhausted: usize,
    undo_in_flight: usize,
    capture_unavailable: usize,
}

impl JournalAdmissionTally {
    pub(crate) fn observe(&mut self, outcome: Result<journal::JournalId, journal::AdmissionError>) {
        match outcome {
            Ok(_) => self.admitted = self.admitted.saturating_add(1),
            Err(journal::AdmissionError::MetadataTooLarge { .. }) => {
                self.too_large = self.too_large.saturating_add(1);
            }
            Err(journal::AdmissionError::IdentityExhausted) => {
                self.identity_exhausted = self.identity_exhausted.saturating_add(1);
            }
            Err(journal::AdmissionError::UndoInFlight) => {
                self.undo_in_flight = self.undo_in_flight.saturating_add(1);
            }
        }
    }

    // Pre-record custody capture failed; forward op still landed.
    pub(crate) fn note_capture_unavailable(&mut self) {
        self.capture_unavailable = self.capture_unavailable.saturating_add(1);
    }

    pub(crate) fn touched(&self) -> bool {
        self.admitted
            + self.too_large
            + self.identity_exhausted
            + self.undo_in_flight
            + self.capture_unavailable
            > 0
    }

    // Precedence: Undo-in-flight > identity-exhausted > per-item causes.
    pub(crate) fn warning_for(&self, kind: &'static str) -> Option<String> {
        if self.undo_in_flight > 0 {
            return Some(crate::i18n::format(
                "files-kind-completed-but-undo-history-was-157e838d",
                &[("kind", (kind).to_owned())],
            ));
        }
        if self.identity_exhausted > 0 {
            let per_item = self.too_large.saturating_add(self.capture_unavailable);
            return Some(match per_item {
                0 => crate::i18n::format(
                    "files-kind-completed-undo-history-is-unavailable-a6fe218a",
                    &[("kind", (kind).to_owned())],
                ),
                n => crate::i18n::format(
                    "files-kind-completed-for-n-items-the-a0b233ad",
                    &[("kind", (kind).to_owned()), ("n", (n).to_string())],
                ),
            });
        }
        match (self.too_large, self.capture_unavailable) {
            (0, 0) => None,
            (1, 0) => Some(crate::i18n::format(
                "files-kind-completed-too-large-to-add-c8cb490c",
                &[("kind", (kind).to_owned())],
            )),
            (n, 0) => Some(crate::i18n::format(
                "files-kind-completed-for-n-items-some-344778ec",
                &[("kind", (kind).to_owned()), ("n", (n).to_string())],
            )),
            (0, 1) => Some(crate::i18n::format(
                "files-kind-completed-undo-is-unavailable-for-67f493bc",
                &[("kind", (kind).to_owned())],
            )),
            (0, n) => Some(crate::i18n::format(
                "files-kind-completed-undo-is-unavailable-for-688b23ad",
                &[("kind", (kind).to_owned()), ("n", (n).to_string())],
            )),
            (t, u) => {
                let total = t.saturating_add(u);
                Some(crate::i18n::format(
                    "files-kind-completed-undo-is-unavailable-for-895cad78",
                    &[("kind", (kind).to_owned()), ("total", (total).to_string())],
                ))
            }
        }
    }
}

// Names paths but never file bytes.
pub(crate) fn describe_conflict(conflict: &journal::InverseConflict) -> String {
    match conflict {
        journal::InverseConflict::IdentityChanged { path, .. } => crate::i18n::format(
            "files-the-file-at-was-replaced-or-319c2104",
            &[("arg0", (path.display()).to_string())],
        ),
        journal::InverseConflict::DestinationOccupied { to } => crate::i18n::format(
            "files-is-now-occupied-bd2d43f1",
            &[("arg0", (to.display()).to_string())],
        ),
        journal::InverseConflict::RollbackUncertain { .. } => {
            crate::i18n::tr("an unexpected entry was moved and the rollback custody is uncertain")
                .to_owned()
        }
        journal::InverseConflict::TrashMetadataRetireFailed { .. } => crate::i18n::tr(
            "the payload was returned to the trash because its metadata could not be retired; \
             retry after clearing the fault",
        )
        .to_owned(),
        journal::InverseConflict::TrashMetadataTwinChanged { .. } => crate::i18n::tr(
            "the payload was returned to the trash because the trash metadata is now a foreign \
             entry; the foreign bytes were left in place",
        )
        .to_owned(),
        journal::InverseConflict::TrashUndoSettlementUncertain { .. } => crate::i18n::tr(
            "the payload was returned to its original location and the trash metadata state is \
             uncertain; retry after clearing the fault",
        )
        .to_owned(),
        journal::InverseConflict::TrashMetadataPublishOccupied { .. } => crate::i18n::tr(
            "the payload was returned to its original location because the trash metadata slot \
             is occupied by a foreign entry; clear the conflict and retry",
        )
        .to_owned(),
        journal::InverseConflict::TrashMetadataPublishFailed { .. } => crate::i18n::tr(
            "the payload was returned to its original location because the trash metadata could \
             not be published; retry after clearing the fault",
        )
        .to_owned(),
        journal::InverseConflict::TrashPublishSettlementUncertain { .. } => crate::i18n::tr(
            "the payload is in the trash and the trash metadata state is uncertain; \
             retry after clearing the fault",
        )
        .to_owned(),
        journal::InverseConflict::CrossFilesystemInverseUncertain { at } => crate::i18n::format(
            "files-a-cross-filesystem-inverse-to-cannot-64b9717a",
            &[("arg0", (at.display()).to_string())],
        ),
        journal::InverseConflict::ParentUnavailable { parent } => crate::i18n::format(
            "files-the-original-folder-is-no-longer-17ce2cfd",
            &[("arg0", (parent.display()).to_string())],
        ),
        journal::InverseConflict::TrashItemUnavailable { trashed_path, .. } => crate::i18n::format(
            "files-the-trash-entry-at-is-no-a68f9a27",
            &[("arg0", (trashed_path.display()).to_string())],
        ),
    }
}

// Removes the orphaned `.trashinfo` twin after the conflict executor moved
// the payload.
pub(crate) fn finalize_conflict_restore(
    entry: crate::trash::TrashEntry,
) -> impl std::future::Future<Output = Result<(), String>> + Send + 'static {
    crate::shell_work::run(move || {
        if entry.trashed_path.exists() {
            return Err(crate::i18n::format(
                "files-could-not-restore-c161af19",
                &[("arg0", (entry.name))],
            ));
        }
        let _ = std::fs::remove_file(&entry.info_path);
        Ok(())
    })
}

// Permanently delete captured Trash entries and their metadata twins off the
// render thread. Used by both delete-one and Empty Trash after confirmation.
pub(crate) fn delete_trash_selection(
    entries: Vec<crate::trash::TrashEntry>,
) -> impl std::future::Future<Output = crate::trash::PermanentDeleteOutcome> + Send + 'static {
    crate::shell_work::run(move || crate::trash::delete_trash_entries(&entries))
}

// Unmounts a filesystem off the render thread through
// [`services::UnmountService`].
pub(crate) fn unmount_drive(
    service: services::UnmountService,
    mount: PathBuf,
    cancel: CancelToken,
) -> impl std::future::Future<Output = Result<PathBuf, String>> + Send + 'static {
    crate::shell_work::run(move || service.unmount(&mount, &cancel))
}

// Creates a new folder off the render thread.
pub(crate) fn new_folder(
    dir: PathBuf,
) -> impl std::future::Future<Output = Result<PathBuf, String>> + Send + 'static {
    crate::shell_work::run(move || services::create_folder(&dir))
}

// Creates a new file off the render thread.
pub(crate) fn new_file(
    dir: PathBuf,
) -> impl std::future::Future<Output = Result<PathBuf, String>> + Send + 'static {
    crate::shell_work::run(move || services::create_file(&dir))
}

// Trashes a batch off the render thread.
pub(crate) fn trash_selection(
    sources: Vec<PathBuf>,
    home_data_dir: PathBuf,
) -> impl std::future::Future<Output = crate::trash::TrashOutcome> + Send + 'static {
    crate::shell_work::run(move || crate::trash::trash_batch_with(&sources, &home_data_dir))
}

// Permanently deletes a batch off the render thread.
pub(crate) fn permanent_delete_selection(
    sources: Vec<PathBuf>,
) -> impl std::future::Future<Output = crate::trash::PermanentDeleteOutcome> + Send + 'static {
    crate::shell_work::run(move || crate::trash::permanent_delete_batch(&sources))
}

// Walks a directory tree off the render thread for the Properties Total Size.
pub(crate) fn recursive_size(
    path: PathBuf,
) -> impl std::future::Future<Output = (u64, usize)> + Send + 'static {
    crate::shell_work::run(move || services::recursive_folder_size(&path))
}

// Walks a subtree off the render thread for `SearchScope::Everywhere`.
pub(crate) fn recursive_search(
    root: PathBuf,
    needle: String,
) -> impl std::future::Future<Output = Vec<model::FileEntry>> + Send + 'static {
    crate::shell_work::run_cancellable(crate::shell_work::Pool::Background, move |cancelled| {
        services::recursive_search(&root, &needle, cancelled)
    })
}

// Applies a bulk-rename batch off the render thread.
pub(crate) fn apply_renames(
    operations: Vec<(PathBuf, PathBuf)>,
) -> impl std::future::Future<Output = services::BulkRenameOutcome> + Send + 'static {
    crate::shell_work::run(move || services::apply_bulk_rename(&operations))
}

// Queries the default application and all installed handlers off-thread;
// both results are kept independently.
pub(crate) fn query_mime_applications(
    query: mime_query::MimeQueryHandle,
    mime: String,
    generation: u64,
    process_cancel: CancelToken,
    scan_cancel: network::OperationCancel,
) -> Task<Msg> {
    let result_mime = mime.clone();
    Task::perform(
        crate::shell_work::run(move || {
            mime_query::query(query.as_ref(), &mime, &process_cancel, &scan_cancel)
        }),
        move |outcome| Msg::OpenWithQueried {
            generation,
            mime: result_mime,
            outcome,
        },
    )
}

// Scans paste sources off the render thread.
pub(crate) fn plan_paste(
    sources: Vec<PathBuf>,
    dest_dir: PathBuf,
    copy: bool,
) -> impl std::future::Future<Output = conflict::PastePlan> + Send + 'static {
    crate::shell_work::run(move || {
        let mode = if copy {
            conflict::ClipMode::Copy
        } else {
            conflict::ClipMode::Cut
        };
        services::plan_paste(&sources, &dest_dir, mode)
    })
}

fn transfer_scope() -> enzui_services::process_transport::BackgroundScope {
    enzui_services::process_transport::BackgroundScope::worker("files-copy")
}

fn transfer_entry_count(path: &Path) -> std::io::Result<u64> {
    let mut count = 1u64;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entries = if entry.file_type()?.is_dir() {
            transfer_entry_count(&entry.path())?
        } else {
            1
        };
        count = count.saturating_add(entries);
    }
    Ok(count)
}

fn perform_transfer(
    src: &Path,
    dst: &Path,
    choice: conflict::ConflictChoice,
    mode: conflict::ClipMode,
    entry_copied: &mut dyn FnMut(),
) -> conflict::ItemOutcome {
    #[cfg(test)]
    transfer_priority::record(dst);
    services::perform_conflict_choice_with_progress(src, dst, choice, mode, entry_copied)
}

// Builds the complete paste queue on Files' bounded shell worker pool.
//
// Free destinations are copied or moved before conflicts are returned, on a
// thread at the background nice level. Shell publication settles on the
// worker before the UI receives the queue.
pub(crate) fn open_paste_queue(
    dest_dir: PathBuf,
    mode: conflict::ClipMode,
    mut plan: conflict::PastePlan,
    mut status: Option<shell_activity::Transfer>,
    mut report: impl FnMut(f32) + Send + 'static,
) -> impl std::future::Future<Output = Box<conflict::ConflictQueue>> + Send + 'static {
    crate::shell_work::run(move || {
        let queue =
            enzui_services::process_transport::run_on_lowered_thread(&transfer_scope(), || {
                let folders = std::mem::take(&mut plan.copy_folders);
                let total = (plan.clear.len() + plan.conflicts.len() + plan.refused.len()) as f32;
                let mut completed = 0.0;
                let mut last_report = None::<std::time::Instant>;
                conflict::ConflictQueue::from_plan(
                    dest_dir,
                    mode,
                    plan,
                    &mut |src, dst, choice, mode| {
                        let count = folders
                            .contains(dst)
                            .then(|| transfer_entry_count(src).ok())
                            .flatten();
                        let mut staged = 0u64;
                        let outcome = perform_transfer(src, dst, choice, mode, &mut || {
                            if let Some(count) = count {
                                staged += 1;
                                if last_report.is_none_or(|at| {
                                    at.elapsed() >= std::time::Duration::from_millis(125)
                                }) {
                                    // Keep progress below completion until the staged folder is published.
                                    let item = (staged as f32 / count as f32).min(0.99);
                                    let progress = (completed + item) / total;
                                    report(progress);
                                    if let Some(status) = &status {
                                        status.copy_progress(progress);
                                    }
                                    last_report = Some(std::time::Instant::now());
                                }
                            }
                        });
                        if outcome.changed_destination() {
                            completed += 1.0;
                        }
                        if let Some(status) = &mut status {
                            status.performed(&outcome);
                        }
                        outcome
                    },
                )
            });
        if let Some(status) = &status {
            status.settle(&queue);
        }
        Box::new(queue)
    })
}

// Applies one conflict decision off the render thread and returns the
// advanced queue.
pub(crate) fn resolve_conflict(
    mut queue: conflict::ConflictQueue,
    choice: conflict::ConflictChoice,
    mut status: Option<shell_activity::Transfer>,
) -> impl std::future::Future<Output = Box<conflict::ConflictQueue>> + Send + 'static {
    crate::shell_work::run(move || {
        if let Some(status) = &mut status {
            status.resume_from(&queue);
        }
        enzui_services::process_transport::run_on_lowered_thread(&transfer_scope(), || {
            queue.resolve(choice, &mut |src, dst, choice, mode| {
                let outcome = perform_transfer(src, dst, choice, mode, &mut || {});
                if let Some(status) = &mut status {
                    status.performed(&outcome);
                }
                outcome
            });
            services::remove_emptied_merge_sources(&mut queue.merged_sources);
        });
        if let Some(status) = &status {
            status.settle(&queue);
        }
        Box::new(queue)
    })
}

// What each transfer item's thread ran at, for the scheduling tests.
#[cfg(test)]
pub(crate) mod transfer_priority {
    use std::path::{Path, PathBuf};

    static OBSERVED: std::sync::Mutex<Vec<(PathBuf, i64)>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn record(dst: &Path) {
        let nice = std::fs::read_to_string("/proc/thread-self/stat")
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")?
                    .1
                    .split_whitespace()
                    .nth(16)?
                    .parse()
                    .ok()
            })
            .unwrap_or(i64::MIN);
        OBSERVED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((dst.to_path_buf(), nice));
    }
}
