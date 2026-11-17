// The strip below the file list naming the folder's repository: branch,
// where it is published, work-tree changes, the project tools it configures
// (never run) with the Files-only trust mark when the Developer package is
// installed, and the repository actions.
//
// One compact line in a status-bar rhythm: every item sits in the same
// 24 px box with one text size, so labels and icons share a centre line, and
// thin rules separate the groups.

use crate::message::Msg;
use crate::model::Model;
use crate::project::{self, ProjectMsg, StatusView};
use enzui_theme::EnzTheme;
use enzui_widgets::badge::{BadgeTone, enz_badge};
use enzui_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_widgets::divider::enz_divider;
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::icon::enz_icon;
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use iced::widget::{Space, container, row};
use iced::{Alignment, Border, Color, Element, Length};

// Below this width the remote and the tool note give way.
const COMPACT_BELOW: f32 = 640.0;
// Below this width the status keeps only its dot.
const MINIMAL_BELOW: f32 = 480.0;

// The height every item shares, and the glyph size in it.
const ITEM_HEIGHT: f32 = 24.0;
const GLYPH: f32 = 14.0;
const DOT: f32 = 7.0;

// An item centred in the shared box.
fn item<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .height(Length::Fixed(ITEM_HEIGHT))
        .align_y(Alignment::Center)
        .into()
}

// Secondary details, at the link's size so the remote, the status and the
// tool note read as one row under the brighter branch name.
fn label<'a>(text: impl Into<String>, tone: TextTone) -> Element<'a, Msg> {
    enz_text(text.into())
        .size(TextSize::Caption)
        .tone(tone)
        .single_line_elided()
        .into_element()
}

fn rule<'a>() -> Element<'a, Msg> {
    container(enz_divider().vertical())
        .height(Length::Fixed(GLYPH))
        .into()
}

fn action<'a>(icon: &'a str, tip: &'a str, message: Msg) -> Element<'a, Msg> {
    enz_toolbar_icon(icon, message)
        .size(ITEM_HEIGHT, GLYPH)
        .aria_label(tip)
        .tooltip(tip)
        .into_element()
}

// The work tree in a few words, with the colour of its dot: changes are a
// warning, conflicts a danger, a clean tree a success, unknown muted.
fn status(info: &project::ProjectInfo, theme: &EnzTheme) -> (String, Color) {
    let palette = &theme.palette;
    match info.kind {
        enzui_services::vcs::git::RepositoryKind::Bare => {
            return (
                crate::i18n::tr("Bare repository").to_owned(),
                palette.text_muted,
            );
        }
        enzui_services::vcs::git::RepositoryKind::GitDirectory => {
            return (
                crate::i18n::tr("Inside the repository's data").to_owned(),
                palette.text_muted,
            );
        }
        _ => {}
    }
    match &info.status {
        StatusView::Ready(summary) if summary.is_clean() && !summary.untracked_truncated => {
            (crate::i18n::tr("Clean").to_owned(), palette.success)
        }
        StatusView::Ready(summary) => {
            let mut parts = Vec::new();
            if summary.conflicted > 0 {
                parts.push(crate::i18n::format(
                    "files-repo-conflicts",
                    &[("count", summary.conflicted.to_string())],
                ));
            }
            let changed = summary.modified + summary.deleted;
            if changed > 0 {
                parts.push(crate::i18n::format(
                    "files-repo-changed",
                    &[("count", changed.to_string())],
                ));
            }
            if summary.untracked > 0 || summary.untracked_truncated {
                let plus = if summary.untracked_truncated { "+" } else { "" };
                parts.push(crate::i18n::format(
                    "files-repo-new",
                    &[("count", format!("{}{plus}", summary.untracked))],
                ));
            }
            let mut text = parts.join(" · ");
            if summary.approximate {
                text.push_str(crate::i18n::tr(" (estimated)"));
            }
            let color = if summary.conflicted > 0 {
                palette.danger
            } else {
                palette.warning
            };
            (text, color)
        }
        other => (project::status_label(other), palette.text_muted),
    }
}

// The strip, when the open folder is in a repository.
//
// Narrow, it sheds words before controls: the remote and the tool note go
// first, then the status keeps only its dot; the branch and the actions
// always stay.
pub(crate) fn strip<'a>(
    model: &'a Model,
    theme: &EnzTheme,
    width: f32,
) -> Option<Element<'a, Msg>> {
    let compact = width < COMPACT_BELOW;
    let minimal = width < MINIMAL_BELOW;
    let info = model
        .project
        .current
        .as_ref()
        .filter(|info| info.dir == model.cwd)?;
    let sp = theme.spacing;
    let mut line = row![]
        .mirrored(crate::i18n::rtl())
        .spacing(sp.sm())
        .align_y(Alignment::Center);

    // The branch leads, a step brighter than the details after it.
    let head = info
        .head
        .clone()
        .unwrap_or_else(|| crate::i18n::tr("Repository").to_owned());
    line = line.push(item(
        row![
            enz_icon("git-branch").size(GLYPH).into_element(),
            enz_text(head)
                .size(TextSize::Footnote)
                .weight_rung(enzui_theme::WeightRung::Medium)
                .single_line_elided()
                .into_element(),
        ]
        .mirrored(crate::i18n::rtl())
        .spacing(sp.xs())
        .align_y(Alignment::Center),
    ));
    if let Some(operation) = info.operation {
        line = line.push(item(
            enz_badge(operation).tone(BadgeTone::Warning).into_element(),
        ));
    }

    // Where it is published, read the way a person says it; it opens the
    // project page when the forge serves one.
    if let Some(remote) = info.remote.as_deref().filter(|_| !compact) {
        line = line.push(rule());
        let name = project::remote_label(remote);
        line = line.push(item(if let Some(url) = project::remote_web_url(remote) {
            // A muted link: no button padding, so the rules stay even.
            enzui_widgets::link::enz_link(name, url, |_| Msg::Project(ProjectMsg::OpenRemote))
                .tone(enzui_widgets::link::LinkTone::Muted)
                .size(enzui_widgets::link::LinkSize::Sm)
                .external(true)
                .into_element()
        } else {
            label(name, TextTone::Muted)
        }));
    }

    // The work tree: a coloured dot, then a few words while there is room.
    line = line.push(rule());
    let (status_text, dot_color) = status(info, theme);
    let dot: Element<'a, Msg> = container(Space::new())
        .width(Length::Fixed(DOT))
        .height(Length::Fixed(DOT))
        .style(move |_| container::Style {
            background: Some(dot_color.into()),
            border: Border {
                radius: (DOT / 2.0).into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into();
    let mut status_row = row![dot]
        .mirrored(crate::i18n::rtl())
        .spacing(sp.xs())
        .align_y(Alignment::Center);
    if !minimal {
        let mut text = status_text;
        if let Some(enclosing) = info.enclosing.as_ref().and_then(|path| path.file_name()) {
            text.push_str(&crate::i18n::format(
                "files-inside-c2fc1baa",
                &[("arg0", enclosing.to_string_lossy().to_string())],
            ));
        }
        status_row = status_row.push(label(text, TextTone::Muted));
    }
    line = line.push(item(status_row));
    if info.status == StatusView::OnRequest {
        line = line.push(item(
            enz_button(
                crate::i18n::tr("Count changes"),
                Msg::Project(ProjectMsg::CountChanges),
            )
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Xs)
            .into_element(),
        ));
    }

    line = line.push(Space::new().width(Length::Fill));

    // Project tools and the trust mark only mean something to the Developer
    // package, which runs them; without it they are not offered.
    if info.open_project && !info.tools.is_empty() && !compact {
        let names = info.tools.join(", ");
        let (icon, note, toggle) = if info.trusted {
            (
                "shield-check",
                crate::i18n::format(
                    "files-names-not-run-trusted-in-files-25b2ffb3",
                    &[("names", names)],
                ),
                enz_button(crate::i18n::tr("Unmark"), Msg::Project(ProjectMsg::Untrust)),
            )
        } else {
            (
                "shield-alert",
                crate::i18n::format("files-names-not-run-3b899442", &[("names", names)]),
                enz_button(
                    crate::i18n::tr("Mark trusted"),
                    Msg::Project(ProjectMsg::Trust),
                ),
            )
        };
        line = line.push(item(
            row![
                enz_icon(icon).size(GLYPH).into_element(),
                label(note, TextTone::Muted),
            ]
            .mirrored(crate::i18n::rtl())
            .spacing(sp.xs())
            .align_y(Alignment::Center),
        ));
        line = line.push(item(
            toggle
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Xs)
                .into_element(),
        ));
        line = line.push(rule());
    }

    // The actions, one icon each.
    let mut actions = row![]
        .mirrored(crate::i18n::rtl())
        .spacing(2.0)
        .align_y(Alignment::Center);
    if info.open_project && info.kind == enzui_services::vcs::git::RepositoryKind::WorkTree {
        actions = actions.push(action(
            "folder-code",
            crate::i18n::tr("Open in your development application"),
            Msg::Project(ProjectMsg::OpenProject),
        ));
    }
    if info.remote.is_some() {
        actions = actions.push(action(
            "link",
            crate::i18n::tr("Copy remote address"),
            Msg::Project(ProjectMsg::CopyRemote),
        ));
    }
    if info.kind != enzui_services::vcs::git::RepositoryKind::Bare {
        actions = actions.push(action(
            "copy",
            crate::i18n::tr("Copy path within the repository"),
            Msg::Project(ProjectMsg::CopyRelativePath),
        ));
    }
    actions = actions.push(action(
        "terminal",
        crate::i18n::tr("Open in Terminal"),
        Msg::OpenInTerminal(None),
    ));
    line = line.push(actions);

    let palette = theme.palette.clone();
    Some(
        container(line)
            .padding([2.0, sp.md()])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(palette.surface_alt.into()),
                ..container::Style::default()
            })
            .into(),
    )
}
