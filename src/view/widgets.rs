use iced::Element;

use crate::message::Msg;

pub(crate) const SIDEBAR_WIDTH: f32 = 220.0;

// Stable widget id of the toolbar search field.
//
// `Msg::ToggleSearch` focuses it on the opening edge only; a closed field is not
// rendered.
pub(crate) fn search_field_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("files-app-toolbar-search")
}

// Stable id of the View Settings button, used by the view to tag it and by the
// reducer to restore focus to it when the Settings dialog closes.
pub(crate) fn view_settings_invoker_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("files-app-view-settings-button")
}

// Stable identity shared by the folder trail and its swapped-in editor.
//
// The framework derives every clickable segment below this namespace and
// carries the namespace itself across the trail ↔ text-input mode swap.
pub(crate) const PATH_BAR_ID: &str = "files-app-folder-path";

pub(crate) fn path_bar_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(PATH_BAR_ID)
}

// Stable id of the in-place rename field; only one row edits at once.
pub(crate) fn inline_rename_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("files-app-inline-rename")
}

// The in-place rename field: Enter or a click elsewhere commits, Escape
// cancels, and a blocked name
// shows the planner's reason as the field error.
pub(crate) fn inline_rename_field<'b>(state: &crate::bulk_rename::BulkRename) -> Element<'b, Msg> {
    let plan = state.plan();
    let row = plan.rows.first();
    let original = row.map_or("", |row| row.original_name.as_str());
    let mut field = enzui_widgets::text_field::enz_text_field_owned(
        "",
        original.to_owned(),
        state.direct.clone(),
        Msg::InlineRenameChanged,
    )
    .id(inline_rename_id())
    .aria_label(crate::i18n::format(
        "files-rename-original-888ee08a",
        &[("original", (original).to_owned())],
    ))
    .on_submit(Msg::CommitInlineRename)
    .on_blur(Msg::InlineRenameBlurred);
    if let Some(reason) = row
        .filter(|row| row.status.blocks())
        .and_then(|row| row.status.notice())
    {
        field = field.error(reason);
    }
    enzui_widgets::keyboard_capture::keyboard_capture_unscoped(
        field.into_element(),
        |key, modifiers| {
            (modifiers.is_empty()
                && matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                ))
            .then_some(Msg::CancelInlineRename)
        },
    )
}

// Stable identity of the path bar's Copy control.
pub(crate) fn copy_path_icon_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new("files-app-path-copy")
}

// Minimum grid tile width (px), sized for a preview and a caption filename.
//
// The view fits as many footprints as possible and flexes them to fill the row.
pub(crate) const GRID_TILE_W: f32 = 120.0;
// Grid tile height (px) — see [`GRID_TILE_W`].
pub(crate) const GRID_TILE_H: f32 = 148.0;
// Gap between grid tiles: the spacing scale's `md` rung by value.
//
// A constant because the pure marquee helpers have no theme in scope;
// `grid_gap_is_the_md_spacing_rung` pins the coupling.
pub(crate) const GRID_GAP: f32 = 12.0;

// A generous inward grab band independent of the thin painted thumb.
pub(crate) fn directory_scrollbar_tokens() -> enzui_theme::ScrollbarTokens {
    enzui_theme::ScrollbarTokens {
        hit_width: 28.0,
        ..enzui_theme::ScrollbarTokens::DEFAULT
    }
    .normalized()
}
