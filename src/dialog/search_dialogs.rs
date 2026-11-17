// Search filters, preview rows, and bulk rename.

use super::*;
use iced::widget::column;

// The search type-facet options in display order; the radio selects by index
// and `filter_type_index` is the inverse.
pub(crate) const FILTER_TYPES: [SearchFilterType; 6] = [
    SearchFilterType::Any,
    SearchFilterType::Folder,
    SearchFilterType::Image,
    SearchFilterType::Document,
    SearchFilterType::Audio,
    SearchFilterType::Video,
];

pub(crate) const FILTER_DATES: [SearchFilterDate; 4] = [
    SearchFilterDate::Any,
    SearchFilterDate::Today,
    SearchFilterDate::Week,
    SearchFilterDate::Month,
];

// The index of `facet` in [`FILTER_TYPES`].
pub(crate) fn filter_type_index(facet: SearchFilterType) -> usize {
    FILTER_TYPES.iter().position(|f| *f == facet).unwrap_or(0)
}

// The index of `facet` in [`FILTER_DATES`].
pub(crate) fn filter_date_index(facet: SearchFilterDate) -> usize {
    FILTER_DATES.iter().position(|f| *f == facet).unwrap_or(0)
}

// The search field's Filters surface: type facet, date facet and recursive
// scope. The framework widget owns placement, focus order and dismissal.
// The narrowest a facet's name column gets before its choices stack below.
const FACET_LABEL_MIN_WIDTH: f32 = 120.0;

pub(crate) fn search_filters_surface<'a>(
    theme: &'a EnzTheme,
    model: &'a Model,
) -> Element<'a, Msg> {
    use enzui_widgets::preferences_row::enz_preferences_row;
    let sp = &theme.spacing;
    let radio_tokens = RadioTokens::from_palette(&theme.palette);

    // Each facet is a settings row: its name at the leading edge, its
    // choices in one line at the trailing edge, as in the platform search
    // and settings panels. A narrow field stacks the choices under the name.
    let choices = |title: &'static str,
                   group: &'static str,
                   selected: usize,
                   on_select: fn(usize) -> Msg,
                   prefix: &'static str|
     -> Element<'a, Msg> {
        enz_preferences_row(crate::i18n::tr(title))
            // A short name needs little room; the choices take the rest
            // and stack under it only when the field is truly narrow.
            .label_min_width(FACET_LABEL_MIN_WIDTH)
            .control(
                enz_radio_group(
                    crate::i18n::tr(title),
                    crate::i18n::radio_labels(group),
                    Some(selected),
                    on_select,
                )
                // The row paints the name; the group keeps it for AT.
                .label_visible(false)
                .orientation(Orientation::Horizontal)
                .spacing(sp.md())
                .tokens(radio_tokens)
                .option_id_prefix(prefix)
                .into_element(),
            )
            .into_element()
    };
    let type_row = choices(
        "File type",
        "type",
        filter_type_index(model.search_filter_type),
        |i| {
            Msg::SetFilterType(
                FILTER_TYPES
                    .get(i)
                    .copied()
                    .unwrap_or(SearchFilterType::Any),
            )
        },
        "files-app-filters-type",
    );
    let date_row = choices(
        "Date modified",
        "date",
        filter_date_index(model.search_filter_date),
        |i| {
            Msg::SetFilterDate(
                FILTER_DATES
                    .get(i)
                    .copied()
                    .unwrap_or(SearchFilterDate::Any),
            )
        },
        "files-app-filters-date",
    );
    let scope = enz_preferences_row(crate::i18n::tr("Search subfolders"))
        .subtitle(crate::i18n::tr(
            "Include every folder below the current location.",
        ))
        .id_prefix("files-app-search-subfolders")
        .switch(
            model.search_scope == crate::model::SearchScope::Everywhere,
            Msg::SetSearchScope,
        )
        .into_element();

    column![scope, type_row, date_row]
        .width(Length::Fill)
        .into()
}

// One preview row, `Original:` over `New:`, plus a notice for unchanged or
// blocked rows.
fn preview_row<'a>(theme: &EnzTheme, row: &crate::bulk_rename::RenameRow) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    let mut col = column![
        enz_labeled_content_compact(crate::i18n::tr(crate::bulk_rename::PREVIEW_LABELS[0]))
            .value(row.original_name.clone())
            .into_element(),
        enz_labeled_content_compact(crate::i18n::tr(crate::bulk_rename::PREVIEW_LABELS[1]))
            .value(row.new_name.clone())
            .into_element(),
    ]
    .spacing(sp.space0);
    if let Some(notice) = row.status.notice() {
        // Blocked rows warn and unchanged rows are muted.
        let tone = if row.status.blocks() {
            BadgeTone::Warning
        } else {
            BadgeTone::Default
        };
        col = col.push(enz_badge(notice).tone(tone).into_element());
    }
    enz_surface(Element::from(col.width(Length::Fill)))
        .tone(SurfaceTone::SurfaceAlt)
        .radius_resolved(|theme| {
            enzui_widgets::theme_lookup::shape_radius(theme, |shape| shape.row).into()
        })
        .padding(sp.sm())
        .width(Length::Fill)
        .into_element()
}

// The bulk-rename dialog.
//
// The preview renders `BulkRename::plan()`, the same plan the reducer applies.
pub(crate) fn bulk_rename<'a>(
    theme: &EnzTheme,
    state: &'a crate::bulk_rename::BulkRename,
    busy: bool,
    window: Size,
) -> Element<'a, Msg> {
    use crate::bulk_rename::{BULK_RENAME_MODES, BulkRenameMode, MAX_PAD, RENAME_TO_LABEL};

    let sp = &theme.spacing;
    let radio_tokens = RadioTokens::from_palette(&theme.palette);
    let plan = state.plan();

    // `enz_radio_group` selects by index through `BULK_RENAME_MODES`.
    let mode_radio = enz_radio_group(
        crate::i18n::tr("Rename rule"),
        crate::i18n::radio_labels("rename"),
        Some(state.mode_index()),
        |i| {
            Msg::SetBulkRenameMode(
                BULK_RENAME_MODES
                    .get(i)
                    .copied()
                    .unwrap_or(BulkRenameMode::FindReplace),
            )
        },
    )
    .orientation(Orientation::Horizontal)
    .spacing(sp.sm())
    .tokens(radio_tokens)
    .option_id_prefix("files-app-bulk-rename-mode")
    .into_element();

    // Only the active mode's fields exist, so no hidden field can be edited.
    let fields: Element<'a, Msg> = match state.mode {
        BulkRenameMode::FindReplace => column![
            enz_preferences_row(crate::i18n::tr("Find"))
                .entry(
                    &state.find,
                    crate::i18n::tr("e.g. IMG_"),
                    Msg::BulkRenameFindChanged,
                )
                .into_element(),
            enz_preferences_row(crate::i18n::tr("Replace"))
                .entry(
                    &state.replace,
                    crate::i18n::tr("e.g. Photo_"),
                    Msg::BulkRenameReplaceChanged,
                )
                .into_element(),
        ]
        .spacing(sp.xs())
        .into(),
        BulkRenameMode::Numbering => column![
            enz_preferences_row(crate::i18n::tr("Template"))
                .subtitle(crate::i18n::tr(crate::bulk_rename::TEMPLATE_TOKENS_HELP))
                .entry(
                    &state.template,
                    crate::bulk_rename::DEFAULT_TEMPLATE,
                    Msg::BulkRenameTemplateChanged,
                )
                .into_element(),
            // Steppers, since values are integers; `min(0)` because a start of 0
            // means 0.
            enz_preferences_row(crate::i18n::tr("Start at"))
                .control(
                    enz_stepper(state.start as i32, |v| {
                        Msg::BulkRenameStartChanged(v.max(0) as u32)
                    })
                    .min(0)
                    .max(9_999)
                    .id_prefix("files-app-bulk-rename-start")
                    .into_element(),
                )
                .into_element(),
            enz_preferences_row(crate::i18n::tr("Step"))
                .control(
                    enz_stepper(state.step as i32, |v| {
                        Msg::BulkRenameStepChanged(v.max(0) as u32)
                    })
                    .min(0)
                    .max(999)
                    .id_prefix("files-app-bulk-rename-step")
                    .into_element(),
                )
                .into_element(),
            enz_preferences_row(crate::i18n::tr("Zero padding"))
                .control(
                    enz_stepper(state.pad as i32, |v| {
                        Msg::BulkRenamePadChanged(v.clamp(0, MAX_PAD as i32) as u32)
                    })
                    .min(0)
                    .max(MAX_PAD as i32)
                    .id_prefix("files-app-bulk-rename-pad")
                    .into_element(),
                )
                .into_element(),
        ]
        .spacing(sp.xs())
        .into(),
    };

    // Bounded, so a large batch cannot push the actions off the surface.
    let preview_rows: Vec<Element<'a, Msg>> =
        plan.rows.iter().map(|r| preview_row(theme, r)).collect();
    let preview: Element<'a, Msg> = if preview_rows.is_empty() {
        enz_text(crate::i18n::tr("Nothing selected."))
            .size(TextSize::Footnote)
            .tone(TextTone::Muted)
            .into_element()
    } else {
        enz_scroll_column(preview_rows)
            .gap(enzui_theme::SpaceToken::Xs)
            .max_content_height(220.0)
            .height(Length::Shrink)
            .into()
    };

    // One file gets a `Rename to:` field; the rule controls are absent.
    let rule: Element<'a, Msg> = if state.is_single() {
        enz_preferences_row(crate::i18n::tr(RENAME_TO_LABEL))
            .entry(
                &state.direct,
                crate::i18n::tr("New file name"),
                Msg::BulkRenameDirectChanged,
            )
            .into_element()
    } else {
        column![mode_radio, fields]
            .spacing(sp.md())
            .width(Length::Fill)
            .into()
    };

    let body: Element<'a, Msg> = column![
        enz_text(state.summary())
            .size(TextSize::Footnote)
            .tone(TextTone::Muted)
            .into_element(),
        enz_preferences_group(rule).into_element(),
        enz_section_header::<Msg>(crate::i18n::tr("Preview")).into_element(),
        preview,
    ]
    .spacing(sp.md())
    .width(Length::Fill)
    .into();

    let mut actions = vec![
        Action::new(
            crate::i18n::tr("Cancel"),
            ButtonKind::Quiet,
            Msg::CancelBulkRename,
        ),
        Action::new(
            crate::i18n::tr("Reset"),
            ButtonKind::Quiet,
            Msg::ResetBulkRename,
        ),
    ];
    // Apply is absent while blocked or busy; `present` has no disabled actions.
    if state.can_apply() && !busy {
        actions.push(Action::new(
            crate::i18n::tr("Rename"),
            ButtonKind::Accent,
            Msg::ApplyBulkRename,
        ));
    }

    present(theme, state.title(), body, actions, window)
}
