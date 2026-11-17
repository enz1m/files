// Files-app keyboard shortcuts: one table for the dispatcher, the Keyboard
// Shortcuts dialog, toolbar chord text and the count gate.
//
// Each row is a framework [`ShortcutBinding`]. [`dispatch_shortcut_command`]
// matches structured `(Key, Modifiers)` and rejects repeat events before
// scanning; [`shortcut_help`] emits one help row per binding, alternates
// included. Redo (`Ctrl+Y`) and Zoom in (`Ctrl++`, `Ctrl+Shift++`) carry
// alternate chords.

use enzui_core::editable_shortcuts::{ShortcutOverrides, ShortcutValidity};
use enzui_core::persist::{FileBackend, InMemoryBackend, RawByteLimit, Store};
use enzui_core::shortcut::{Modifier, NamedKey, Shortcut, ShortcutKey};
use enzui_core::{
    EditableShortcutCatalog, ShortcutBinding, dispatch_shortcut_command, shortcut_help,
};
use enzui_widgets::shortcut_list::ShortcutEntry;
use iced::keyboard;
use std::path::Path;
#[cfg(not(test))]
use std::sync::RwLock;
use std::sync::{Arc, LazyLock};

// A bindable files-app command.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FilesAction {
    // Open the "Select items matching…" glob dialog (`Ctrl+S`).
    SelectMatching,
    // Copy the current selection to the internal clipboard (`Ctrl+C`).
    Copy,
    // Cut the current selection (`Ctrl+X`).
    Cut,
    // Paste the clipboard into the current directory (`Ctrl+V`).
    Paste,
    // Open the folder chooser to move the selection (`Ctrl+Shift+M`); the keyboard
    // twin of drag-to-folder, with the same `route_item_drop` validation.
    MoveTo,
    // Open the folder chooser to copy the selection (`Ctrl+Shift+G`).
    CopyTo,
    // Select every visible entry (`Ctrl+A`).
    SelectAll,
    // Toggle hidden file visibility (`Ctrl+H`).
    ToggleHidden,
    // Begin inline path-bar editing (`Ctrl+L`).
    LocationBar,
    // Toggle the search bar (`Ctrl+F`).
    Search,
    // Rename the selection in the bulk-rename dialog (`F2`).
    Rename,
    // Preview the selected file (`Space`), as Quick Look and Sushi do; in a
    // preview window, close it.
    Preview,
    Properties,
    // Reload the current directory listing (`F5`).
    Refresh,
    // Navigate back in history (`Alt+Left`).
    GoBack,
    // Navigate forward in history (`Alt+Right`).
    GoForward,
    // Navigate to the parent directory (`Alt+Up`).
    GoUp,
    // Create a new folder in the current directory (`Ctrl+Shift+N`).
    NewFolder,
    // Create a new empty file in the current directory (`Ctrl+N`).
    NewFile,
    // Move the current selection to the Trash (`Delete`).
    MoveToTrash,
    // Ask to permanently delete the selection (`Shift+Delete`).
    //
    // Opens the confirmation dialog; a binding cannot bypass it.
    AskPermanentDelete,
    // Undo the newest session journal entry (`Ctrl+Z`); no-op when empty.
    Undo,
    // Redo the newest undone entry (`Ctrl+Shift+Z`, alternate `Ctrl+Y`).
    Redo,
    // One zoom level larger (`Ctrl+=`, alternates `Ctrl++` and `Ctrl+Shift++`).
    ZoomIn,
    // One zoom level smaller (`Ctrl+-`).
    ZoomOut,
    // Back to the default zoom level (`Ctrl+0`).
    ZoomReset,
    // Open a tab at the current location (`Ctrl+T`).
    NewTab,
    // Close the active tab (`Ctrl+W`).
    CloseTab,
    // The next tab (`Ctrl+PageDown`).
    NextTab,
    // The previous tab (`Ctrl+PageUp`).
    PreviousTab,
}

// A files-app catalog row: an unconditional framework [`ShortcutBinding`].
pub(crate) type FilesBinding = ShortcutBinding<FilesAction>;

fn command(
    id: &'static str,
    action: FilesAction,
    label: &'static str,
    shortcut: Shortcut,
) -> FilesBinding {
    ShortcutBinding::new(id, action, shortcut).with_description(label)
}

// Full catalog of bound files-app actions, in stable order.
pub(crate) static ALL_BINDINGS: LazyLock<Vec<FilesBinding>> = LazyLock::new(|| {
    vec![
        command(
            "files.properties",
            FilesAction::Properties,
            "Properties",
            Shortcut::new(Modifier::CTRL, 'i'),
        ),
        command(
            "files.copy",
            FilesAction::Copy,
            "Copy",
            Shortcut::new(Modifier::CTRL, 'c'),
        ),
        command(
            "files.cut",
            FilesAction::Cut,
            "Cut",
            Shortcut::new(Modifier::CTRL, 'x'),
        ),
        command(
            "files.paste",
            FilesAction::Paste,
            "Paste",
            Shortcut::new(Modifier::CTRL, 'v'),
        ),
        command(
            "files.move-to",
            FilesAction::MoveTo,
            "Move to…",
            Shortcut::new(Modifier::CTRL | Modifier::SHIFT, 'm'),
        ),
        command(
            "files.copy-to",
            FilesAction::CopyTo,
            "Copy to…",
            Shortcut::new(Modifier::CTRL | Modifier::SHIFT, 'g'),
        ),
        command(
            "files.select-all",
            FilesAction::SelectAll,
            "Select all",
            Shortcut::new(Modifier::CTRL, 'a'),
        ),
        command(
            "files.select-matching",
            FilesAction::SelectMatching,
            "Select items matching…",
            Shortcut::new(Modifier::CTRL, 's'),
        ),
        command(
            "files.toggle-hidden",
            FilesAction::ToggleHidden,
            "Toggle hidden files",
            Shortcut::new(Modifier::CTRL, 'h'),
        ),
        command(
            "files.search",
            FilesAction::Search,
            "Search",
            Shortcut::new(Modifier::CTRL, 'f'),
        ),
        command(
            "files.location-bar",
            FilesAction::LocationBar,
            "Location bar",
            Shortcut::new(Modifier::CTRL, 'l'),
        ),
        command(
            "files.refresh",
            FilesAction::Refresh,
            "Refresh",
            Shortcut::new(Modifier::NONE, NamedKey::F5),
        ),
        command(
            "files.rename",
            FilesAction::Rename,
            "Rename",
            Shortcut::new(Modifier::NONE, NamedKey::F2),
        ),
        command(
            "files.preview",
            FilesAction::Preview,
            "Preview",
            Shortcut::new(Modifier::NONE, NamedKey::Space),
        ),
        command(
            "files.go-back",
            FilesAction::GoBack,
            "Go back",
            Shortcut::new(Modifier::ALT, NamedKey::ArrowLeft),
        ),
        command(
            "files.go-forward",
            FilesAction::GoForward,
            "Go forward",
            Shortcut::new(Modifier::ALT, NamedKey::ArrowRight),
        ),
        command(
            "files.go-up",
            FilesAction::GoUp,
            "Go to parent folder",
            Shortcut::new(Modifier::ALT, NamedKey::ArrowUp),
        ),
        command(
            "files.new-folder",
            FilesAction::NewFolder,
            "New folder",
            Shortcut::new(Modifier::CTRL | Modifier::SHIFT, 'n'),
        ),
        command(
            "files.new-file",
            FilesAction::NewFile,
            "New file",
            Shortcut::new(Modifier::CTRL, 'n'),
        ),
        command(
            "files.move-to-trash",
            FilesAction::MoveToTrash,
            "Move to Trash",
            Shortcut::new(Modifier::NONE, NamedKey::Delete),
        ),
        command(
            "files.delete-permanently",
            FilesAction::AskPermanentDelete,
            "Delete permanently…",
            Shortcut::new(Modifier::SHIFT, NamedKey::Delete),
        ),
        command(
            "files.undo",
            FilesAction::Undo,
            "Undo",
            Shortcut::new(Modifier::CTRL, 'z'),
        ),
        command(
            "files.redo",
            FilesAction::Redo,
            "Redo",
            Shortcut::new(Modifier::CTRL | Modifier::SHIFT, 'z'),
        )
        .with_alternate(Shortcut::new(Modifier::CTRL, 'y')),
        command(
            "files.zoom-in",
            FilesAction::ZoomIn,
            "Zoom in",
            Shortcut::new(Modifier::CTRL, '='),
        )
        .with_alternates([
            Shortcut::new(Modifier::CTRL, '+'),
            Shortcut::new(Modifier::CTRL | Modifier::SHIFT, '+'),
        ]),
        command(
            "files.zoom-out",
            FilesAction::ZoomOut,
            "Zoom out",
            Shortcut::new(Modifier::CTRL, '-'),
        ),
        command(
            "files.zoom-reset",
            FilesAction::ZoomReset,
            "Reset zoom",
            Shortcut::new(Modifier::CTRL, '0'),
        ),
        command(
            "files.new-tab",
            FilesAction::NewTab,
            "New tab",
            Shortcut::new(Modifier::CTRL, 't'),
        ),
        command(
            "files.close-tab",
            FilesAction::CloseTab,
            "Close tab",
            Shortcut::new(Modifier::CTRL, 'w'),
        ),
        command(
            "files.next-tab",
            FilesAction::NextTab,
            "Next tab",
            Shortcut::new(Modifier::CTRL, NamedKey::PageDown),
        ),
        command(
            "files.previous-tab",
            FilesAction::PreviousTab,
            "Previous tab",
            Shortcut::new(Modifier::CTRL, NamedKey::PageUp),
        ),
    ]
});

// The editable catalog: [`ALL_BINDINGS`] as defaults plus the user's overrides.
pub(crate) type FilesShortcuts = EditableShortcutCatalog<FilesAction>;

// The resolved table in force. The event classifier runs without the model,
// so the reducer installs the catalog's table here after every change, and
// dispatch, help and menu chord labels all read the same table.
#[cfg(not(test))]
static INSTALLED: LazyLock<RwLock<Arc<Vec<FilesBinding>>>> =
    LazyLock::new(|| RwLock::new(Arc::new(ALL_BINDINGS.clone())));

// Tests run in parallel threads; a per-thread table keeps one test's
// rebinding out of another's dispatch.
#[cfg(test)]
thread_local! {
    static INSTALLED_FOR_TEST: std::cell::RefCell<Arc<Vec<FilesBinding>>> =
        std::cell::RefCell::new(Arc::new(ALL_BINDINGS.clone()));
}

// The catalog for persisted `overrides`; unknown ids and default-equal
// values are dropped.
#[must_use]
pub(crate) fn catalog(overrides: ShortcutOverrides) -> FilesShortcuts {
    EditableShortcutCatalog::from_overrides(ALL_BINDINGS.clone(), overrides)
}

// Make `catalog`'s resolved table the one dispatch and labels use.
pub(crate) fn install(catalog: &FilesShortcuts) {
    let table = Arc::new(catalog.resolved().to_vec());
    #[cfg(not(test))]
    match INSTALLED.write() {
        Ok(mut installed) => *installed = table,
        Err(poisoned) => *poisoned.into_inner() = table,
    }
    #[cfg(test)]
    INSTALLED_FOR_TEST.with(|installed| *installed.borrow_mut() = table);
}

pub(crate) fn bindings() -> Arc<Vec<FilesBinding>> {
    #[cfg(not(test))]
    match INSTALLED.read() {
        Ok(installed) => Arc::clone(&installed),
        Err(poisoned) => Arc::clone(&poisoned.into_inner()),
    }
    #[cfg(test)]
    INSTALLED_FOR_TEST.with(|installed| Arc::clone(&installed.borrow()))
}

// The current primary chord of action `id`, as shown in menus.
#[must_use]
pub(crate) fn label(id: &str) -> Option<String> {
    bindings()
        .iter()
        .find(|binding| binding.id() == id)
        .map(|binding| binding.primary().localized_label())
}

// Whether `candidate` may be bound. F1 opens this list and Tab and Escape
// move focus and close dialogs; a bare letter or digit would take type-to-find
// keys; the desktop owns Super chords and the session keys.
#[must_use]
pub(crate) fn validate(candidate: &Shortcut) -> ShortcutValidity {
    let mods = candidate.modifiers();
    match candidate.key() {
        ShortcutKey::Named(NamedKey::Escape | NamedKey::Tab | NamedKey::F1) => {
            return ShortcutValidity::Invalid(
                crate::i18n::tr("Files uses this key for focus, dialogs or help.").to_owned(),
            );
        }
        ShortcutKey::Character(_)
            if !mods.contains(Modifier::CTRL) && !mods.contains(Modifier::ALT) =>
        {
            return ShortcutValidity::Invalid(
                crate::i18n::tr("Add Ctrl or Alt, so typing still finds files.").to_owned(),
            );
        }
        _ => {}
    }
    if mods.contains(Modifier::SUPER)
        || *candidate == Shortcut::new(Modifier::ALT, NamedKey::F4)
        || *candidate == Shortcut::new(Modifier::CTRL | Modifier::ALT, NamedKey::Delete)
    {
        return ShortcutValidity::Reserved(
            crate::i18n::tr("This shortcut is reserved by the desktop.").to_owned(),
        );
    }
    ShortcutValidity::Valid
}

// The persistence identity of the override map; a version bump refuses an
// older file instead of misreading it.
const OVERRIDES_FORMAT: &str = "files-app-shortcuts";
const OVERRIDES_VERSION: u32 = 1;
const OVERRIDE_RECORDS_MAX: u64 = 64;
const OVERRIDES_PREAMBLE_BYTES_MAX: u64 = 1024;
const OVERRIDES_RAW_BYTE_LIMIT: RawByteLimit = match RawByteLimit::for_body(
    OVERRIDES_FORMAT,
    OVERRIDES_VERSION,
    OVERRIDES_PREAMBLE_BYTES_MAX
        + OVERRIDE_RECORDS_MAX * enzui_core::atomic_state::MAX_RECORD_BYTES,
) {
    Ok(limit) => limit,
    Err(_) => panic!("shortcut body ceiling exceeds framework authority"),
};

// The override store: a file for `path`, or memory for `None` (tests).
#[must_use]
pub(crate) fn store(path: Option<&Path>) -> Store<ShortcutOverrides> {
    match path {
        Some(path) => Store::new(
            FileBackend::new(path, OVERRIDES_RAW_BYTE_LIMIT),
            OVERRIDES_FORMAT,
            OVERRIDES_VERSION,
        ),
        None => Store::new(InMemoryBackend::new(), OVERRIDES_FORMAT, OVERRIDES_VERSION),
    }
}

// The help dialog's rows for `catalog`, one per binding with alternates folded in.
#[must_use]
pub(crate) fn shortcut_dialog_entries_from(bindings: &[FilesBinding]) -> Vec<ShortcutEntry> {
    shortcut_help(bindings.iter())
        .into_iter()
        .map(|row| {
            let chords = row
                .shortcuts()
                .map(Shortcut::localized_label)
                .collect::<Vec<_>>()
                .join(", ");
            ShortcutEntry::new(
                crate::i18n::text(row.section()),
                chords,
                crate::i18n::text(row.description()),
            )
            .stable_key(row.id())
        })
        .collect()
}

// The shortcut-help dialog's rows, derived from [`ALL_BINDINGS`].
#[must_use]
pub(crate) fn shortcut_dialog_entries() -> Vec<ShortcutEntry> {
    shortcut_dialog_entries_from(&bindings())
}

// Decodes a key press into its bound [`FilesAction`]; repeat events never match.
#[must_use]
pub(crate) fn dispatch(event: &keyboard::Event) -> Option<FilesAction> {
    dispatch_shortcut_command(bindings().iter(), event, &())
}
