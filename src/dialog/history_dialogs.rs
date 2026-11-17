// Previous versions of a file and a folder as it was (enzimOS history).
//
// Both are wide dialogs built like the rest of Files: a header row naming
// what is shown, rows in the Downloads page's shape (file-type icon, name,
// one muted status line), and the default action as the Accent button.
// Closing is the corner button or Escape, as in Properties.

use super::*;
use crate::history::{BrowseDialog, HistoryMsg, Preview, VersionsDialog};
use enzui_widgets::banner::{BannerTone, enz_banner};
use enzui_widgets::icon::{IconTone, enz_icon};
use enzui_widgets::list_row_surface::enz_list_row_surface;
use enzui_widgets::status_page::enz_status_page;
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use iced::widget::{column, container};

pub(crate) const VERSIONS_TITLE: &str = "Previous Versions";
pub(crate) const BROWSE_TITLE: &str = "Browse Previous Versions";

// The window width from which the version list and the preview sit side by side.
const SIDE_BY_SIDE: f32 = 860.0;
const FILE_ICON_SIZE: f32 = 24.0;
const PREVIEW_HEIGHT: f32 = 260.0;

fn send(message: HistoryMsg) -> Msg {
    Msg::History(message)
}

fn name_of(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

// "In Documents": the folder a file is in, by name.
fn location_of(path: &std::path::Path) -> Option<String> {
    let parent = path.parent()?;
    Some(crate::i18n::format(
        "files-in-folder",
        &[("folder", name_of(parent))],
    ))
}

fn file_icon<'a>(name: &str, dir: bool) -> Element<'a, Msg> {
    crate::icon::row_file_icon(name, dir, FILE_ICON_SIZE)
}

fn caption<'a>(text: String) -> Element<'a, Msg> {
    enz_text(text)
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element()
}

fn loading<'a>(theme: &EnzTheme, text: &str) -> Element<'a, Msg> {
    row![
        enz_spinner().size(16.0).into_element(),
        enz_text(text.to_owned())
            .tone(TextTone::Muted)
            .into_element()
    ]
    .spacing(theme.spacing.sm())
    .align_y(Alignment::Center)
    .into()
}

fn problem<'a>(title: &str, error: &str) -> Element<'a, Msg> {
    enz_status_page::<Msg>(title.to_owned())
        .description(error.to_owned())
        .icon("history")
        .compact(true)
        .into_element()
}

fn error_banner<'a>(error: Option<&String>) -> Option<Element<'a, Msg>> {
    error.map(|e| {
        enz_banner(e.clone())
            .tone(BannerTone::Danger)
            .width(Length::Fill)
            .into_element()
    })
}

// A pale panel the preview sits in.
fn panel<'a>(theme: &EnzTheme, content: Element<'a, Msg>) -> Element<'a, Msg> {
    enz_surface(
        container(content)
            .padding(theme.spacing.md())
            .width(Length::Fill)
            .height(Length::Fixed(PREVIEW_HEIGHT)),
    )
    .tone(SurfaceTone::SurfaceAlt)
    .radius(10.0)
    .width(Length::Fill)
    .into_element()
}

pub(crate) fn previous_versions<'a>(
    theme: &EnzTheme,
    dialog: &'a VersionsDialog,
    window: Size,
) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    let name = name_of(&dialog.path);
    let header = super::storage_dialogs::subject(
        theme,
        if crate::history::is_image(&dialog.path) {
            "image"
        } else {
            "file-text"
        },
        name.clone(),
        location_of(&dialog.path),
    );
    let now = crate::history::now();
    let mut chosen = None;
    let content: Element<'a, Msg> = match &dialog.versions {
        None => loading(theme, crate::i18n::tr("Looking for earlier versions…")),
        Some(Err(error)) => problem(
            crate::i18n::tr("Earlier versions could not be listed"),
            error,
        ),
        Some(Ok(versions)) if versions.iter().all(|v| v.current) => {
            enz_status_page::<Msg>(crate::i18n::tr("No earlier versions yet"))
                .description(crate::i18n::tr(
                    "A version is kept every hour while a file changes. Come back after you have \
             worked on it.",
                ))
                .icon("history")
                .compact(true)
                .into_element()
        }
        Some(Ok(versions)) => {
            let mut list = column![].spacing(2).width(Length::Fill);
            for (index, version) in versions.iter().enumerate() {
                let when = crate::history::when(version.taken, now);
                let size = crate::services::format_bytes(version.size);
                let detail = if version.current {
                    crate::i18n::format("files-size-current", &[("size", size)])
                } else {
                    size
                };
                let selected = dialog.selected == Some(index);
                if selected {
                    chosen = Some(version.clone());
                }
                let text = column![
                    enz_text(when.clone())
                        .size(TextSize::Body)
                        .single_line_elided()
                        .full_width(),
                    caption(detail),
                ]
                .spacing(sp.space1)
                .width(Length::Fill);
                let content = container(
                    row![
                        enz_icon(if version.current {
                            "file-check"
                        } else {
                            "history"
                        })
                        .size(18.0)
                        .tint_theme_tone(IconTone::Muted)
                        .into_element(),
                        text
                    ]
                    .mirrored(crate::i18n::rtl())
                    .spacing(sp.md())
                    .align_y(Alignment::Center),
                )
                .padding(iced::Padding::from([sp.sm(), sp.md()]))
                .width(Length::Fill);
                list = list.push(
                    enz_list_row_surface(content, send(HistoryMsg::Select(index)))
                        .selectable(true)
                        .selected(selected)
                        .label(when)
                        .content_padding(0.0)
                        .content_height(true)
                        .id_prefix("files-history-version")
                        .into_element(),
                );
            }
            let preview: Element<'a, Msg> = match &dialog.preview {
                Some(Preview::Text(text)) => panel(
                    theme,
                    enz_scroll_view(
                        enz_text(text.clone())
                            .size(TextSize::Footnote)
                            .font_role(enzui_theme::FontRole::Monospace)
                            .into_element(),
                    )
                    .height(Length::Fill)
                    .into(),
                ),
                Some(Preview::Image(handle)) => panel(
                    theme,
                    container(
                        iced::widget::image(handle.clone())
                            .content_fit(iced::ContentFit::Contain)
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .center(Length::Fill)
                    .into(),
                ),
                Some(Preview::None) => panel(
                    theme,
                    container(
                        column![
                            file_icon(&name, false),
                            caption(
                                crate::i18n::tr(
                                    "No preview for this kind of file. Open it to look."
                                )
                                .to_owned()
                            )
                        ]
                        .spacing(sp.sm())
                        .align_x(Alignment::Center),
                    )
                    .center(Length::Fill)
                    .into(),
                ),
                None if chosen.is_some() => panel(
                    theme,
                    container(enz_spinner().into_element())
                        .center(Length::Fill)
                        .into(),
                ),
                None => panel(theme, Space::new().into()),
            };
            if window.width >= SIDE_BY_SIDE {
                row![
                    container(list).width(Length::FillPortion(2)),
                    container(preview).width(Length::FillPortion(3))
                ]
                .mirrored(crate::i18n::rtl())
                .spacing(sp.lg())
                .align_y(Alignment::Start)
                .into()
            } else {
                column![list, preview].spacing(sp.md()).into()
            }
        }
    };
    let usable = chosen.as_ref().is_some_and(|v| !v.current) && !dialog.busy;
    let mut body = column![header, content]
        .spacing(sp.lg())
        .width(Length::Fill);
    if let Some(banner) = error_banner(dialog.error.as_ref()) {
        body = body.push(banner);
    }
    // What Restore does to the file there now, said before it is pressed.
    if dialog.busy {
        body = body.push(loading(theme, crate::i18n::tr("Working…")));
    } else if usable {
        body = body.push(caption(
            crate::i18n::tr("Restoring puts the current file in the Trash first.").to_owned(),
        ));
    }
    present_sized(
        theme,
        crate::i18n::tr(VERSIONS_TITLE),
        body.into(),
        vec![
            Action::new(
                crate::i18n::tr("Open"),
                ButtonKind::Quiet,
                send(HistoryMsg::OpenSelected),
            )
            .enabled(usable),
            Action::new(
                crate::i18n::tr("Save a Copy"),
                ButtonKind::Quiet,
                send(HistoryMsg::SaveSelected),
            )
            .enabled(usable),
            Action::new(
                crate::i18n::tr("Restore"),
                ButtonKind::Accent,
                send(HistoryMsg::RestoreSelected),
            )
            .enabled(usable),
        ],
        if window.width >= SIDE_BY_SIDE {
            760.0
        } else {
            theme.sizing.dialog_width_lg
        },
        window,
    )
}

pub(crate) fn browse_history<'a>(
    theme: &EnzTheme,
    dialog: &'a BrowseDialog,
    window: Size,
) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    let now = crate::history::now();
    let mut body = column![super::storage_dialogs::subject(
        theme,
        "folder",
        name_of(&dialog.folder),
        Some(crate::i18n::tr("Pick a time, then the items to bring back.").to_owned()),
    )]
    .spacing(sp.lg())
    .width(Length::Fill);
    match &dialog.snapshots {
        None => {
            body = body.push(loading(
                theme,
                crate::i18n::tr("Looking for earlier versions…"),
            ));
        }
        Some(Err(error)) => {
            body = body.push(problem(
                crate::i18n::tr("Earlier versions could not be listed"),
                error,
            ));
        }
        Some(Ok(snapshots)) if snapshots.is_empty() => {
            body = body.push(
                enz_status_page::<Msg>(crate::i18n::tr("No earlier versions yet"))
                    .description(crate::i18n::tr(
                        "Your home is kept every hour. The first version appears within the hour.",
                    ))
                    .icon("history")
                    .compact(true)
                    .into_element(),
            );
        }
        Some(Ok(snapshots)) => {
            let (_, taken) = snapshots[dialog.at.min(snapshots.len() - 1)];
            let older = dialog.at + 1 < snapshots.len();
            let newer = dialog.at > 0;
            // Older on the left, newer on the right, as on a timeline.
            let when = column![
                enz_text(crate::history::when(taken, now))
                    .size(TextSize::Headline)
                    .into_element(),
                caption(crate::i18n::format(
                    "files-browse-position",
                    &[
                        ("index", (snapshots.len() - dialog.at).to_string()),
                        ("count", snapshots.len().to_string()),
                    ],
                )),
            ]
            .spacing(2)
            .align_x(Alignment::Center);
            body = body.push(
                enz_surface(
                    row![
                        enz_toolbar_icon("chevron-left", send(HistoryMsg::Step(true)))
                            .enabled(older)
                            .aria_label(crate::i18n::tr("Older"))
                            .tooltip(crate::i18n::tr("Older"))
                            .into_element(),
                        container(when).center_x(Length::Fill),
                        enz_toolbar_icon("chevron-right", send(HistoryMsg::Step(false)))
                            .enabled(newer)
                            .aria_label(crate::i18n::tr("Newer"))
                            .tooltip(crate::i18n::tr("Newer"))
                            .into_element(),
                    ]
                    .mirrored(crate::i18n::rtl())
                    .align_y(Alignment::Center)
                    .padding(sp.sm())
                    .width(Length::Fill),
                )
                .tone(SurfaceTone::SurfaceAlt)
                .radius(10.0)
                .width(Length::Fill)
                .into_element(),
            );
            match &dialog.items {
                None => body = body.push(loading(theme, crate::i18n::tr("Loading…"))),
                Some(Err(error)) => {
                    body = body.push(problem(
                        crate::i18n::tr("This version could not be read"),
                        error,
                    ));
                }
                Some(Ok(items)) if items.is_empty() => {
                    body = body.push(caption(
                        crate::i18n::tr("The folder was empty then.").to_owned(),
                    ));
                }
                Some(Ok(items)) => {
                    let mut list = column![].spacing(2).width(Length::Fill);
                    for item in items.iter().take(500) {
                        let mut detail = if item.dir {
                            crate::i18n::tr("Folder").to_owned()
                        } else {
                            crate::services::format_bytes(item.size)
                        };
                        if !item.exists {
                            detail = crate::i18n::format(
                                "files-detail-deleted-since",
                                &[("detail", detail)],
                            );
                        }
                        let name = item.name.clone();
                        let checked = dialog.chosen.contains(&item.name);
                        let text = column![
                            enz_text(item.name.clone())
                                .size(TextSize::Body)
                                .single_line_elided()
                                .full_width(),
                            enz_text(detail)
                                .size(TextSize::Caption)
                                .tone(if item.exists {
                                    TextTone::Muted
                                } else {
                                    TextTone::Warning
                                })
                                .into_element(),
                        ]
                        .spacing(sp.space1)
                        .width(Length::Fill);
                        let toggle = name.clone();
                        let content = container(
                            row![
                                enz_checkbox(checked, "", move |on| {
                                    send(HistoryMsg::Choose(name.clone(), on))
                                })
                                .tokens(CheckboxTokens::from_palette(&theme.palette))
                                .into_element(),
                                file_icon(&item.name, item.dir),
                                text,
                            ]
                            .mirrored(crate::i18n::rtl())
                            .spacing(sp.md())
                            .align_y(Alignment::Center),
                        )
                        .padding(iced::Padding::from([sp.xs(), sp.md()]))
                        .width(Length::Fill);
                        list = list.push(
                            enz_list_row_surface(
                                content,
                                send(HistoryMsg::Choose(toggle, !checked)),
                            )
                            .selectable(true)
                            .selected(checked)
                            .label(item.name.clone())
                            .content_padding(0.0)
                            .content_height(true)
                            .id_prefix("files-history-item")
                            .into_element(),
                        );
                    }
                    body = body.push(list);
                }
            }
        }
    }
    if let Some(banner) = error_banner(dialog.error.as_ref()) {
        body = body.push(banner);
    }
    let count = dialog.chosen.len();
    if dialog.busy {
        body = body.push(loading(theme, crate::i18n::tr("Restoring…")));
    } else if count > 0 {
        body = body.push(caption(
            crate::i18n::tr("Anything a restored item replaces goes to the Trash first.")
                .to_owned(),
        ));
    }
    present_sized(
        theme,
        crate::i18n::tr(BROWSE_TITLE),
        body.into(),
        vec![
            Action::new(
                if count == 0 {
                    crate::i18n::tr("Restore").to_owned()
                } else {
                    crate::i18n::count("files-restore-count", count as u64)
                },
                ButtonKind::Accent,
                send(HistoryMsg::RestoreChosen),
            )
            .enabled(count > 0 && !dialog.busy),
        ],
        theme.sizing.dialog_width_lg,
        window,
    )
}
