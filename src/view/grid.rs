// The grid view: wrapping tiles of thumbnail or icon plus name. Thumbnails
// replace the MIME icon when ready, so tiles never render blank.

use enzui_core::drag::DragOperation;
use enzui_core::drag_autoscroll::AutoScroll;
use enzui_core::virtual_grid::TileGrid;
use enzui_core::virtual_sequence::{Anchor, ItemProvider, PixelOverscan};
use enzui_theme::EnzTheme;
use enzui_widgets::context_target::enz_context_target;
use enzui_widgets::drag_autoscroll_host::enz_drag_autoscroll_host;
use enzui_widgets::drag_drop_host::DragHostChannel;
use enzui_widgets::drag_item_surface::{DragSurfaceItem, enz_drag_item_surface};
use enzui_widgets::enz_text::{TextAlign, TextSize, TextTone, enz_text};
use enzui_widgets::image_widget::{EnzImageFit, enz_image_handle};
use enzui_widgets::marquee_area::{MarqueeTokens, enz_marquee_area};
use enzui_widgets::pressable::{PressableStyle, enz_pressable};
use enzui_widgets::virtual_grid::enz_virtual_grid;
use iced::widget::{column, container};
use iced::{Alignment, Element, Length};

use super::widgets::{GRID_GAP, GRID_TILE_H, GRID_TILE_W};
use crate::drag_drop;
use crate::message::Msg;
use crate::model::{FileEntry, Model};

// The grid scroller's id, which the autoscroll host drives during a drag.
pub(crate) const GRID_SCROLLER_ID: &str = "files-app-grid-scroller";

// Tile glyph size: a third above the theme's `icon_xxl` rung, so an icon or
// thumbnail fills its tile instead of floating in it.
pub(crate) fn tile_glyph_size(theme: &EnzTheme) -> f32 {
    (theme.sizing.icon_xxl * 4.0 / 3.0).round()
}

// Height of a tile's glyph band: `icon_xxl` plus the `xxl` spacing rung.
pub(crate) fn tile_glyph_band(theme: &EnzTheme) -> f32 {
    tile_glyph_size(theme) + theme.spacing.xxl()
}

// The glyph (and thumbnail) size at the grid's zoom level.
pub(crate) fn zoomed_glyph_size(model: &Model, theme: &EnzTheme) -> f32 {
    (crate::icon::slot_size(model, tile_glyph_size(theme)) * model.grid_zoom_factor()).round()
}

// The glyph band at the grid's zoom level.
pub(crate) fn zoomed_glyph_band(model: &Model, theme: &EnzTheme) -> f32 {
    zoomed_glyph_size(model, theme) + theme.spacing.xxl()
}

// The minimum tile width at the grid's zoom level; only the glyph growth is
// added, so the name keeps its width at every level.
pub(crate) fn grid_tile_width(model: &Model, theme: &EnzTheme) -> f32 {
    GRID_TILE_W + zoomed_glyph_size(model, theme) - tile_glyph_size(theme)
}

// [`view`] registering on `channel`, whose host the caller places; see
// [`super::list::view_hosted_by`].
pub(crate) fn view_hosted_by<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    viewport_height: f32,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    if model.visible_provider.len() == 0 {
        let empty = container(super::empty_directory_page(model))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        return background_context_target(empty, model);
    }

    // Realisation, paint, marquee hit boxes, keyboard columns and thumbnail
    // scheduling all read this frame's allocation, never last frame's width.
    let channel = channel.clone();
    let responsive: Element<'a, Msg> = iced::widget::responsive(move |size| {
        let tile_width = grid_tile_width(model, theme);
        let usable = (size.width - 2.0 * GRID_GAP).max(tile_width);
        let tile_height = grid_tile_height(model, theme);
        let resolved = TileGrid::new(tile_width, tile_height)
            .with_spacing(GRID_GAP, GRID_GAP)
            .stretch_for_width(usable);
        model.grid_layout.set(resolved);
        grid_for_geometry(model, theme, viewport_height, resolved, &channel)
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    background_context_target(responsive, model)
}

fn grid_for_geometry<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    viewport_height: f32,
    resolved: enzui_core::virtual_grid::ResolvedTileGrid,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    let cols = resolved.columns.max(1);
    let tile_width = resolved.tile_width;
    let tile_height = resolved.tile_height;
    let geometry = resolved.grid();
    let virtual_grid: Element<'a, Msg> = enz_virtual_grid(
        &model.visible_provider,
        geometry,
        move |_index, projected| match model.projected_entry(projected) {
            Some(entry) => tile(entry, model, theme, tile_height),
            None => container(iced::widget::Space::new()).into(),
        },
    )
    .resolved(resolved)
    // Re-seeds the scrollable after a re-parent.
    .scroll_offset(model.grid_scroll_y)
    .id(iced::widget::Id::new(GRID_SCROLLER_ID))
    .scrollbar_tokens(super::widgets::directory_scrollbar_tokens())
    .viewport_height(viewport_height)
    .overscan_px(tile_height / 2.0)
    .semantics(crate::i18n::tr("Files"))
    .on_scroll(|viewport| Msg::GridScrolled(viewport.absolute_offset().y))
    .height(Length::Fill)
    .width(Length::Fill)
    .into();

    let realised = realised_range_for_geometry(
        model.visible_provider.len(),
        resolved,
        viewport_height,
        model.grid_scroll_y,
    );
    let mut item_rects = Vec::with_capacity(realised.len());
    let mut item_ids = Vec::with_capacity(realised.len());
    let mut selected_ids = Vec::new();
    // Only realised tiles are draggable; reflow re-registers keyed geometry.
    let mut drag_items: Vec<DragSurfaceItem> = Vec::with_capacity(realised.len());
    for index in realised {
        let Some(projected) = model.visible_provider.item(index as u64) else {
            continue;
        };
        let Some(entry) = model.projected_entry(&projected) else {
            continue;
        };
        let col = index % cols;
        let row = index / cols;
        let rect = [
            GRID_GAP + col as f32 * (tile_width + GRID_GAP),
            GRID_GAP + row as f32 * (tile_height + GRID_GAP) - model.grid_scroll_y,
            tile_width,
            tile_height,
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
    // The full selection travels with the drag, including offscreen entries.
    let drag_selected = drag_drop::selected_ids_in_order(model);

    let padded = container(virtual_grid)
        .padding(GRID_GAP)
        .width(Length::Fill)
        .height(Length::Fill);
    // Rects are relative to the padded container, the tile grid's space.
    let dragged: Element<'a, Msg> =
        enz_drag_item_surface(channel, padded, drag_drop::DIRECTORY_COLLECTION, drag_items)
            .selected(drag_selected)
            .accepts_background(true)
            // Pointer-down is captured only to establish tile focus.
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
        .tokens(MarqueeTokens::from_palette(&theme.palette))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    let body: Element<'a, Msg> = enz_drag_autoscroll_host(
        body,
        iced::widget::Id::new(GRID_SCROLLER_ID),
        AutoScroll::new(30.0, 400.0),
    )
    // Programmatic autoscroll never fires `.on_scroll(..)`, so this is the only
    // way the model learns the host scrolled.
    .on_scrolled(Msg::GridScrolled)
    .width(Length::Fill)
    .height(Length::Fill)
    .into();
    enzui_widgets::track_bounds(body, &model.directory_view_bounds)
}

fn background_context_target<'a>(content: Element<'a, Msg>, model: &Model) -> Element<'a, Msg> {
    enz_context_target(content, Msg::OpenBackgroundContextMenu)
        .id(iced::advanced::widget::Id::new(
            "files-app-grid-background-context",
        ))
        .label(crate::i18n::format(
            "files-background-actions-441af9b7",
            &[("arg0", (model.cwd.display()).to_string())],
        ))
        // Covers the void between and below tiles.
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

pub(crate) fn realised_range_for_geometry(
    item_count: u64,
    resolved: enzui_core::virtual_grid::ResolvedTileGrid,
    viewport_height: f32,
    scroll_y: f32,
) -> std::ops::Range<usize> {
    let grid = resolved.grid();
    let columns = resolved.columns.max(1);
    let rows = grid.row_model(item_count, columns);
    let band = PixelOverscan::new(resolved.tile_height / 2.0).realize(
        &rows,
        &Anchor::from_scroll_top(&rows, scroll_y),
        viewport_height,
    );
    let items = grid.items_in_rows(band, columns, item_count);
    usize::try_from(items.start).unwrap_or(usize::MAX)
        ..usize::try_from(items.end).unwrap_or(usize::MAX)
}

// Id prefix for grid tiles, shared with the reducer so keyboard navigation
// moves real widget focus onto the destination tile.
pub(crate) const GRID_TILE_ID_PREFIX: &str = "files-app-grid-tile";

// Metadata lines a tile reserves under its name: one for a single facet, two
// for more, none when Grid info is off.
pub(crate) fn grid_info_lines(model: &Model) -> usize {
    // An Everywhere search adds where each result is.
    let location = usize::from(searching_everywhere(model));
    match model.grid_info.iter().filter(|shown| **shown).count() + location {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

fn searching_everywhere(model: &Model) -> bool {
    model.search_active && model.search_scope == crate::model::SearchScope::Everywhere
}

// The metadata band's height excluding the gap above, from the framework's
// line-height ramp so a typography retune cannot clip the second line.
pub(crate) fn grid_info_band(model: &Model) -> f32 {
    grid_info_lines(model) as f32 * TextSize::Caption2.line_height_px()
}

pub(crate) fn grid_tile_height(model: &Model, theme: &EnzTheme) -> f32 {
    let zoomed = GRID_TILE_H + zoomed_glyph_band(model, theme) - tile_glyph_band(theme);
    let lines = grid_info_lines(model);
    if lines == 0 {
        zoomed
    } else {
        zoomed + grid_info_band(model) + theme.spacing.xs()
    }
}

fn tile<'a>(
    entry: &FileEntry,
    model: &Model,
    theme: &EnzTheme,
    tile_height: f32,
) -> Element<'a, Msg> {
    let palette = &theme.palette;
    let selected = model.is_selected(&entry.path);
    let glyph: Element<'a, Msg> = match model.thumbnails.get(&entry.path) {
        Some(crate::model::ThumbnailState::Ready { handle, .. }) => {
            let size = zoomed_glyph_size(model, theme);
            let (width, height) = match handle {
                iced::widget::image::Handle::Rgba { width, height, .. } => {
                    let scale = size / (*width).max(*height).max(1) as f32;
                    (*width as f32 * scale, *height as f32 * scale)
                }
                _ => (size, size),
            };
            let image = enz_image_handle(handle.clone())
                .alt_text(crate::i18n::format(
                    "files-preview-of-b9157211",
                    &[("arg0", (entry.name).clone())],
                ))
                .fit(EnzImageFit::Contain)
                .placeholder_background(iced::Color::TRANSPARENT)
                .radius(theme.radius.md)
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .into_element();
            container(image)
                .center_x(Length::Fill)
                .center_y(Length::Fixed(zoomed_glyph_band(model, theme)))
                .into()
        }
        _ => container(crate::icon::entry_element(
            model,
            entry,
            palette,
            crate::icon::grid_tile_backdrop(palette, selected),
            zoomed_glyph_size(model, theme),
        ))
        .center_x(Length::Fixed(grid_tile_width(model, theme)))
        .center_y(Length::Fixed(zoomed_glyph_band(model, theme)))
        .into(),
    };

    // `single_line_elided()` elides by measured width and publishes an
    // `AccessibilityCheck::Elision` node for the a11y audit.
    if let Some(state) = model
        .inline_rename
        .as_ref()
        .filter(|state| state.targets.first() == Some(&entry.path))
    {
        // The rename field sits outside the pressable so it receives input.
        return container(
            column![glyph, crate::view::widgets::inline_rename_field(state)]
                .spacing(theme.spacing.space3)
                .align_x(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(tile_height))
        .padding(theme.spacing.space3)
        .align_y(Alignment::Center)
        .into();
    }
    // Up to two lines, broken inside long names, then elided.
    let name = container(
        enz_text(entry.name.clone())
            .size(TextSize::Caption)
            .tone(TextTone::Default)
            .align(TextAlign::Center)
            .wrap(true)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .lines(2)
            .width(Length::Fill),
    )
    .center_x(Length::Fill);

    let mut content = column![glyph, name]
        .spacing(theme.spacing.space3)
        .align_x(Alignment::Center)
        .width(Length::Fill);
    let mut metadata = Vec::new();
    if searching_everywhere(model) {
        // The folder it was found in, relative to where the search started.
        let parent = entry.path.parent().unwrap_or(&entry.path);
        let shown = parent
            .strip_prefix(&model.cwd)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .map_or_else(
                || {
                    parent
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                },
                |relative| relative.display().to_string(),
            );
        if !shown.is_empty() {
            metadata.push(shown);
        }
    }
    if model.grid_info_visible(crate::model::MetaColumn::Size) {
        metadata.push(crate::services::format_size(entry));
    }
    if model.grid_info_visible(crate::model::MetaColumn::Modified) {
        metadata.push(crate::services::format_modified(entry));
    }
    if model.grid_info_visible(crate::model::MetaColumn::Kind) {
        metadata.push(entry.kind.label(&entry.mime));
    }
    if !metadata.is_empty() {
        // The reserved band lets a second line lay out; longer runs elide
        // rather than grow the tile.
        content = content.push(
            container(
                enz_text(metadata.join(" · "))
                    .size(TextSize::Caption2)
                    .tone(TextTone::Muted)
                    .align(TextAlign::Center)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(grid_info_band(model)))
            .width(Length::Fill)
            .clip(true),
        );
    }

    // Top-aligned, so icons line up across a row whether a name takes one
    // line or two.
    let cell = container(content)
        .width(Length::Fill)
        .height(Length::Fixed(tile_height))
        .padding(theme.spacing.space3)
        .align_y(Alignment::Start);

    // Pointer modifiers are latched by the pressable, matching list-row selection.
    let path_for_click = entry.path.clone();
    let kind_for_click = entry.kind;

    // `PressableStyle::Plain`: `Card` draws a border on every tile at rest.
    // Enter and the AT default action open directly, bypassing the double-click
    // latch; Space only selects (`Msg::SelectOnly`), as in `view::list::entry_row`.
    let path_for_enter = entry.path.clone();
    let kind_for_enter = entry.kind;
    let path_for_space = entry.path.clone();
    let pressable = enz_pressable(cell.into(), Msg::Noop)
        .on_activate_with_modifiers(move |m| Msg::RowClickPrimary {
            path: path_for_click.clone(),
            kind: kind_for_click,
            command: m.command(),
            shift: m.shift(),
        })
        .style(PressableStyle::Plain)
        .selectable(true)
        .selected(selected)
        .aria_label(crate::icon::entry_accessible_name(model, entry))
        // Keyed on the path so duplicate basenames from search stay distinct.
        .id(crate::context_menu::grid_tile_id(&entry.path))
        .on_enter(move || Msg::OpenEntry(path_for_enter.clone(), kind_for_enter))
        .on_space(move || Msg::SpaceOnRow(path_for_space.clone()))
        .width(Length::Fill);

    // The grid needs its own right-press capture; `enz_context_target` passes
    // every other event to the pressable. The anchor comes from the click point
    // because a tile in a scrolled wrapping grid has no static trigger layout.
    let path = entry.path.clone();
    let content = enz_context_target(pressable, move |at| Msg::OpenContextMenu(path.clone(), at))
        .width(Length::Fill)
        .height(Length::Fixed(tile_height))
        .into();
    super::native_transfer::entry(model, entry, content)
}
