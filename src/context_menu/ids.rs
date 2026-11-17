// Menu invoker and row ids.

use super::*;

// An injective, UTF-8-safe key for a filesystem path.
//
// `to_string_lossy` is not injective for non-UTF-8 paths, so ids would alias;
// hex-encoding the raw `OsStr` bytes maps distinct paths to distinct keys.
fn path_key(path: &Path) -> String {
    use std::fmt::Write;
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    let mut key = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(key, "{b:02x}");
    }
    key
}

// The path-derived id used by the Trash list's row surface.
//
// Trash can hold two originals with the same name; the disambiguated payload path
// is unique and keyed through [`path_key`].
pub(crate) fn trash_row_id(trashed_path: &Path) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(
        crate::view::shell::TRASH_ROW_ID_PREFIX,
        &path_key(trashed_path),
    )
}

// The routable context-affordance id paired with [`trash_row_id`].
pub(crate) fn trash_context_id(trashed_path: &Path) -> iced::advanced::widget::Id {
    enzui_widgets::context_menu_host::derive_context_menu_id(&trash_row_id(trashed_path))
}

// The widget id of the list row for `path`.
//
// Keyed by the full path through [`path_key`], since recursive search can show
// two entries with one basename. The accessible label stays the display name.
// Must match `view::list::entry_row`'s `.id(list_row_id(&entry.path))`.
pub(crate) fn list_row_id(path: &Path) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(crate::view::list::LIST_ROW_ID_PREFIX, &path_key(path))
}

// The row surface's context-affordance id, path-keyed like [`list_row_id`].
pub(crate) fn list_context_id(path: &Path) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(
        &format!("{}-ctx", crate::view::list::LIST_ROW_ID_PREFIX),
        &path_key(path),
    )
}

// The widget id of the grid tile for `path`, path-keyed like [`list_row_id`].
pub(crate) fn grid_tile_id(path: &Path) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(crate::view::grid::GRID_TILE_ID_PREFIX, &path_key(path))
}
