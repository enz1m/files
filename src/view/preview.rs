use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{EnzTheme, FontRole};
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::selection_scope::enz_selection_scope;
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::status_page::enz_status_page;
use enzui_widgets::surface::{RadiusSize, SurfaceTone, enz_surface};
use enzui_widgets::title_bar::enz_title_bar;
use iced::widget::image;
use iced::{ContentFit, Element, Length};
use std::hash::{Hash, Hasher};
use std::path::Path;

use crate::message::Msg;
use crate::preview::{Preview, PreviewContent};

pub(crate) const UNSUPPORTED_TITLE: &str = "No preview available";
pub(crate) const PREVIEW_ERROR_TITLE: &str = "Preview could not be loaded";

fn text_selection_id(path: &Path) -> iced::advanced::widget::Id {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    iced::advanced::widget::Id::from(format!(
        "files-text-preview-selection-{:016x}",
        hasher.finish()
    ))
}

fn preview_body<'a>(
    item: &'a Preview,
    frame: Option<&'a image::Handle>,
    theme: &'a EnzTheme,
    id: iced::window::Id,
) -> Element<'a, Msg> {
    use iced::widget::container;

    match &item.content {
        PreviewContent::Image(handle) => {
            let img: Element<'a, Msg> = image(handle.clone())
                .content_fit(ContentFit::Contain)
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
            let named: Element<'a, Msg> =
                SemanticsContainer::leaf(img, Semantics::new(Role::Image).label(item.name.clone()))
                    .into();
            enz_surface(named)
                .fill(iced::Color::BLACK)
                .radius_size(RadiusSize::None)
                .width(Length::Fill)
                .height(Length::Fill)
                .into_element()
        }
        PreviewContent::Pdf {
            page,
            pending,
            error,
        } => {
            use enzui_widgets::toolbar_icon::enz_toolbar_icon;
            use iced::widget::{column, row};
            let controls = row![
                enz_toolbar_icon(
                    "chevron-left",
                    Msg::PreviewPdfPage(id, page.index.saturating_sub(1))
                )
                .label(crate::i18n::tr("Previous page"))
                .enabled(page.index > 0 && pending.is_none())
                .into_element(),
                enz_text(crate::i18n::format(
                    "files-preview-page",
                    &[
                        ("page", (page.index + 1).to_string()),
                        ("count", page.count.to_string())
                    ]
                ))
                .size(TextSize::Footnote),
                enz_toolbar_icon(
                    "chevron-right",
                    Msg::PreviewPdfPage(id, page.index.saturating_add(1))
                )
                .label(crate::i18n::tr("Next page"))
                .enabled(page.index + 1 < page.count && pending.is_none())
                .into_element(),
            ]
            .spacing(theme.spacing.sm())
            .align_y(iced::Alignment::Center);
            let mut body = column![
                image(page.image.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill),
                container(controls)
                    .center_x(Length::Fill)
                    .padding(theme.spacing.sm()),
            ];
            if let Some(error) = error {
                body = body.push(
                    enz_text(error)
                        .size(TextSize::Footnote)
                        .tone(TextTone::Muted),
                );
            }
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
        PreviewContent::Media { player, status } => {
            use enzui_widgets::media_controls::enz_media_controls;
            use iced::widget::column;
            let picture: Element<'a, Msg> = if let Some(error) = &status.error {
                container(
                    enz_status_page(crate::i18n::tr(PREVIEW_ERROR_TITLE))
                        .description(error.to_string())
                        .icon("triangle-alert")
                        .into_element(),
                )
                .center(Length::Fill)
                .into()
            } else if let Some(frame) = frame {
                container(
                    image(frame.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .style(|_| container::Style {
                    background: Some(iced::Color::BLACK.into()),
                    ..Default::default()
                })
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            } else {
                let description = if status.loading {
                    crate::i18n::tr("Loading preview…").to_owned()
                } else if !status.missing_decoders.is_empty() {
                    status.missing_decoders.join(", ")
                } else {
                    item.type_label.clone()
                };
                container(
                    enz_status_page(&item.name)
                        .description(description)
                        .icon(if player.audio { "music" } else { "film" })
                        .into_element(),
                )
                .center(Length::Fill)
                .into()
            };
            let controls =
                enz_media_controls(format!("files-preview-media-{id}"), status.snapshot(None))
                    .time_display(enzui_widgets::media_controls::TimeDisplay::Elapsed)
                    .into_element()
                    .map(move |control| Msg::PreviewMediaControl(id, control));
            column![picture, controls]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
        PreviewContent::Text(text) => {
            // The theme's fixed-width face keeps indentation and columns.
            let body_text = enz_text(text.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Default)
                .font_role(FontRole::Monospace)
                .font_from_theme(theme)
                .into_element();
            let selectable_body = enz_selection_scope(body_text)
                .id(text_selection_id(&item.path))
                .label(crate::i18n::tr("Text file preview"))
                .document_key(&item.path)
                .on_copy(Msg::CopyPreviewSelection)
                .into_element();
            let text = enz_scroll_column(vec![selectable_body])
                .inset(theme.spacing.md())
                .into_element();
            enz_surface(text)
                .tone(SurfaceTone::Elevated)
                .radius_size(RadiusSize::None)
                .width(Length::Fill)
                .height(Length::Fill)
                .into_element()
        }
        PreviewContent::Unsupported => container(
            enz_status_page(crate::i18n::tr(UNSUPPORTED_TITLE))
                .description(format!("{}\n{}", item.name, item.type_label))
                .icon("file")
                .into_element(),
        )
        .center(Length::Fill)
        .into(),
        PreviewContent::Error(error) => container(
            enz_status_page(crate::i18n::tr(PREVIEW_ERROR_TITLE))
                .description(format!("{}\n{error}", item.name))
                .icon("triangle-alert")
                .into_element(),
        )
        .center(Length::Fill)
        .into(),
    }
}

pub(crate) fn view<'a>(
    item: &'a Preview,
    frame: Option<&'a image::Handle>,
    theme: &'a EnzTheme,
    id: iced::window::Id,
) -> Element<'a, Msg> {
    enz_app_shell(move || preview_body(item, frame, theme, id))
        .layout(ShellLayout::TitlebarContent)
        .titlebar(move || {
            enz_title_bar()
                .id(format!("files-preview-{id}"))
                .title(item.name.clone())
                .title_heading(enzui_core::semantics::HeadingLevel::Page)
                .into_element()
        })
        .on_window_gesture(move |command| Msg::PreviewWindow(id, command))
        .into_element()
}
