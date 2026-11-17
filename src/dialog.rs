// App-owned dialog content and shared framework modal hosting.

use enzui_core::semantics::HeadingLevel;
use enzui_theme::EnzTheme;
use enzui_widgets::badge::{BadgeTone, enz_badge};
use enzui_widgets::capped_dialog_body::enz_capped_dialog_body;
use enzui_widgets::checkbox::{CheckboxTokens, enz_checkbox};
use enzui_widgets::combo_box::enz_combo_box;
use enzui_widgets::confirm_dialog::enz_confirm_dialog;
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::expander::enz_expander;
use enzui_widgets::labeled_content::{enz_labeled_content, enz_labeled_content_compact};
use enzui_widgets::modal_host::{
    ModalHostEvent, initial_focus_operation, presentation_scope_id, restore_focus_operation,
    scoped_focus_operation,
};
use enzui_widgets::modal_surface::{ModalSurfaceTokens, enz_modal_surface};
use enzui_widgets::preferences_group::enz_preferences_group;
use enzui_widgets::preferences_row::enz_preferences_row;
use enzui_widgets::radio_group::{Orientation, RadioTokens, enz_radio_group};
use enzui_widgets::reflow_pair::{PairArm, reflow_pair};
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::scroll_view::enz_scroll_view;
use enzui_widgets::section_header::enz_section_header;
use enzui_widgets::spinner::enz_spinner;
use enzui_widgets::stepper::enz_stepper;
use enzui_widgets::surface::{SurfaceTone, enz_surface};
use enzui_widgets::text_field::{TextFieldSize, enz_text_field};
use enzui_widgets::{ButtonKind, enz_button};
use iced::widget::{Space, row};
use iced::{Alignment, Element, Length, Size, Task};

use crate::message::Msg;
use crate::model::{Model, PropertiesInfo, SearchFilterDate, SearchFilterType, SortKey, ViewMode};

// The animated Settings host keeps a stable scope across exit generations.
pub(crate) const GENERIC_DIALOG_SCOPE: &str = "files-app-generic-dialog";

pub(crate) fn generic_dialog_scope_id() -> iced::advanced::widget::Id {
    presentation_scope_id(GENERIC_DIALOG_SCOPE)
}

pub(crate) fn initial_focus_task() -> Task<Msg> {
    iced::advanced::widget::operate(initial_focus_operation(generic_dialog_scope_id()))
}

pub(crate) fn focus_task(event: ModalHostEvent) -> Task<Msg> {
    match scoped_focus_operation(event, generic_dialog_scope_id()) {
        Some(operation) => iced::advanced::widget::operate(operation),
        None => Task::none(),
    }
}

// Restore focus after a generic dialog closes.
pub(crate) fn restore_focus_task(
    event: ModalHostEvent,
    invoker: Option<&iced::advanced::widget::Id>,
) -> Task<Msg> {
    match restore_focus_operation(event, invoker) {
        Some(operation) => iced::advanced::widget::operate(operation),
        None => Task::none(),
    }
}

pub(crate) const DELETE_CONFIRM_SCOPE: &str = "files-app-delete-confirm";

// Entries the delete summary names before folding the rest into `+N items`,
// keeping the dialog a bounded height for any selection size.
pub(crate) const DELETE_SUMMARY_MAX_ITEMS: usize = 5;

// Longer names are middle-elided so one name cannot widen the dialog.
const DELETE_SUMMARY_NAME_MAX: usize = 48;

// Works on `char`s and keeps both ends so the extension stays visible.
fn elide_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= DELETE_SUMMARY_NAME_MAX {
        return name.to_owned();
    }
    let keep = DELETE_SUMMARY_NAME_MAX - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let start: String = chars[..head].iter().collect();
    let end: String = chars[chars.len() - tail..].iter().collect();
    format!("{start}\u{2026}{end}")
}

// One bounded item line: `Folder “name”` for a directory, `“name”` otherwise,
// with the name middle-elided.
fn summary_item_line(name: &str, is_dir: bool) -> String {
    let name = elide_name(name);
    if is_dir {
        crate::i18n::format("files-folder-name-63be78f8", &[("name", (name))])
    } else {
        crate::i18n::format("files-quoted-name", &[("name", name)])
    }
}

// At most `DELETE_SUMMARY_MAX_ITEMS` lines, then `+N items` with the exact
// remainder so the summary matches the title's count.
fn summary_item_lines(items: &[(String, bool)]) -> Vec<String> {
    let total = items.len();
    let shown = total.min(DELETE_SUMMARY_MAX_ITEMS);
    let mut lines: Vec<String> = items[..shown]
        .iter()
        .map(|(name, is_dir)| summary_item_line(name, *is_dir))
        .collect();
    let remaining = total - shown;
    if remaining > 0 {
        lines.push(crate::i18n::count("files-more-items", remaining as u64));
    }
    lines
}

fn delete_warning(count: usize) -> &'static str {
    if count == 1 {
        crate::i18n::tr("This item will be deleted from your computer and cannot be recovered.")
    } else {
        crate::i18n::tr("These items will be deleted from your computer and cannot be recovered.")
    }
}

mod extensions;
#[path = "dialog/history_dialogs.rs"]
mod history_dialogs;
#[path = "dialog/present.rs"]
mod present;
#[path = "dialog/properties.rs"]
mod properties;
#[path = "dialog/search_dialogs.rs"]
mod search_dialogs;
#[path = "dialog/share_dialogs.rs"]
mod share_dialogs;
#[path = "dialog/storage_dialogs.rs"]
mod storage_dialogs;
pub(crate) use storage_dialogs::{
    CLOUD_TITLE, CONNECT_TITLE, FORMAT_TITLE, SIGN_IN_TITLE, UNLOCK_TITLE, cloud, format, location,
    location_title, sign_in_password_id, unlock, unlock_passphrase_id,
};

pub(crate) use history_dialogs::{BROWSE_TITLE, VERSIONS_TITLE, browse_history, previous_versions};
use present::present_sized;
pub(crate) use present::{
    Action, FOLDERS_FIRST_DESCRIPTION, RECURSIVE_SIZE_COMPUTING, SHOW_HIDDEN_DESCRIPTION,
    SINGLE_CLICK_DESCRIPTION, SINGLE_CLICK_LABEL, SORT_KEY_LABELS, SORT_KEYS, SORT_ORDER_LABELS,
    VIEW_MODE_LABELS, VIEW_MODES, VISIBLE_COLUMNS_LABEL, delete_confirm, device_confirm,
    mount_question, present, sort_key_index, view_mode_index,
};
pub(crate) use present::{about_page_body, extensions_page_body};
pub(crate) use properties::{BackupInfo, VIEW_SETTINGS_SCOPE, properties};
pub(crate) use search_dialogs::{FILTER_DATES, FILTER_TYPES};
pub(crate) use search_dialogs::{bulk_rename, search_filters_surface};
pub(crate) use share_dialogs::{
    add_download, all_activity, attach, conflicts, folder_color, open_with, select_matching,
};

pub(crate) fn shortcuts<'a>() -> Element<'a, Msg> {
    use enzui_widgets::shortcuts_dialog::enz_shortcuts_dialog;
    let entries = crate::keybindings::shortcut_dialog_entries();
    enz_shortcuts_dialog::<Msg>(true, entries)
        .title(crate::i18n::tr("Keyboard Shortcuts"))
        .into_surface(Some(Box::new(|| Msg::CloseShortcuts)))
}
