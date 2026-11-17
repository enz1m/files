use super::*;

// The breadcrumb and its editor share the same outside-click authority.
pub(crate) fn titlebar_mode_band<'a>(
    control: Element<'a, Msg>,
    model: &'a Model,
) -> Element<'a, Msg> {
    let band: Element<'a, Msg> = container(control)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center)
        .into();
    enzui_widgets::breadcrumb::track_edit_bounds(band, &model.breadcrumb_edit_bounds)
}

pub(crate) const MIN_TRAIL_CRUMB_WIDTH: f32 = 120.0;

pub(crate) fn toolbar_actions<'a>(model: &'a Model) -> Vec<ActionItem<'a, Msg>> {
    let (view_icon, view_label, next_view) = if model.view_mode == ViewMode::List {
        (
            "list",
            crate::i18n::tr("List view — switch to grid"),
            ViewMode::Grid,
        )
    } else {
        (
            "layout-grid",
            crate::i18n::tr("Grid view — switch to list"),
            ViewMode::List,
        )
    };
    let checked = |value| {
        if value {
            enzui_core::menu_model::CheckState::Checked
        } else {
            enzui_core::menu_model::CheckState::Unchecked
        }
    };
    vec![
        ActionItem::icon(
            "view.search",
            "search",
            crate::i18n::tr("Search"),
            Msg::ToggleSearch,
        )
        .check(checked(model.search_active))
        .group("view"),
        ActionItem::icon(
            "view.toggle",
            view_icon,
            view_label,
            Msg::SetViewMode(next_view),
        )
        .group("view"),
        ActionItem::command(
            Descriptor::new("file.paste").with_presentation(Presentation::Compact),
            Action::new("file.paste", Msg::PasteHere)
                .with_label(crate::i18n::tr("Paste"))
                .with_icon(Icon::named("clipboard-paste"))
                .with_enabled(model.can_paste()),
        )
        .group("view"),
        // Settings restores focus to this stable inline invoker when its dialog closes.
        ActionItem::with_inline(
            Descriptor::new("view.settings").with_presentation(Presentation::Compact),
            Action::new("view.settings", Msg::ToggleViewSettings)
                .with_label(crate::i18n::tr("View settings"))
                .with_icon(Icon::named("settings")),
            move |action, _| {
                enz_toolbar_icon("settings", Msg::ToggleViewSettings)
                    .label(action.label())
                    .active(model.view_settings_open)
                    .id(view_settings_invoker_id())
                    .into_element()
            },
        )
        .check(checked(model.view_settings_open))
        .group("panels"),
        ActionItem::icon(
            "view.details",
            "panel-right",
            crate::i18n::tr(TOGGLE_DETAILS_LABEL),
            Msg::ToggleDetails,
        )
        .check(checked(model.details_open))
        .group("panels"),
    ]
}

pub(crate) fn shell_tokens_for() -> ShellTokens {
    ShellTokens {
        // The Places rail stays fixed so labels and pin gestures do not reflow.
        sidebar_width: SIDEBAR_WIDTH,
        sidebar_width_adaptive: false,
        ..ShellTokens::flush_page()
    }
}
