// Trash screen and rows.

use super::*;
use iced::widget::column;

// The same words as the Trash row menu's command, which it also opens.
pub(crate) const DELETE_TRASH_LABEL: &str = "Delete Permanently\u{2026}";
// Stable identity namespace for Trash rows and their context affordances.
pub(crate) const TRASH_ROW_ID_PREFIX: &str = "files-app-trash-row";

// The Trash screen.
//
// An empty Trash shows no destructive action; a populated Trash lists each
// entry by its origin name and where it came from, read from its
// `.trashinfo` twin.
pub(crate) fn trash_body<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    content_width: f32,
) -> Element<'a, Msg> {
    if model.trash_entries.is_empty() {
        return container(
            trash_empty_page(model)
                .description(crate::i18n::tr(TRASH_EMPTY_DESCRIPTION))
                .into_element(),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();
    }

    // Empty Trash is the page's labelled action, as Computer and Network lay
    // theirs out; its id is where focus returns after the confirmation.
    let header = enz_page_header::<Msg>(crate::i18n::tr(TRASH_TITLE).to_owned())
        .spacing_scale(theme.spacing)
        .primary_action(
            enzui_core::Action::new("files-empty-trash", Msg::EmptyTrash)
                .with_label(crate::i18n::tr(EMPTY_TRASH_LABEL)),
        )
        .primary_action_kind(enzui_widgets::button::ButtonKind::Quiet)
        .into_element();

    let shown = trash_visible_columns(content_width, theme);
    let mut rows: Vec<Element<'a, Msg>> = Vec::with_capacity(model.trash_entries.len() + 2);
    rows.push(header);
    rows.push(column_header(model, theme, shown));
    for entry in &model.trash_entries {
        rows.push(trash_row(entry, model, theme, shown));
    }

    // The same page inset as Computer, Network and the Download Manager.
    enz_scroll_column(rows)
        .spacing(theme.spacing.xs())
        .inset(theme.spacing.space9)
        .into_element()
}

const DELETED_COLUMN: f32 = 120.0;
const SIZE_COLUMN: f32 = 80.0;
const TYPE_COLUMN: f32 = 120.0;
// The trailing restore / delete actions' reserved width, so the columns
// line up with the header whether or not a row shows its actions.
const ACTIONS_COLUMN: f32 = 64.0;

pub(crate) const TRASH_COLUMNS: [&str; 4] = ["Name", "Deleted", "Size", "Type"];

// Which of Deleted, Size and Type fit beside the name at `content_width`.
// As in the directory list, a narrowing window folds the least important
// column first (Type, then Size, then Deleted) and the name always stands.
pub(crate) fn trash_visible_columns(content_width: f32, theme: &EnzTheme) -> [bool; 3] {
    use enzui_widgets::enz_table::{FoldCandidate, plan_columns, priority_order};
    let gutter = enzui_widgets::list_row_surface::DEFAULT_OUTER_GUTTER;
    // The page inset, the row's own padding and the actions it reserves.
    let inner = (content_width
        - 2.0 * (theme.spacing.space9 + gutter + theme.spacing.md())
        - ACTIONS_COLUMN)
        .max(0.0);
    let mut candidates = vec![FoldCandidate {
        column: 0,
        width: crate::view::list::MIN_NAME_COL + crate::view::list::ICON_SIZE + theme.spacing.sm(),
    }];
    candidates.extend(
        [DELETED_COLUMN, SIZE_COLUMN, TYPE_COLUMN]
            .into_iter()
            .enumerate()
            .map(|(index, width)| FoldCandidate {
                column: index + 1,
                width: width + theme.spacing.sm(),
            }),
    );
    let declared = TRASH_COLUMNS.map(enzui_core::table::ColumnKey::new);
    let order = priority_order(&declared, &candidates, |key| {
        TRASH_COLUMNS.iter().position(|name| *name == key.as_str())
    });
    let plan = plan_columns(&candidates, &order, inner);
    let mut shown = [false; 3];
    for column in plan.visible.into_iter().filter(|column| *column > 0) {
        shown[column - 1] = true;
    }
    shown
}

// The empty Trash: the theme's empty bin when it has one, else the glyph.
fn trash_empty_page<'a>(model: &Model) -> enzui_widgets::status_page::EnzStatusPage<'a, Msg> {
    let page = enz_status_page(crate::i18n::tr(TRASH_EMPTY_TITLE));
    match enzui_widgets::file_icon::themed_icon(&["user-trash"], TRASH_EMPTY_ART) {
        Some(art) if model.colored_file_icons => page.artwork(art.into_element(TRASH_EMPTY_ART)),
        _ => page.icon("trash-2"),
    }
}

// The empty-Trash artwork's size: the status page's 64 px glyph, scaled for
// the art's padded canvas.
const TRASH_EMPTY_ART: f32 = 96.0;

// The icon slot of a Trash row: two-line rows take the larger artwork.
fn trash_icon_size(model: &Model) -> f32 {
    if crate::icon::uses_art(model) {
        TRASH_ROW_ART
    } else {
        crate::view::list::ICON_SIZE
    }
}

// Theme artwork beside a two-line Trash row, as in the Downloads rows.
const TRASH_ROW_ART: f32 = 32.0;

fn column_header<'a>(model: &Model, theme: &'a EnzTheme, shown: [bool; 3]) -> Element<'a, Msg> {
    use enzui_widgets::table_header_cell::{EnzTableHeaderCellAlign, enz_table_header_cell};
    let cell = |label: &'static str, width: Length, lead: f32| -> Element<'a, Msg> {
        container(
            enz_table_header_cell(crate::i18n::tr(label).to_owned())
                .align(if crate::i18n::rtl() {
                    EnzTableHeaderCellAlign::Right
                } else {
                    EnzTableHeaderCellAlign::Left
                })
                .cell_padding_x(lead)
                .text_color(theme.palette.text_muted)
                .into_element(),
        )
        .width(width)
        .into()
    };
    let [name, deleted, size, kind] = TRASH_COLUMNS;
    let mut cells = vec![cell(
        name,
        Length::Fill,
        trash_icon_size(model) + theme.spacing.sm(),
    )];
    for (shown, label, width) in [
        (shown[0], deleted, DELETED_COLUMN),
        (shown[1], size, SIZE_COLUMN),
        (shown[2], kind, TYPE_COLUMN),
    ] {
        if shown {
            cells.push(cell(label, Length::Fixed(width), 0.0));
        }
    }
    cells.push(
        iced::widget::Space::new()
            .width(Length::Fixed(ACTIONS_COLUMN))
            .into(),
    );
    container(
        iced::widget::Row::with_children(cells)
            .mirrored(crate::i18n::rtl())
            .spacing(theme.spacing.sm())
            .align_y(Alignment::Center),
    )
    // The rows' own inset: the row surface's outer gutter, then the
    // content padding, so each label sits over its column.
    .padding(Padding::from([
        0.0,
        enzui_widgets::list_row_surface::DEFAULT_OUTER_GUTTER + theme.spacing.md(),
    ]))
    .width(Length::Fill)
    .into()
}

// A `.trashinfo` deletion date (local time, `YYYY-MM-DDThh:mm:ss`) as the
// list's Modified column shows times: `Oct 1, 09:15` this year,
// `Oct 1, 2025` before. Anything unreadable is shown as it is.
#[must_use]
pub(crate) fn deleted_label(date: &str, this_year: i32) -> String {
    let parsed = (|| {
        let (day, clock) = date.split_once('T')?;
        let mut parts = day.split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        let date = enzui_core::civil_date::Date::new(year, month, day)?;
        let localizer = crate::i18n::localizer();
        if year == this_year {
            let mut clock = clock.split(':');
            let hour = clock.next()?.parse().ok()?;
            let minute = clock.next()?.parse().ok()?;
            let time = enzui_core::time_of_day::TimeOfDay::hm(hour, minute)?;
            let mut args = enzui_core::localization::Arguments::new();
            args.set("date", localizer.locale().format_month_day(date).ok()?);
            args.set("time", localizer.locale().format_time(time, false).ok()?);
            Some(localizer.text_with("enzui-file-modified-date-time", &args))
        } else {
            localizer
                .locale()
                .format_date(date, enzui_core::localization::DateStyle::Medium)
                .ok()
        }
    })();
    parsed.unwrap_or_else(|| date.to_owned())
}

fn current_year() -> i32 {
    enzui_core::local_time::current_local_civil_time().map_or(i32::MIN, |now| now.date.year())
}

// One selectable row in the Trash listing.
//
// Restore and permanent delete are icon-only trailing actions that stay
// mounted and accessible but paint only while the row is hovered or focused.
// The row owns the secondary-press hook, so its context menu is keyboard and
// AT routable.
fn trash_row<'a>(
    entry: &'a crate::trash::TrashEntry,
    model: &'a Model,
    theme: &'a EnzTheme,
    shown: [bool; 3],
) -> Element<'a, Msg> {
    let payload = model.trash_payloads.get(&entry.trashed_path).copied();
    let is_dir = payload.is_some_and(|p| p.is_dir);
    let name = enz_text(entry.name.clone())
        .size(TextSize::Body)
        .tone(TextTone::Default)
        .single_line_elided();
    // The original path, where Restore puts it back.
    let origin = enz_text(entry.original_path.display().to_string())
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .single_line_elided();
    let kind = if is_dir {
        crate::model::EntryKind::Dir.label("")
    } else {
        crate::model::EntryKind::File.label(&entry.mime)
    };
    let size = match payload {
        Some(crate::trash::TrashPayload {
            size: Some(bytes), ..
        }) => crate::services::format_bytes(bytes),
        _ => "\u{2014}".to_owned(),
    };
    let meta = |text: String, width: f32| -> Element<'a, Msg> {
        container(
            enz_text(text)
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .single_line_elided(),
        )
        .width(Length::Fixed(width))
        .into()
    };
    let glyph = crate::icon::named_element(model, &entry.name, is_dir, trash_icon_size(model));
    let mut cells: Vec<Element<'a, Msg>> = vec![
        glyph,
        column![name, origin]
            .spacing(theme.spacing.space1)
            .width(Length::Fill)
            .into(),
    ];
    for (shown, text, width) in [
        (
            shown[0],
            deleted_label(&entry.deletion_date, current_year()),
            DELETED_COLUMN,
        ),
        (shown[1], size, SIZE_COLUMN),
        (shown[2], kind, TYPE_COLUMN),
    ] {
        if shown {
            cells.push(meta(text, width));
        }
    }
    let details: Element<'a, Msg> = iced::widget::Row::with_children(cells)
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.sm())
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into();
    let actions = row![
        enz_toolbar_icon(
            "rotate-ccw",
            Msg::RestoreFromTrash(entry.trashed_path.clone())
        )
        .aria_label(crate::i18n::tr(RESTORE_TRASH_LABEL))
        .tooltip(crate::i18n::tr(RESTORE_TRASH_LABEL)),
        enz_toolbar_icon(
            "trash",
            Msg::AskDeleteTrashEntry(entry.trashed_path.clone())
        )
        .aria_label(crate::i18n::tr(DELETE_TRASH_LABEL))
        .tooltip(crate::i18n::tr(DELETE_TRASH_LABEL)),
    ]
    .mirrored(crate::i18n::rtl())
    .spacing(theme.spacing.xs())
    .align_y(Alignment::Center);
    let content = container(details)
        .padding(Padding::from([theme.spacing.xs(), theme.spacing.md()]))
        .width(Length::Fill);
    let path = entry.trashed_path.clone();
    let row_id = crate::context_menu::trash_row_id(&path);
    let context_id = crate::context_menu::trash_context_id(&path);

    // A click only edits selection. Restore and permanent delete are explicit
    // icon buttons (and context-menu actions), never row activation.
    let select_path = entry.trashed_path.clone();
    enz_list_row_surface(
        content,
        Msg::SelectTrashEntry {
            path: select_path.clone(),
            command: false,
            shift: false,
        },
    )
    .content_padding(0.0)
    .on_activate_with_modifiers(move |modifiers| Msg::SelectTrashEntry {
        path: select_path.clone(),
        command: modifiers.command(),
        shift: modifiers.shift(),
    })
    .label(entry.name.clone())
    .selected(model.is_selected(&entry.trashed_path))
    .trailing(actions)
    .trailing_visibility(TrailingVisibility::OnHoverFocusOrSelected)
    .content_height(true)
    .id(row_id)
    .id_prefix(TRASH_ROW_ID_PREFIX)
    .on_context(move |at| Msg::OpenContextMenu(path.clone(), at))
    .context_id(context_id)
    .context_label(crate::i18n::format(
        "files-actions-a9474105",
        &[("arg0", (entry.name).clone())],
    ))
    .width(Length::Fill)
    .into()
}

// Pins the `enz_app_shell` composition through the values `shell_tokens`
// feeds and the geometry the shell resolves: a `SpanningBand` header, the
// app's sidebar width and titlebar height, nonzero gap and radius, and no
