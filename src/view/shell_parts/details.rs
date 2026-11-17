// Details inspector pane.

use super::*;
use iced::widget::column;

// The details inspector's title and its `Role::Group` accessible label.
pub(crate) const DETAILS_INSPECTOR_TITLE: &str = "Details";

// The details pane: metadata for one selection, count and total size for
// several, and an empty state for none.
//
// A single selection uses a cached properties snapshot, falling back to loaded
// entry fields while it is pending or unavailable. Folder size walks start only
// on request. Previews share the grid's bounded thumbnail lifecycle and fall
// back to a file-type icon.
pub(crate) fn details_pane<'a>(model: &'a Model, theme: &'a EnzTheme) -> Element<'a, Msg> {
    let sp = &theme.spacing;

    if model.selection.is_empty() {
        return container(
            enz_status_page(crate::i18n::tr(crate::view::DETAILS_EMPTY_HINT))
                .icon("info")
                .into_element(),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();
    }

    // Folders add 0 to the total, matching the Size column's em dash; a recursive
    // walk per folder would need batched off-thread work.
    if model.selection.len() >= 2 {
        let mut total: u64 = 0;
        for p in model.selection.iter() {
            if let Some(e) = model.entry_for_path(p)
                && !matches!(e.kind, crate::model::EntryKind::Dir)
            {
                total = total.saturating_add(e.size);
            }
        }
        let heading = enz_text(crate::i18n::format(
            "files-items-selected-075427f0",
            &[("arg0", (model.selection.len()).to_string())],
        ))
        .size(TextSize::Body)
        .tone(TextTone::Default);
        let total_row: Element<'a, Msg> = enz_labeled_content(crate::i18n::tr("Total size"))
            .value(crate::services::format_bytes(total))
            .typography(theme.typography)
            .sizing(theme.sizing)
            .into();
        let rows = column![heading.into_element(), total_row]
            .spacing(sp.md())
            .width(Length::Fill);
        return container(rows).padding(sp.lg()).width(Length::Fill).into();
    }

    let path = model
        .selection
        .iter()
        .next()
        .expect("single-selection branch has one path");
    let Some(entry) = model.entry_for_path(path) else {
        return container(
            enz_status_page(crate::i18n::tr(crate::view::DETAILS_EMPTY_HINT))
                .icon("info")
                .into_element(),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into();
    };

    // Falls back to the MIME icon while loading or after a recorded failure.
    let preview_height = if model.thumbnails.contains_key(&entry.path) {
        128.0
    } else {
        theme.sizing.icon_xxl + sp.md()
    };
    let glyph_el: Element<'a, Msg> = match model.thumbnails.get(&entry.path) {
        Some(crate::model::ThumbnailState::Ready { handle, .. }) => {
            enz_image_handle(handle.clone())
                .alt_text(crate::i18n::format(
                    "files-preview-of-b9157211",
                    &[("arg0", (entry.name).clone())],
                ))
                .fit(EnzImageFit::Contain)
                .hug_picture()
                .radius(theme.radius.md)
                .width(Length::Fill)
                .height(Length::Fixed(preview_height))
                .into_element()
        }
        _ => container(crate::icon::entry_element(
            model,
            entry,
            &theme.palette,
            theme.palette.surface_alt,
            crate::icon::slot_size(model, theme.sizing.icon_xxl),
        ))
        .center_y(Length::Fixed(preview_height))
        .into(),
    };
    // The slot centres within `icon_xxl`; the outer container fills the pane.
    let glyph = container(glyph_el).center_x(Length::Fill);

    // Name and type are centred under the centred preview, as one heading.
    let name = enz_text(entry.name.clone())
        .size(TextSize::Body)
        .weight_rung(enzui_theme::WeightRung::Medium)
        .tone(TextTone::Default)
        .align(enzui_widgets::enz_text::TextAlign::Center)
        // Names often lack whitespace; glyph fallback keeps a long name inside
        // the pane.
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        .full_width()
        .into_element();
    let type_line = enz_text(entry.kind.label(&entry.mime))
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .align(enzui_widgets::enz_text::TextAlign::Center)
        .full_width()
        .into_element();

    let info = model
        .inspector_properties
        .as_ref()
        .filter(|info| &info.path == path);

    let field = |label: &'static str, value: String| -> Element<'a, Msg> {
        enz_labeled_content(label)
            .value(value)
            .typography(theme.typography)
            .sizing(theme.sizing)
            .into()
    };

    let mut rows: Vec<Element<'a, Msg>> = Vec::new();

    // The type is the heading's second line, so it is not repeated as a row;
    // a folder's MIME type (`inode/directory`) says nothing.
    let is_dir = matches!(entry.kind, crate::model::EntryKind::Dir);
    if let Some(info) = info {
        if info.is_symlink && !info.link_target.is_empty() {
            rows.push(field(
                crate::i18n::tr("Link target"),
                info.link_target.clone(),
            ));
        }
        if !info.mime_type.is_empty() && !is_dir {
            rows.push(field(crate::i18n::tr("MIME type"), info.mime_type.clone()));
        }
        if matches!(entry.kind, crate::model::EntryKind::Dir) {
            if !info.content_summary.is_empty() {
                rows.push(field(
                    crate::i18n::tr("Contents"),
                    info.content_summary.clone(),
                ));
            }
        } else if !info.size_text.is_empty() && info.size_text != "\u{2014}" {
            rows.push(field(crate::i18n::tr("Size"), info.size_text.clone()));
        }
        if !info.modified_text.is_empty() {
            rows.push(field(
                crate::i18n::tr("Modified"),
                info.modified_text.clone(),
            ));
        }
        if !info.accessed_text.is_empty() {
            rows.push(field(
                crate::i18n::tr("Accessed"),
                info.accessed_text.clone(),
            ));
        }
        rows.push(field(crate::i18n::tr("Owner"), info.owner.clone()));
        rows.push(field(crate::i18n::tr("Group"), info.group.clone()));
        rows.push(field(
            crate::i18n::tr("Permissions"),
            crate::i18n::format(
                "files-permissions-value",
                &[
                    ("symbolic", info.symbolic_permissions()),
                    ("octal", info.octal_permissions()),
                ],
            ),
        ));
    } else {
        // Loaded metadata remains available while the inspector snapshot is pending.
        if !entry.mime.is_empty() && !is_dir {
            rows.push(field(crate::i18n::tr("MIME type"), entry.mime.clone()));
        }
        if !matches!(entry.kind, crate::model::EntryKind::Dir) {
            rows.push(field(
                crate::i18n::tr("Size"),
                crate::services::format_size(entry),
            ));
        }
        let modified = crate::services::format_modified(entry);
        if !modified.is_empty() {
            rows.push(field(crate::i18n::tr("Modified"), modified));
        }
    }

    if entry.kind == crate::model::EntryKind::Dir {
        rows.push(folder_size_row(model, theme, path));
    }

    let header: Element<'a, Msg> = column![glyph, name, type_line]
        .spacing(sp.xs())
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into();

    let field_stack: Element<'a, Msg> = column(rows).spacing(sp.xs()).width(Length::Fill).into();
    let body = column![header, field_stack]
        .spacing(sp.md())
        .width(Length::Fill);

    container(body)
        .padding(Padding::from([sp.lg(), 0.0]))
        .width(Length::Fill)
        .into()
}

fn folder_size_row<'a>(
    model: &Model,
    theme: &EnzTheme,
    path: &std::path::Path,
) -> Element<'a, Msg> {
    use crate::folder_size::{self, FolderSize};
    use enzui_widgets::button::{ButtonKind, ButtonSize, enz_button_with};
    use enzui_widgets::toolbar_icon::enz_toolbar_icon;

    let row_for = |control: Element<'a, Msg>| -> Element<'a, Msg> {
        // Trailing, where the other rows' values sit.
        let control = container(control)
            .width(Length::Fill)
            .align_x(Alignment::End);
        enz_labeled_content(crate::i18n::tr("Size"))
            .control(control)
            .typography(theme.typography)
            .sizing(theme.sizing)
            .into()
    };
    let action_id = enzui_core::identity::part_id(
        &crate::context_menu::list_row_id(path),
        "details-folder-size",
    );
    let items = |tally: folder_size::Tally| {
        crate::i18n::format(
            "files-size-items",
            &[
                ("size", folder_size::label(FolderSize::Measured(tally))),
                ("items", folder_size::items_label(tally.items)),
            ],
        )
    };
    match model.folder_sizes.get(path).copied() {
        Some(FolderSize::Measured(tally)) => enz_labeled_content(crate::i18n::tr("Size"))
            .value(items(tally))
            .typography(theme.typography)
            .sizing(theme.sizing)
            .into(),
        Some(size @ FolderSize::Measuring(tally)) => {
            let so_far = crate::i18n::format(
                "files-measuring-size-items",
                &[
                    ("size", folder_size::label(size)),
                    ("items", folder_size::items_label(tally.items)),
                ],
            );
            row_for(
                row![
                    enz_text(so_far)
                        .size(TextSize::Caption)
                        .tone(TextTone::Muted),
                    enz_toolbar_icon("x", Msg::CancelFolderMeasure(path.to_path_buf()))
                        .id(action_id)
                        .size(
                            theme.sizing.icon_md + theme.spacing.xs(),
                            theme.sizing.icon_sm
                        )
                        .aria_label(crate::i18n::tr(folder_size::STOP_LABEL))
                        .tooltip(crate::i18n::tr(folder_size::STOP_LABEL)),
                ]
                .mirrored(crate::i18n::rtl())
                .spacing(theme.spacing.xs())
                .align_y(Alignment::Center)
                .into(),
            )
        }
        None => {
            let target = path.to_path_buf();
            row_for(
                enz_button_with(crate::i18n::tr(folder_size::CALCULATE_LABEL), move || {
                    Msg::MeasureFolder(target.clone())
                })
                .icon(folder_size::CALCULATE_ICON)
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm)
                .id(action_id)
                .into(),
            )
        }
    }
}

pub(super) fn details_panel<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    width: f32,
    rtl: bool,
) -> Element<'a, Msg> {
    use enzui_core::semantics::{Role, Semantics};
    use enzui_widgets::motion::{MotionEffect, motion};
    use enzui_widgets::surface::{BorderStyle, SurfaceTone, enz_surface};
    let geometry = DetailsGeometry::new(rtl, theme.radius.lg);
    let body = container(details_pane(model, theme))
        .padding(Padding {
            top: 8.0,
            right: 8.0,
            bottom: 0.0,
            left: 8.0,
        })
        .width(Length::Fill);
    let scroll: Element<'a, Msg> = enz_scroll_column(vec![body.into()]).spacing(0.0).into();
    let surface: Element<'a, Msg> = enz_surface(scroll)
        .tone(SurfaceTone::SurfaceAlt)
        .border_style(BorderStyle::None)
        .border_width(0.0)
        .radius_resolved(move |theme| {
            DetailsGeometry::new(rtl, enzui_widgets::theme_lookup::radius(theme).lg).radius
        })
        .padding(0)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .clip(true)
        .into();
    let panel: Element<'a, Msg> =
        iced::widget::stack![surface, Element::new(DetailsEdge { rtl })].into();
    let named = SemanticsContainer::new(
        panel,
        Semantics::new(Role::Group).label(crate::i18n::tr(DETAILS_INSPECTOR_TITLE)),
    );
    // Translation preserves label/value layout throughout the reveal.
    motion(
        named,
        model.details_open,
        MotionEffect::Slide(geometry.slide),
    )
    .into()
}

struct DetailsGeometry {
    radius: iced::border::Radius,
    edge: iced::advanced::graphics::geometry::border::Edge,
    slide: enzui_widgets::motion::SlideDirection,
}

impl DetailsGeometry {
    fn new(rtl: bool, radius: f32) -> Self {
        use enzui_widgets::motion::SlideDirection;
        use iced::advanced::graphics::geometry::border::Edge;
        if rtl {
            Self {
                radius: iced::border::right(radius),
                edge: Edge::Trailing,
                slide: SlideDirection::Left,
            }
        } else {
            Self {
                radius: iced::border::left(radius),
                edge: Edge::Leading,
                slide: SlideDirection::Right,
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DetailsEdge {
    rtl: bool,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer> for DetailsEdge {
    fn size(&self) -> iced::Size<Length> {
        iced::Size::new(Length::Fill, Length::Fill)
    }
    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        _: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        iced::advanced::layout::paint_box(limits, Length::Fill, Length::Fill).apply(tree);
    }
    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        _: iced::mouse::Cursor,
        _: &iced::Rectangle,
    ) {
        use iced::advanced::graphics::geometry::{Renderer as _, border::Border};
        let geometry =
            DetailsGeometry::new(self.rtl, enzui_widgets::theme_lookup::radius(theme).lg);
        let radius = geometry.radius;
        renderer.draw_layout_border(
            tree.container_presentation_node()
                .expect("details edge paint allocation"),
            layout.bounds(),
            Border {
                edge: geometry.edge,
                top: true,
                corners: [
                    radius.top_left,
                    radius.top_right,
                    radius.bottom_left,
                    radius.bottom_right,
                ],
                color: enzui_widgets::toolbar_divider::default_divider_ink(
                    &enzui_widgets::theme_lookup::palette(theme),
                ),
            },
        );
    }
}
