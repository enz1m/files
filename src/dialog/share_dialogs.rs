// Conflicts, open-with, attach, downloads, matching, activity, shortcuts.

use super::*;
use iced::widget::column;

// The conflict-resolution dialog.
//
// The primary label and message both derive from the conflict kind, so `Merge`
// and `Replace` always match their operation.
pub(crate) fn conflicts<'a>(
    theme: &EnzTheme,
    queue: &crate::conflict::ConflictQueue,
    busy: bool,
    window: Size,
) -> Element<'a, Msg> {
    use crate::conflict::ConflictChoice;

    let sp = &theme.spacing;
    let Some(current) = queue.current.as_ref() else {
        return iced::widget::Space::new().into();
    };
    let checkbox_tokens = CheckboxTokens::from_palette(&theme.palette);

    let comparison = reflow_pair(
        enz_labeled_content_compact(crate::i18n::tr("Existing"))
            .value(current.dst_detail.clone())
            .into_element(),
        enz_labeled_content_compact(crate::i18n::tr("Incoming"))
            .value(current.src_detail.clone())
            .into_element(),
        360.0,
        PairArm {
            spacing: sp.md(),
            leading_width: Length::Fill,
            ..PairArm::default()
        },
        PairArm {
            spacing: sp.sm(),
            ..PairArm::default()
        },
    );
    let comparison: Element<'a, Msg> = enz_surface(comparison)
        .fill_width()
        .tone(SurfaceTone::SurfaceAlt)
        .radius_resolved(|theme| {
            enzui_widgets::theme_lookup::shape_radius(theme, |shape| shape.card).into()
        })
        .padding(sp.sm())
        .into_element();

    let apply_all = enz_checkbox(
        queue.apply_to_all,
        crate::i18n::tr("Apply to all remaining conflicts"),
        Msg::SetApplyToAllConflicts,
    )
    .tokens(checkbox_tokens)
    .into_element();

    let mut body = column![
        enz_text(crate::i18n::format(
            "files-already-exists-f61b07b3",
            &[("arg0", current.name())]
        ))
        .size(TextSize::Body)
        .into_element(),
        enz_labeled_content_compact(crate::i18n::tr("Destination folder"))
            .value(
                current
                    .dst
                    .parent()
                    .unwrap_or(&queue.dest_dir)
                    .display()
                    .to_string(),
            )
            .into_element(),
        enz_text(queue.position_label())
            .size(TextSize::Footnote)
            .tone(TextTone::Muted)
            .into_element(),
        comparison,
    ]
    .spacing(sp.sm())
    .width(Length::Fill);
    if !queue.remaining.is_empty() {
        body = body.push(apply_all);
    }
    let body = body.into();

    let mut actions = vec![Action::new(
        crate::i18n::tr("Cancel"),
        ButtonKind::Quiet,
        Msg::CancelConflicts,
    )];
    // Only the opposite of the current pause state is offered.
    if queue.paused {
        actions.push(Action::new(
            crate::i18n::tr("Resume"),
            ButtonKind::Accent,
            Msg::ResumeConflicts,
        ));
    } else {
        actions.push(Action::new(
            crate::i18n::tr("Pause"),
            ButtonKind::Quiet,
            Msg::PauseConflicts,
        ));
        actions.push(Action::new(
            crate::i18n::tr("Skip"),
            ButtonKind::Quiet,
            Msg::ResolveConflict(ConflictChoice::Skip),
        ));
        actions.push(Action::new(
            // Through the queue, so the label and choice read the same kind.
            queue.primary_label(),
            ButtonKind::Accent,
            Msg::ResolveConflict(ConflictChoice::primary_for(current.kind)),
        ));
    }

    for action in &mut actions {
        action.enabled &= !busy;
    }
    present(
        theme,
        crate::i18n::tr("File conflict"),
        body,
        actions,
        window,
    )
}

pub(crate) fn open_with<'a>(
    theme: &EnzTheme,
    state: &'a crate::open_with::OpenWithState,
    window: Size,
) -> Element<'a, Msg> {
    use crate::open_with::{behavior_from_index, behavior_index};

    let sp = &theme.spacing;
    let radio_tokens = RadioTokens::from_palette(&theme.palette);

    // One subtitle names the file; the dialog title already says "Open With".
    let header = enz_text(state.chooser_subtitle())
        .size(TextSize::Footnote)
        .tone(TextTone::Muted)
        .into_element();

    // One compact row per application: its icon, its name, and what it is
    // to this file.
    let mut chooser = column![].spacing(2.0).width(Length::Fill);
    use crate::open_with::{CandidateIcon, CandidateState};
    match &state.candidate_state {
        CandidateState::Loading => {
            chooser = chooser.push(
                enz_text(crate::i18n::tr("Finding installed applications\u{2026}"))
                    .size(TextSize::Caption)
                    .tone(TextTone::Muted)
                    .into_element(),
            );
        }
        CandidateState::Empty => {
            chooser = chooser.push(
                enz_text(crate::i18n::format(
                    "files-no-installed-applications-advertise-support-for-8c46555f",
                    &[("arg0", (state.mime).clone())],
                ))
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .into_element(),
            );
        }
        CandidateState::Unavailable(reason) => {
            chooser = chooser.push(
                enz_text(reason)
                    .size(TextSize::Caption)
                    .tone(TextTone::Danger)
                    .into_element(),
            );
        }
        CandidateState::Ready => {}
    }
    if let Some(notice) = &state.default_notice {
        chooser = chooser.push(
            enz_text(notice)
                .size(TextSize::Caption)
                .tone(TextTone::Warning)
                .into_element(),
        );
    }
    const APP_ICON: f32 = 24.0;
    for (i, cand) in state.candidates.iter().enumerate() {
        let selected = state.selected == Some(i);
        let mut icon = enzui_widgets::icon_slot::enz_icon_slot("app-window")
            .icon_size(if cand.icon.is_some() { APP_ICON } else { 20.0 })
            .slot_width(APP_ICON)
            .slot_height(APP_ICON);
        icon = match &cand.icon {
            Some(CandidateIcon::Svg(handle)) => icon.icon_source(handle.clone()),
            Some(CandidateIcon::Raster(handle)) => icon.icon_source(handle.clone()),
            None => icon,
        };
        let icon: Element<'a, Msg> = icon.into();
        let role = if cand.is_default {
            Some(crate::i18n::tr("Default"))
        } else if cand.as_text {
            Some(crate::i18n::tr("Opens it as text"))
        } else {
            None
        };
        let mut text = column![
            enz_text(cand.name.clone())
                .size(TextSize::Body)
                .single_line_elided()
                .into_element()
        ]
        .width(Length::Fill);
        if let Some(role) = role {
            text = text.push(
                enz_text(role)
                    .size(TextSize::Caption)
                    .tone(TextTone::Muted)
                    .into_element(),
            );
        }
        let mut content = row![icon, text]
            .mirrored(crate::i18n::rtl())
            .spacing(sp.sm())
            .align_y(iced::Alignment::Center);
        if selected {
            content = content.push(
                enzui_widgets::icon::enz_icon("check")
                    .size(16.0)
                    .into_element(),
            );
        }
        chooser = chooser.push(
            enzui_widgets::list_row_surface::enz_list_row_surface(
                content,
                Msg::SelectOpenCandidate(i),
            )
            .selectable(true)
            .selected(selected)
            .row_height(44.0)
            .label(cand.rendered_name().to_owned())
            .id_prefix(format!("files-app-open-with-{i}"))
            .into_element(),
        );
    }
    // A long list scrolls inside the dialog instead of stretching it: up to
    // seven rows show, plus any status line above them.
    let chooser: Element<'a, Msg> = if state.candidates.len() > 7 {
        iced::widget::container(
            enzui_widgets::scroll_view::enz_scroll_view(chooser).width(Length::Fill),
        )
        .height(Length::Fixed(7.0 * 46.0))
        .into()
    } else {
        chooser.into()
    };

    let behavior = enz_radio_group(
        crate::i18n::tr("Open Behavior"),
        crate::i18n::radio_labels("behavior"),
        Some(behavior_index(state.behavior)),
        |i| Msg::SetOpenBehavior(behavior_from_index(i)),
    )
    .orientation(Orientation::Vertical)
    .spacing(sp.xs())
    .tokens(radio_tokens)
    .option_id_prefix("files-app-open-behavior")
    .into_element();

    let custom = enz_preferences_row(crate::i18n::tr("Or type a command"))
        .entry(
            &state.custom_command,
            "e.g. vim %f",
            Msg::OpenWithCommandChanged,
        )
        .into_element();

    let body: Element<'a, Msg> = column![header, chooser, behavior, custom]
        .spacing(sp.md())
        .width(Length::Fill)
        .into();

    present(
        theme,
        crate::i18n::tr("Open With"),
        body,
        vec![
            Action::new(
                crate::i18n::tr("Cancel"),
                ButtonKind::Quiet,
                Msg::CloseOpenWith,
            ),
            Action::new(
                crate::i18n::tr("Open"),
                ButtonKind::Accent,
                Msg::ConfirmOpenWith,
            ),
        ],
        window,
    )
}

// The Attach network location dialog.
//
// The password uses [`enz_secure_field`](enzui_widgets::secure_field), labelled
// from the one requirement state. Discovered hosts render as rows that fill the
// form.
pub(crate) fn attach<'a>(
    theme: &EnzTheme,
    state: &'a crate::network::AttachState,
    window: Size,
) -> Element<'a, Msg> {
    use crate::network::{
        PROTOCOL_LABELS, USERNAME_FIELD_LABEL, protocol_from_index, protocol_index,
    };
    use enzui_widgets::secure_field::enz_secure_field;

    // The mount service asks for what a server needs as it connects.
    if let Some(target) = &state.service_target {
        return super::storage_dialogs::sign_in(theme, state, target, window);
    }
    if state.service {
        return super::storage_dialogs::connect_to_server(theme, state, window);
    }
    let sp = &theme.spacing;
    let radio_tokens = RadioTokens::from_palette(&theme.palette);

    let protocol = enz_radio_group(
        crate::i18n::tr("Protocol"),
        &PROTOCOL_LABELS,
        Some(protocol_index(state.protocol)),
        |i| Msg::SetAttachProtocol(protocol_from_index(i)),
    )
    .orientation(Orientation::Horizontal)
    .spacing(sp.sm())
    .tokens(radio_tokens)
    .option_id_prefix("files-app-attach-protocol")
    .into_element();

    let server = enz_preferences_row(crate::i18n::tr("Server"))
        .entry(&state.server, "fileserver.local", Msg::AttachServerChanged)
        .into_element();
    let share = enz_preferences_row(crate::i18n::tr("Share"))
        .entry(&state.share, "public", Msg::AttachShareChanged)
        .into_element();
    let domain = enz_preferences_row(crate::i18n::tr("Domain / workgroup (optional)"))
        .entry(&state.domain, "WORKGROUP", Msg::AttachDomainChanged)
        .into_element();
    // Optional: an empty username mounts anonymously.
    let username = enz_preferences_row(crate::i18n::tr(USERNAME_FIELD_LABEL))
        .entry(&state.username, "guest", Msg::AttachUsernameChanged)
        .into_element();

    // The plaintext is borrowed for one view pass; the allocation is wiped when
    // the last `SecretText` handle drops.
    let password = enz_secure_field(
        state.password.expose_secret(),
        Msg::AttachPasswordChanged,
        state.password_revealed,
        Msg::ToggleAttachPasswordRevealed,
    )
    .label(state.password_requirement.label())
    .placeholder("")
    .id_prefix("files-app-attach-password")
    .into_element();
    let remember = enz_checkbox(
        state.remember_password,
        crate::i18n::tr("Remember password in the keyring"),
        Msg::SetAttachRememberPassword,
    )
    .tokens(CheckboxTokens::from_palette(&theme.palette))
    .into_element();

    let discover_button: Element<'a, Msg> = row![
        Space::new().width(Length::Fill),
        enz_button(
            if state.backend == crate::network::BackendStatus::Working {
                crate::i18n::tr("Cancel scan")
            } else {
                crate::i18n::tr("Discover SMB")
            },
            if state.backend == crate::network::BackendStatus::Working {
                Msg::CancelNetworkOperation
            } else {
                Msg::DiscoverSmb
            },
        )
        .kind(ButtonKind::Quiet)
        .into_element(),
    ]
    .mirrored(crate::i18n::rtl())
    .into();
    let mut discovery = column![
        enz_section_header(crate::i18n::tr("Discover")).into_element(),
        discover_button,
    ]
    .spacing(sp.xs())
    .width(Length::Fill);
    for (i, sh) in state.discovered.iter().enumerate() {
        discovery = discovery.push(
            enz_preferences_row(&sh.host)
                .subtitle(&sh.share)
                .action(crate::i18n::tr("Use"), Msg::AdoptDiscovered(i))
                .into_element(),
        );
    }
    if let Some(msg) = state.discovery.message_for(&state.context.trust) {
        let tone = if state.discovery.is_failure() && !state.context.trust.discovery_blocked() {
            TextTone::Danger
        } else {
            TextTone::Muted
        };
        discovery = discovery.push(
            enz_text(msg)
                .size(TextSize::Caption)
                .tone(tone)
                .into_element(),
        );
    }

    let backend = enz_text(state.backend.status_line())
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element();

    let body: Element<'a, Msg> = column![
        protocol,
        server,
        share,
        domain,
        username,
        password,
        remember,
        Element::from(discovery),
        backend,
    ]
    .spacing(sp.md())
    .width(Length::Fill)
    .into();

    present(
        theme,
        crate::i18n::tr("Attach network location"),
        body,
        vec![
            Action::new(
                crate::i18n::tr("Cancel"),
                ButtonKind::Quiet,
                Msg::CloseAttach,
            ),
            Action::new(
                crate::i18n::tr("Attach"),
                ButtonKind::Accent,
                Msg::ConfirmAttach,
            ),
        ],
        window,
    )
}

// The Add-download dialog for an HTTP(S) URL or magnet link.
//
// The reducer re-checks the URI and, for torrents, the backend before opening the
// SaveFile portal; queue admission waits for its grant.
pub(crate) fn add_download<'a>(
    theme: &EnzTheme,
    state: &'a crate::downloads::AddDownloadState,
    torrent_backend: crate::downloads::Aria2Status,
    window: Size,
) -> Element<'a, Msg> {
    use crate::downloads::{
        ADD_DOWNLOAD_HINT, ADD_DOWNLOAD_LABEL, ADD_DOWNLOAD_PLACEHOLDER, ENQUEUE_LABEL,
    };

    let sp = &theme.spacing;

    let hint = enz_text(crate::i18n::tr(ADD_DOWNLOAD_HINT))
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element();

    // Full width: this is the dialog's primary content.
    let entry = enz_text_field(
        crate::i18n::tr("URL or magnet link"),
        ADD_DOWNLOAD_PLACEHOLDER,
        &state.url,
        Msg::AddDownloadUrlChanged,
    )
    .input_purpose(iced::advanced::input_method::Purpose::Url)
    .size(TextFieldSize::Md)
    .into_element();

    let speed_limit = enz_text_field(
        crate::i18n::tr("Speed limit (KiB/s; 0 = unlimited)"),
        "0",
        &state.speed_limit_kib,
        Msg::AddDownloadLimitChanged,
    )
    .input_purpose(iced::advanced::input_method::Purpose::Digits)
    .size(TextFieldSize::Md)
    .into_element();
    let destination = enz_text(crate::i18n::format(
        "files-destination-change-in-the-system-save-d54b1a5b",
        &[("arg0", (state.destination_hint.display()).to_string())],
    ))
    .size(TextSize::Caption)
    .tone(TextTone::Muted)
    .into_element();
    let advanced_content = column![speed_limit, destination]
        .spacing(sp.sm())
        .width(Length::Fill);
    let advanced_body: Element<'a, Msg> = advanced_content.into();
    let advanced = enz_expander(crate::i18n::tr("Advanced"), state.advanced_open, |_| {
        Msg::ToggleDownloadAdvanced
    })
    .content(advanced_body)
    .into_element();

    let mut content = column![hint, entry, advanced]
        .spacing(sp.md())
        .width(Length::Fill);
    if let Some(error) = &state.error {
        content = content.push(
            enz_text(error.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Danger),
        );
    } else if state.is_media_site() {
        // Said as soon as such a link is typed, since adding it stays disabled.
        content = content.push(
            enz_text(crate::i18n::tr(crate::downloads::MEDIA_SITE_UNSUPPORTED))
                .size(TextSize::Caption)
                .tone(TextTone::Warning),
        );
    }
    let body: Element<'a, Msg> = content.into();

    present(
        theme,
        crate::i18n::tr(ADD_DOWNLOAD_LABEL),
        body,
        vec![
            Action::new(
                crate::i18n::tr("Cancel"),
                ButtonKind::Quiet,
                Msg::CloseAddDownload,
            ),
            // Disabled until the entry is acceptable.
            Action::new(
                crate::i18n::tr(ENQUEUE_LABEL),
                ButtonKind::Accent,
                Msg::EnqueueDownload,
            )
            .enabled(state.can_enqueue(torrent_backend)),
        ],
        window,
    )
}

// The Select items matching glob dialog (Ctrl+S).
//
// `Msg::SelectMatchingSubmit` replaces the selection with every visible entry
// matching the pattern.
pub(crate) fn select_matching<'a>(
    theme: &EnzTheme,
    model: &'a Model,
    window: Size,
) -> Element<'a, Msg> {
    use crate::view::shell::{
        SELECT_MATCHING_HINT, SELECT_MATCHING_PLACEHOLDER, SELECT_MATCHING_TITLE,
    };

    let sp = &theme.spacing;

    let hint = enz_text(crate::i18n::tr(SELECT_MATCHING_HINT))
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element();

    let entry = enz_preferences_row(crate::i18n::tr("Pattern"))
        .entry(
            &model.select_matching_draft,
            SELECT_MATCHING_PLACEHOLDER,
            Msg::SelectMatchingDraftChanged,
        )
        .into_element();

    let body: Element<'a, Msg> = column![hint, entry]
        .spacing(sp.md())
        .width(Length::Fill)
        .into();

    present(
        theme,
        crate::i18n::tr(SELECT_MATCHING_TITLE),
        body,
        vec![
            Action::new(
                crate::i18n::tr("Cancel"),
                ButtonKind::Quiet,
                Msg::CloseSelectMatching,
            ),
            Action::new(
                crate::i18n::tr("Select"),
                ButtonKind::Accent,
                Msg::SelectMatchingSubmit,
            ),
        ],
        window,
    )
}

pub(crate) const FOLDER_COLOR_TITLE: &str = "Folder Color";

// The colour chooser for the folder at `path`. A swatch applies at once;
// *Default* clears the tag.
pub(crate) fn folder_color<'a>(
    theme: &EnzTheme,
    model: &'a Model,
    path: &'a std::path::Path,
    window: Size,
) -> Element<'a, Msg> {
    use enzui_widgets::workspace_accent_chooser::enz_workspace_accent_chooser;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let palette = enzui_theme::WorkspaceAccentPalette::for_mode(&theme.palette);
    let target = path.to_path_buf();
    let chooser: Element<'a, Msg> =
        enz_workspace_accent_chooser(palette, model.folder_color(path), move |key| {
            Msg::SetFolderColor(target.clone(), key)
        })
        .aria_label(crate::i18n::tr(FOLDER_COLOR_TITLE))
        .into();
    let hint = enz_text(crate::i18n::format(
        "files-choose-how-name-is-marked-in-2e2ed08b",
        &[("name", (name))],
    ))
    .size(TextSize::Caption)
    .tone(TextTone::Muted)
    .into_element();
    let body: Element<'a, Msg> = column![hint, chooser]
        .spacing(theme.spacing.md())
        .width(Length::Fill)
        .into();
    present(
        theme,
        crate::i18n::tr(FOLDER_COLOR_TITLE),
        body,
        vec![Action::new(
            crate::i18n::tr("Done"),
            ButtonKind::Accent,
            Msg::CloseFolderColor,
        )],
        window,
    )
}

pub(crate) const ALL_ACTIVITY_TITLE: &str = "All activity";

pub(crate) const ALL_ACTIVITY_LIST_LABEL: &str = "All activity operations";

// The accessible-name prefix of one all-activity row.
//
// Distinct from [`enzui_widgets::activity_surface::ROW_LABEL_PREFIX`], so
// gates can tell the bounded surface from this exhaustive list.

pub(crate) const ALL_ACTIVITY_EMPTY: &str = "Nothing is running.";

// One all-activity row, read out of the centre before the dialog is built.
struct AllActivityRow {
    key: enzui_core::operation_activity::ActivityKey,
    label: String,
    detail: Option<String>,
    announcement: String,
    fraction: Option<f32>,
    controls: Vec<enzui_core::operation_activity::ActivityControl>,
}

// Every row the activity centre holds, in the surface's attention order.
//
// The `+N more` destination when the hidden rows span providers. Order and prose
// come from `ActivityCenter::view` and `Activity::announcement`.
pub(crate) fn all_activity<'a>(
    theme: &EnzTheme,
    center: &enzui_core::operation_activity::ActivityCenter,
    window: Size,
) -> Element<'a, Msg> {
    use enzui_core::operation_activity::ActivityControl;
    use enzui_core::semantics::{Role, Semantics};
    use enzui_widgets::gauge::Sampled;
    use enzui_widgets::progress_bar::enz_progress_bar_sampled;
    use enzui_widgets::semantics_container::SemanticsContainer;

    let sp = &theme.spacing;

    // Owned prose, so the unbudgeted clone drops before the `'a` element is built.
    let expanded = center.clone().with_row_budget(usize::MAX);
    let read: Vec<AllActivityRow> = {
        let view = expanded.view();
        debug_assert_eq!(view.overflow, 0, "the all-activity list withholds nothing");
        view.rows
            .iter()
            .map(|row| AllActivityRow {
                key: row.key,
                label: row.label.clone(),
                detail: row.detail.clone(),
                announcement: row.announcement(),
                fraction: row.progress.fraction(),
                controls: row.controls.clone(),
            })
            .collect()
    };

    let body: Element<'a, Msg> = if read.is_empty() {
        enz_text(crate::i18n::tr(ALL_ACTIVITY_EMPTY))
            .size(TextSize::Body)
            .tone(TextTone::Muted)
            .into_element()
    } else {
        let mut list = column![].spacing(sp.md()).width(Length::Fill);
        for row in read {
            let AllActivityRow {
                key,
                label,
                detail,
                announcement,
                fraction,
                controls,
            } = row;
            let mut lines = column![enz_text(label.clone()).size(TextSize::Body)]
                .spacing(sp.xs())
                .width(Length::Fill);
            if let Some(detail) = detail {
                lines = lines.push(
                    enz_text(detail)
                        .size(TextSize::Caption)
                        .tone(TextTone::Muted),
                );
            }
            let sample: Sampled<f32> = fraction.into();
            lines = lines.push(enz_progress_bar_sampled(sample));
            // The row's own controls, as quiet icon buttons beside it: the
            // sidebar status is only a summary, so this is where an operation
            // is paused, resumed, retried or cancelled.
            let mut buttons = iced::widget::row![]
                .spacing(sp.xs())
                .align_y(iced::Alignment::Center);
            for control in controls {
                let icon = match control {
                    ActivityControl::Pause => "pause",
                    ActivityControl::Resume => "play",
                    ActivityControl::Cancel => "x",
                    ActivityControl::Retry => "rotate-cw",
                    ActivityControl::Resolve => "circle-question-mark",
                    ActivityControl::OpenPanel => "arrow-up-right",
                };
                buttons = buttons.push(
                    enzui_widgets::toolbar_icon::enz_toolbar_icon(
                        icon,
                        Msg::ActivityControl(key, control),
                    )
                    .aria_label(control.label())
                    .tooltip(control.label()),
                );
            }
            let content = iced::widget::row![lines, buttons]
                .mirrored(crate::i18n::rtl())
                .spacing(sp.md())
                .align_y(iced::Alignment::Center)
                .width(Length::Fill);
            list = list.push(SemanticsContainer::new(
                content,
                Semantics::new(Role::ListItem)
                    .label(crate::i18n::format(
                        "files-all-activity-row",
                        &[("activity", label)],
                    ))
                    .description(announcement),
            ));
        }
        SemanticsContainer::new(
            list,
            Semantics::new(Role::Group).label(crate::i18n::tr(ALL_ACTIVITY_LIST_LABEL)),
        )
        .into()
    };

    present(
        theme,
        crate::i18n::tr(ALL_ACTIVITY_TITLE),
        body,
        vec![Action::new(
            crate::i18n::tr("Close"),
            ButtonKind::Quiet,
            Msg::CloseAllActivity,
        )],
        window,
    )
}
