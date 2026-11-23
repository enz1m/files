// The list view: sortable Name / Size / Modified / Type columns over keyed
// rows. Each header starts at the x-origin of its cells, and the sort glyph is
// vertically centred on the header label.

use enzui_core::drag::DragOperation;
use enzui_core::drag_autoscroll::AutoScroll;
use enzui_core::table::ColumnKey;
use enzui_core::virtual_sequence::{Anchor, HeightModel, ItemProvider, RealizationWindow};
use enzui_theme::{EnzTheme, Palette};
use enzui_widgets::context_target::enz_context_target;
use enzui_widgets::drag_autoscroll_host::enz_drag_autoscroll_host;
use enzui_widgets::drag_drop_host::DragHostChannel;
use enzui_widgets::drag_item_surface::{DragSurfaceItem, enz_drag_item_surface};
use enzui_widgets::enz_text::TextSize;
use enzui_widgets::icon_label::{IconLabelA11y, IconLabelDisplay, enz_icon_label};
use enzui_widgets::list_row_surface::{ListRowSurfaceTokens, enz_list_row_surface};
use enzui_widgets::marquee_area::{MarqueeTokens, enz_marquee_area};
use enzui_widgets::table_header_cell::{EnzTableHeaderCellAlign, enz_table_header_cell};
use enzui_widgets::table_text_cell::{EnzTableTextAlign, enz_table_text_cell};
use enzui_widgets::text_measure::{MeasureTypography, enz_text_measure_width};
use enzui_widgets::virtual_list::enz_virtual_list;
use iced::font::Weight;
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length};

use crate::drag_drop;
use crate::message::Msg;
use crate::model::{FileEntry, Model, SortKey};
use crate::services;

// The virtual scroller's stable id, which the autoscroll host drives during a
// drag.
pub(crate) const LIST_SCROLLER_ID: &str = "files-app-list-scroller";

// The row and header left inset is the `md` spacing rung, read at both sites.
// Row icon size. No `SizingTokens` rung matches: 18 fits the compact 30px row,
// where 16 leaves too much whitespace and 20 crowds it.
pub(crate) const ICON_SIZE: f32 = 16.0;

// The gutter to the right of each content-sized metadata column, from the `lg`
// spacing rung.
//
// Metadata columns are as wide as their widest header or cell plus this gutter,
// and every remaining pixel goes to Name, as in Nautilus and Finder.
fn meta_gutter(theme: &EnzTheme) -> f32 {
    theme.spacing.lg()
}

// Width reserved on every metadata header for the sort chevron and its gap, so a
// column does not reflow when it becomes the active sort.
//
// Mirrors [`enz_table_header_cell`]: `row![].spacing(6)` and the chevron icon's
// `.size(11.0)` layout box. Update this if either changes.
const HEADER_CHEVRON_RESERVE: f32 = 6.0 + 11.0;

fn metadata_column_widths_from_cells(
    cells: [f32; 3],
    theme: &EnzTheme,
    header_w: impl Fn(SortKey) -> f32,
) -> [f32; 3] {
    let gutter = meta_gutter(theme);
    [
        header_w(SortKey::Size).max(cells[0]).ceil() + gutter,
        header_w(SortKey::Modified).max(cells[1]).ceil() + gutter,
        header_w(SortKey::Kind).max(cells[2]).ceil() + gutter,
    ]
}

fn cached_metadata_column_widths(model: &Model, theme: &EnzTheme) -> [f32; 3] {
    let header_typo = MeasureTypography::new(TextSize::Caption)
        .pin_size_px(12.0)
        .weight(Weight::Semibold);
    // Measured folder sizes are absent from the cached cell widths, so they widen
    // the Size column here.
    let mut cells = model.visible_provider.metadata_cell_widths;
    let cell_typo = MeasureTypography::new(TextSize::Caption);
    cells[0] = model
        .folder_sizes
        .values()
        .map(|size| enz_text_measure_width(&crate::folder_size::label(*size), cell_typo))
        .fold(cells[0], f32::max);
    metadata_column_widths_from_cells(cells, theme, |key| {
        enz_text_measure_width(key.label(), header_typo) + HEADER_CHEVRON_RESERVE
    })
}
// Row and header share the marquee coordinate scale and clear the 24px hit-target floor.
pub(crate) const ROW_HEIGHT: f32 = 30.0;

// The minimum readable Name column width; below it the trailing metadata
// columns are dropped, so the columns yield before the name does.
//
// The decision subtracts the measured metadata widths and the row's real padding
// and gaps from `content_width`.
pub(crate) const MIN_NAME_COL: f32 = 120.0;

// The id namespace every list row derives its widget id under.
//
// Shared with [`crate::context_menu::list_row_id`], which recomputes the id so a
// dismissed context menu can restore focus; a mismatch fails silently. Ids are
// keyed on the path, because recursive search can show duplicate basenames.
pub(crate) const LIST_ROW_ID_PREFIX: &str = "files-app-list-row";

// Resolves optional columns as width contracts: Type yields first, then
// Modified, then Size; Name keeps its readable floor.
pub(crate) fn resolved_visible_columns(
    content_width: f32,
    requested: [bool; 3],
    widths: [f32; 3],
    theme: &EnzTheme,
) -> [bool; 3] {
    use enzui_widgets::enz_table::{FoldCandidate, plan_columns, priority_order};

    let outer = ListRowSurfaceTokens::from_palette(&theme.palette).outer_gutter;
    let inner = (content_width - 2.0 * outer).max(0.0);
    let mut candidates = vec![FoldCandidate {
        column: 0,
        width: MIN_NAME_COL + 2.0 * theme.spacing.md(),
    }];
    candidates.extend(
        widths
            .iter()
            .enumerate()
            .filter(|(index, _)| requested[*index])
            .map(|(index, width)| FoldCandidate {
                column: index + 1,
                width: *width + theme.spacing.sm(),
            }),
    );
    // Most important first, so the shared planner folds Type first and Name never.
    let declared = [
        crate::i18n::tr("Name"),
        crate::i18n::tr("Size"),
        crate::i18n::tr("Modified"),
        crate::i18n::tr("Type"),
    ]
    .map(ColumnKey::new);
    let order = priority_order(&declared, &candidates, |key| match key.as_str() {
        "Name" => Some(0),
        "Size" => Some(1),
        "Modified" => Some(2),
        "Type" => Some(3),
        _ => None,
    });
    let plan = plan_columns(&candidates, &order, inner);
    let mut shown = [false; 3];
    for column in plan.visible.into_iter().filter(|column| *column > 0) {
        shown[column - 1] = true;
    }
    shown
}

// Every list row is exactly one line tall.
//
// Hidden columns are not folded onto extra lines; their values remain in the
// Details inspector and the Properties dialog.
pub(crate) fn resolved_row_height(_requested: [bool; 3], _visible: [bool; 3]) -> f32 {
    ROW_HEIGHT
}

// A list length at the list's zoom level, in whole pixels.
fn zoomed(model: &Model, base: f32) -> f32 {
    (base * model.list_zoom_factor()).round()
}

// The list's icon slot: the glyph size, or the larger theme artwork slot.
pub(crate) fn list_icon_size(model: &Model) -> f32 {
    zoomed(model, crate::icon::slot_size(model, ICON_SIZE))
}

pub(crate) fn row_height_for(model: &Model, theme: &EnzTheme, content_width: f32) -> f32 {
    let requested = [
        model.column_visible(crate::model::MetaColumn::Size),
        model.column_visible(crate::model::MetaColumn::Modified),
        model.column_visible(crate::model::MetaColumn::Kind),
    ];
    let widths = cached_metadata_column_widths(model, theme);
    zoomed(
        model,
        resolved_row_height(
            requested,
            resolved_visible_columns(content_width, requested, widths, theme),
        ),
    )
}

// [`view`] registering on `channel`, whose host the caller places: the
// window's, so a row can also be dropped on a breadcrumb crumb.
pub(crate) fn view_hosted_by<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    content_width: f32,
    viewport_height: f32,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    let palette = &theme.palette;

    if model.visible_provider.len() == 0 {
        return background_context_target(empty_state(model, palette), model);
    }

    // Cell maxima are cached in the projection when directory, filter or sort
    // changes, so a frame reads three values instead of rescanning entries.
    let meta_widths = cached_metadata_column_widths(model, theme);
    // A hidden column contributes no width and is not pushed onto any row.
    let requested_cols = [
        model.column_visible(crate::model::MetaColumn::Size),
        model.column_visible(crate::model::MetaColumn::Modified),
        model.column_visible(crate::model::MetaColumn::Kind),
    ];
    let visible_cols = resolved_visible_columns(content_width, requested_cols, meta_widths, theme);
    let row_height = zoomed(model, resolved_row_height(requested_cols, visible_cols));
    let compact = false; // framework priority folding drops columns one at a time
    let outer_gutter = ListRowSurfaceTokens::from_palette(&theme.palette).outer_gutter;
    let inner_width = (content_width - 2.0 * outer_gutter).max(0.0);

    // The builder runs only for the realised window plus five rows of overscan.
    let virtual_rows: Element<'a, Msg> = enz_virtual_list(
        &model.visible_provider,
        row_height,
        move |index, projected| match model.projected_entry(projected) {
            Some(entry) => entry_row(
                index,
                entry,
                model,
                theme,
                RowPresentation {
                    compact,
                    meta_widths,
                    visible_cols,
                    row_height,
                },
            ),
            None => container(iced::widget::Space::new()).into(),
        },
    )
    // Also re-seeds the scrollable when a modal or sidebar change re-parents the
    // view (`Scrollable::initial_offset`).
    .scroll_offset(model.list_scroll_y)
    .viewport_height(viewport_height)
    .overscan(5)
    .id(iced::widget::Id::new(LIST_SCROLLER_ID))
    .scrollbar_tokens(super::widgets::directory_scrollbar_tokens())
    .on_scroll(|viewport| Msg::ListScrolled(viewport.absolute_offset().y))
    .height(Length::Fill)
    .width(Length::Fill)
    .into();

    // Marquee geometry uses the virtual list's realised band, in viewport-relative
    // coordinates because the marquee wraps the virtual scroller.
    let realised = realised_range_with_height(
        model.visible_provider.len() as usize,
        model.list_scroll_y,
        viewport_height,
        row_height,
    );
    let mut item_rects = Vec::with_capacity(realised.len());
    let mut item_ids = Vec::with_capacity(realised.len());
    let mut selected_ids = Vec::new();
    // The same band drives the drag surface, so a drag never builds an offscreen
    // row or schedules an offscreen thumbnail.
    let mut drag_items: Vec<DragSurfaceItem> = Vec::with_capacity(realised.len());
    for index in realised {
        let Some(projected) = model.visible_provider.item(index as u64) else {
            continue;
        };
        let Some(entry) = model.projected_entry(&projected) else {
            continue;
        };
        let rect = [
            0.0,
            index as f32 * row_height - model.list_scroll_y,
            inner_width,
            row_height,
        ];
        item_rects.push(rect);
        let id = drag_drop::path_to_id(&entry.path);
        item_ids.push(id.clone());
        if model.is_selected(&entry.path) {
            selected_ids.push(id.clone());
        }
        drag_items.push(
            DragSurfaceItem::new(
                id,
                iced::Rectangle {
                    x: rect[0],
                    y: rect[1],
                    width: rect[2],
                    height: rect[3],
                },
                entry.name.clone(),
            )
            .drop_target(drag_drop::accepts_drop(entry.kind)),
        );
    }
    // The drag carries the full selection, including offscreen rows; the marquee's
    // highlight set stays realised-only.
    let drag_selected = drag_drop::selected_ids_in_order(model);

    let dragged: Element<'a, Msg> = enz_drag_item_surface(
        channel,
        virtual_rows,
        drag_drop::DIRECTORY_COLLECTION,
        drag_items,
    )
    .selected(drag_selected)
    .accepts_background(true)
    // The row's focus_button captures pointer-down only to take focus.
    .allows_captured_press(true)
    .source_capabilities(drag_drop::source_capabilities())
    .offering(drag_drop::files_payload())
    .accepting(drag_drop::files_payload())
    .destination_operations(vec![DragOperation::Move, DragOperation::Copy])
    .into();

    let body: Element<'a, Msg> = enz_marquee_area(dragged)
        .items(item_rects)
        .ids(item_ids)
        .selected_ids(selected_ids)
        .on_preview_keyed(Msg::MarqueePreview)
        .on_commit_keyed(Msg::MarqueeResolved)
        .on_background_click(Msg::ClearSelection)
        .highlight_covered(false)
        .tokens(MarqueeTokens::from_palette(palette))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    // Autoscroll drives the existing scroller by id; the drag/drop host owns the
    // one `DragSession` and publishes the typed `ItemDropRequest`.
    let body: Element<'a, Msg> = enz_drag_autoscroll_host(
        body,
        iced::widget::Id::new(LIST_SCROLLER_ID),
        AutoScroll::new(30.0, 400.0),
    )
    // The model owns the scroll offset. Autoscroll operates the scrollable without
    // `notify_scroll`, so `.on_scroll(..)` does not fire; without this the next
    // view re-seeds the stale offset and an edge-held drag jitters.
    .on_scrolled(Msg::ListScrolled)
    .width(Length::Fill)
    .height(Length::Fill)
    .into();
    let body = background_context_target(body, model);
    let body = enzui_widgets::track_bounds(body, &model.directory_view_bounds);

    let stack = column![
        header(model, theme, compact, meta_widths, visible_cols),
        body
    ]
    .spacing(theme.spacing.space0)
    .width(Length::Fill)
    .height(Length::Fill);

    // Rows get their outer inset from list_row_surface; the header keeps its own.
    stack.into()
}

fn realised_range_with_height(
    item_count: usize,
    scroll_y: f32,
    viewport_height: f32,
    row_height: f32,
) -> std::ops::Range<usize> {
    let geometry = HeightModel::new(item_count, row_height);
    RealizationWindow::new(5).realize(
        &geometry,
        &Anchor::from_scroll_top(&geometry, scroll_y),
        viewport_height,
    )
}

// The column header row, built from [`enz_table_header_cell`].
//
// Each cell publishes `Role::ColumnHeader` with `aria-sort` (also in the
// description, which AT-SPI reads) and draws the chevron from the same state.
fn header<'a>(
    model: &Model,
    theme: &EnzTheme,
    compact: bool,
    meta_widths: [f32; 3],
    visible_cols: [bool; 3],
) -> Element<'a, Msg> {
    let palette = &theme.palette;
    // The Name label starts where row names start (`ROW_PAD_X + ICON_SIZE +
    // icon_text_gap`); the row container supplies `ROW_PAD_X`.
    let name = header_cell(
        SortKey::Name,
        model,
        theme,
        Length::Fill,
        name_inset(model, theme),
    );
    let mut header_row = row![name]
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.sm())
        .align_y(Alignment::Center)
        .width(Length::Fill);
    if !compact {
        // The rows' content-sized widths keep labels over their cells; hidden
        // columns get no header cell.
        if visible_cols[0] {
            header_row = header_row.push(header_cell(
                SortKey::Size,
                model,
                theme,
                Length::Fixed(meta_widths[0]),
                0.0,
            ));
        }
        if visible_cols[1] {
            header_row = header_row.push(header_cell(
                SortKey::Modified,
                model,
                theme,
                Length::Fixed(meta_widths[1]),
                0.0,
            ));
        }
        if visible_cols[2] {
            header_row = header_row.push(header_cell(
                SortKey::Kind,
                model,
                theme,
                Length::Fixed(meta_widths[2]),
                0.0,
            ));
        }
    }

    let _ = palette; // header row paints no fill at rest — see the comment below.
    container(header_row)
        .height(Length::Fixed(ROW_HEIGHT))
        .width(Length::Fill)
        .padding(
            iced::Padding::ZERO
                .left(theme.spacing.md())
                .right(theme.spacing.md()),
        )
        .align_y(Alignment::Center)
        // No fill at rest: the content pane is transparent, so an opaque header
        // reads as a band. Focus rings and `aria-sort` come from the cells;
        // `list_header_columns_still_publish_aria_sort` guards the semantics.
        .into()
}

// The gap between a row's icon and its name label: the `sm` spacing rung.
pub(crate) fn icon_text_gap(theme: &EnzTheme) -> f32 {
    theme.spacing.sm()
}

// Extra left inset aligning the Name header label with row file names, which
// sit after icon and gap.
pub(crate) fn name_inset(model: &Model, theme: &EnzTheme) -> f32 {
    list_icon_size(model) + icon_text_gap(theme)
}

// One sortable header cell; `lead_pad` is the Name cell's extra alignment inset.
fn header_cell<'a>(
    key: SortKey,
    model: &Model,
    theme: &EnzTheme,
    width: Length,
    lead_pad: f32,
) -> Element<'a, Msg> {
    let label = key.label();
    let active = model.sort_key == key;

    // No tooltip: the visible labels already name the control, and sort state is
    // in the cell's accessible description.
    let cell: Element<'a, Msg> = enz_table_header_cell(label.to_owned())
        .align(if crate::i18n::rtl() {
            EnzTableHeaderCellAlign::Right
        } else {
            EnzTableHeaderCellAlign::Left
        })
        .sortable(true)
        .sort_active(active)
        .sort_ascending(model.sort_ascending)
        .header_height(ROW_HEIGHT)
        // The row container carries the `md` inset, so only Name's alignment
        // inset is added; `0.0` is honoured literally.
        .cell_padding_x(lead_pad)
        .text_color(if active {
            theme.palette.text
        } else {
            theme.palette.text_muted
        })
        .accent_color(theme.palette.accent)
        .on_sort(Msg::SortBy(key))
        .id_prefix("files-app-list-header")
        .into_element();

    container(cell)
        .width(width)
        .align_y(Alignment::Center)
        .into()
}

struct RowPresentation {
    compact: bool,
    meta_widths: [f32; 3],
    visible_cols: [bool; 3],
    row_height: f32,
}

fn entry_row<'a>(
    index: u64,
    entry: &FileEntry,
    model: &Model,
    theme: &EnzTheme,
    presentation: RowPresentation,
) -> Element<'a, Msg> {
    let RowPresentation {
        compact,
        meta_widths,
        visible_cols,
        row_height,
    } = presentation;
    let palette = &theme.palette;
    let selected = model.is_selected(&entry.path);

    // The rename field sits outside the row surface so it receives pointer and key
    // input.
    if let Some(state) = model
        .inline_rename
        .as_ref()
        .filter(|state| state.targets.first() == Some(&entry.path))
    {
        let icon = crate::icon::entry_element(
            model,
            entry,
            palette,
            crate::icon::list_row_backdrop(palette, selected),
            list_icon_size(model),
        );
        // No row surface here, so the gutter is part of the inset: the icon
        // stays where the other rows put theirs.
        let inset = ROW_GUTTER + theme.spacing.md() - ROW_GUTTER_EXTRA;
        return container(
            row![icon, crate::view::widgets::inline_rename_field(state)]
                .mirrored(crate::i18n::rtl())
                .spacing(icon_text_gap(theme))
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .height(Length::Fixed(row_height))
        .width(Length::Fill)
        .padding(iced::Padding::ZERO.left(inset).right(inset))
        .align_y(Alignment::Center)
        .into();
    }

    // Adjacent selected visible rows form one pill; `selection_group` owns the
    // corner geometry, but only the collection knows display order, so the
    // neighbours' selection is resolved here.
    let neighbour_selected = |neighbour: Option<u64>| -> bool {
        neighbour
            .and_then(|i| model.visible_provider.item(i))
            .and_then(|projected| model.projected_entry(&projected))
            .is_some_and(|e| model.is_selected(&e.path))
    };
    let group_above = neighbour_selected(index.checked_sub(1));
    let group_below = neighbour_selected(index.checked_add(1));

    // Decorative: the row surface already announces the name. The reserved icon
    // slot keeps every name at one x-origin, and long names elide.
    let primary: Element<'a, Msg> = row![
        crate::icon::entry_element(
            model,
            entry,
            palette,
            crate::icon::list_row_backdrop(palette, selected),
            list_icon_size(model)
        ),
        enz_icon_label("", entry.name.clone())
            .display(IconLabelDisplay::TitleOnly)
            .size(TextSize::Body)
            .text_color(palette.text)
            .a11y(IconLabelA11y::Decorative)
            .fill(true),
    ]
    .mirrored(crate::i18n::rtl())
    .spacing(icon_text_gap(theme))
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into();
    let meta = |s: String| -> Element<'a, Msg> {
        enz_table_text_cell(s)
            .align(if crate::i18n::rtl() {
                EnzTableTextAlign::Right
            } else {
                EnzTableTextAlign::Left
            })
            .body_size(TextSize::Caption.px())
            .cell_padding_x(0.0)
            .text_color(palette.text_muted)
            .into_element()
    };
    // Name is the flexible child, so narrowing elides the name instead of pushing
    // metadata off the row; header and rows share one `visible_cols` resolution.
    let _ = index;
    let values = [
        services::format_size(entry),
        services::format_modified(entry),
        entry.kind.label(&entry.mime),
    ];
    let mut content = row![primary]
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.sm())
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fixed(row_height));
    if !compact {
        for column in 0..3 {
            if visible_cols[column] {
                let cell = if column == 0 && matches!(entry.kind, crate::model::EntryKind::Dir) {
                    folder_size_cell(entry, model, theme, values[0].clone())
                } else {
                    meta(values[column].clone())
                };
                content = content.push(
                    container(cell)
                        .width(Length::Fixed(meta_widths[column]))
                        .align_y(Alignment::Center),
                );
            }
        }
    }

    // The highlight stands `ROW_GUTTER` in from the pane's sides; the content
    // inset gives the same amount back, so names and columns stay where the
    // header puts them.
    let row_el = container(content)
        .height(Length::Fixed(row_height))
        .width(Length::Fill)
        .padding(
            iced::Padding::ZERO
                .left(theme.spacing.md() - ROW_GUTTER_EXTRA)
                .right(theme.spacing.md() - ROW_GUTTER_EXTRA),
        )
        .align_y(Alignment::Center);

    // The reducer decides plain, Ctrl, Shift or double click from
    // `Model::last_row_press`: `mouse_area::on_double_click` misses presses the
    // row's `focus_button` captured. Enter opens via `.on_enter`; Space selects
    // and previews via `.on_space`, so it never arms the double-click latch.
    let path_for_click = entry.path.clone();
    let kind_for_click = entry.kind;

    // `Role::ListItem` row chrome with keyboard activation, focus ring and
    // translucent hover/pressed overlays.
    let path_for_enter = entry.path.clone();
    let kind_for_enter = entry.kind;
    let path_for_space = entry.path.clone();
    let path = entry.path.clone();
    let content = enz_list_row_surface::<Msg>(row_el, Msg::Noop)
        .content_padding(0.0)
        .outer_gutter(ROW_GUTTER)
        .on_activate_with_modifiers(move |m| Msg::RowClickPrimary {
            path: path_for_click.clone(),
            kind: kind_for_click,
            command: m.command(),
            shift: m.shift(),
        })
        // Enter and the AT default action open directly, bypassing the latch.
        .on_enter(move || Msg::OpenEntry(path_for_enter.clone(), kind_for_enter))
        // Space selects and previews, and never reaches the double-click
        // deadband.
        .on_space(move || Msg::SpaceOnRow(path_for_space.clone()))
        .selectable(true)
        .selected(selected)
        .selection_group(group_above, group_below)
        .label(crate::icon::entry_accessible_name(model, entry))
        .row_height(row_height)
        .tokens(ListRowSurfaceTokens::from_palette(palette))
        // Ids derive from the path, not the aliasing display name.
        .id_prefix(LIST_ROW_ID_PREFIX)
        .id(crate::context_menu::list_row_id(&entry.path))
        .context_id(crate::context_menu::list_context_id(&entry.path))
        .width(Length::Fill)
        // The row-owned secondary-press capture hugs exactly this row.
        .on_context(move |at| Msg::OpenContextMenu(path.clone(), at))
        .context_label(crate::i18n::format(
            "files-actions-a9474105",
            &[("arg0", (entry.name).clone())],
        ))
        .into_element();
    super::native_transfer::entry(model, entry, content)
}

// A folder's Size cell: a dash that becomes Calculate Size under the pointer,
// then the running total with Stop, then the total.
// How far a row's hover and selection highlight stands in from the pane's
// sides: the list row's default gutter plus [`ROW_GUTTER_EXTRA`].
const ROW_GUTTER: f32 = enzui_widgets::list_row_surface::DEFAULT_OUTER_GUTTER + ROW_GUTTER_EXTRA;
// The gutter added to the list row's default, taken back from the content
// inset so nothing inside the row moves.
const ROW_GUTTER_EXTRA: f32 = 4.0;

fn folder_size_cell<'a>(
    entry: &FileEntry,
    model: &Model,
    theme: &EnzTheme,
    unmeasured: String,
) -> Element<'a, Msg> {
    use crate::folder_size::{self, FolderSize};
    use enzui_widgets::hover_group::{HoverGroupStyle, enz_hover_group_with};
    use enzui_widgets::toolbar_icon::enz_toolbar_icon;

    let size = model.folder_sizes.get(&entry.path).copied();
    let rest = size.map_or(unmeasured, folder_size::label);
    if let Some(FolderSize::Measured(_)) = size {
        return meta_text(rest, &theme.palette);
    }
    let (icon, label, message) = match size {
        Some(_) => (
            "x",
            crate::i18n::tr(folder_size::STOP_LABEL),
            Msg::CancelFolderMeasure(entry.path.clone()),
        ),
        None => (
            folder_size::CALCULATE_ICON,
            crate::i18n::tr(folder_size::CALCULATE_LABEL),
            Msg::MeasureFolder(entry.path.clone()),
        ),
    };
    let action_id = enzui_core::identity::part_id(
        &crate::context_menu::list_row_id(&entry.path),
        "folder-size-cell",
    );
    let control = theme.sizing.icon_md + theme.spacing.xs();
    let glyph = theme.sizing.icon_sm;
    let palette = theme.palette.clone();
    // The unmeasured dash sits centred in the button's own slot, so the
    // calculator glyph that replaces it on hover appears in the same place
    // rather than a few pixels to its right.
    let unmeasured = size.is_none();
    enz_hover_group_with(move |hovered| -> Element<'a, Msg> {
        if hovered {
            enz_toolbar_icon(icon, message.clone())
                .id(action_id.clone())
                .size(control, glyph)
                .aria_label(label)
                .tooltip(label)
                .into()
        } else if unmeasured {
            container(
                enz_table_text_cell(rest.clone())
                    .align(EnzTableTextAlign::Center)
                    .body_size(TextSize::Caption.px())
                    .cell_padding_x(0.0)
                    .text_color(palette.text_muted)
                    .into_element(),
            )
            .width(Length::Fixed(control))
            .into()
        } else {
            meta_text(rest.clone(), &palette)
        }
    })
    .style(|_, _| HoverGroupStyle::NONE)
    // The whole cell reveals the action, not just the dash's few pixels.
    .fill_width()
    .into_element()
}

fn meta_text<'a>(value: String, palette: &Palette) -> Element<'a, Msg> {
    enz_table_text_cell(value)
        .align(if crate::i18n::rtl() {
            EnzTableTextAlign::Right
        } else {
            EnzTableTextAlign::Left
        })
        .body_size(TextSize::Caption.px())
        .cell_padding_x(0.0)
        .text_color(palette.text_muted)
        .into_element()
}

// The right-click surface for listing space no row occupies.
//
// `enz_context_target` hit-tests its own resolved frame, so a `Fill` frame covers
// the void below the last row; an inner row target that captured the press wins.
fn background_context_target<'a>(content: Element<'a, Msg>, model: &Model) -> Element<'a, Msg> {
    enz_context_target(content, Msg::OpenBackgroundContextMenu)
        .id(iced::advanced::widget::Id::new(
            "files-app-list-background-context",
        ))
        .label(crate::i18n::format(
            "files-background-actions-441af9b7",
            &[("arg0", (model.cwd.display()).to_string())],
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn empty_state<'a>(model: &Model, palette: &Palette) -> Element<'a, Msg> {
    container(super::empty_directory_page(model))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style({
            let bg = palette.background;
            move |_t: &iced::Theme| container::Style {
                background: Some(bg.into()),
                ..Default::default()
            }
        })
        .into()
}
