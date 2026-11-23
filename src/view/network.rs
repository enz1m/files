// Network tab: GVfs-backed discovery and connection surfaces.

use enzui_core::Action;
use enzui_theme::EnzTheme;
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::page_header::enz_page_header;
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::section_header::enz_section_header;
use enzui_widgets::status_page::{StatusPageTone, enz_status_page};
use iced::widget::{column, container};
use iced::{Element, Length};

use crate::message::Msg;
use crate::model::Model;
use crate::network::{BackendStatus, DiscoveryState};

pub(crate) const TITLE: &str = "Network";
pub(crate) const CONNECT_LABEL: &str = "Connect to Server";
pub(crate) const DISCOVER_LABEL: &str = "Scan Network";
pub(crate) const OPEN_SETTINGS_LABEL: &str = "Open Network Settings";
const CONNECT_ACTION_ID: &str = "network.connect";
const OPEN_SETTINGS_ACTION_ID: &str = "network.open-settings";
// The Add a Cloud Drive action, on the Network and Computer headers.
pub(crate) const ADD_CLOUD_ACTION_ID: &str = "files-add-cloud";
pub(crate) const ADD_CLOUD_LABEL: &str = "Add Cloud Drive";

// Offered only when the mount service can add a cloud service.
pub(crate) fn add_cloud_action(model: &Model) -> Option<Action<Msg>> {
    (!model.cloud_providers.is_empty()).then(|| {
        Action::new(ADD_CLOUD_ACTION_ID, Msg::ShowAddCloud)
            .with_label(crate::i18n::tr(ADD_CLOUD_LABEL))
    })
}

// Stable focus target shared by every visual form of Network's Attach action.
//
// Retry, empty and idle states move the id from the header to their
// status-page CTA, so exactly one control carries it and dialog focus
// restoration never targets a vanished id.
pub(crate) fn connect_invoker_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(CONNECT_ACTION_ID)
}

fn connect_action() -> Action<Msg> {
    Action::new(CONNECT_ACTION_ID, Msg::ShowAttach).with_label(crate::i18n::tr(CONNECT_LABEL))
}

pub(crate) fn view<'a>(model: &'a Model, theme: &'a EnzTheme) -> Element<'a, Msg> {
    let state = &model.network;
    let body_has_connect_action = !state.pending
        && (state.operation_error.is_some()
            || matches!(
                &state.discovery,
                DiscoveryState::Idle | DiscoveryState::Empty | DiscoveryState::Failed(_)
            ));
    // Page actions stay flat in Files.
    let mut page_header = enz_page_header(crate::i18n::tr(TITLE))
        .primary_action_kind(enzui_widgets::button::ButtonKind::Quiet);
    if !body_has_connect_action {
        page_header = page_header.primary_action(connect_action());
    }
    if let Some(action) = add_cloud_action(model) {
        page_header = page_header.secondary_action(action);
    }
    // Scanning is a quiet icon command beside the title; without the
    // discovery backend it is disabled and says why.
    let (icon, message, tip) = if state.pending {
        (
            "x",
            Msg::CancelNetworkOperation,
            crate::i18n::tr("Cancel").to_owned(),
        )
    } else if state.backend == BackendStatus::Unavailable {
        (
            "radar",
            Msg::DiscoverNetwork,
            crate::i18n::format(
                "files-discover-label-network-discovery-is-unavailable-7ffde5df",
                &[("DISCOVER_LABEL", (DISCOVER_LABEL).to_owned())],
            ),
        )
    } else {
        (
            "radar",
            Msg::DiscoverNetwork,
            crate::i18n::tr(DISCOVER_LABEL).to_owned(),
        )
    };
    let scan = enzui_widgets::toolbar_icon::enz_toolbar_icon(icon, message)
        .enabled(state.pending || state.backend != BackendStatus::Unavailable)
        .aria_label(if state.pending {
            crate::i18n::tr("Cancel")
        } else {
            crate::i18n::tr(DISCOVER_LABEL)
        })
        .tooltip(tip);
    let header: Element<'a, Msg> = iced::widget::row![page_header.into_element(), scan]
        .align_y(iced::Alignment::Center)
        .width(Length::Fill)
        .into();

    // The backend line only speaks up while something is running; an
    // unavailable backend is explained by the page body, not in red above it.
    let quiet = matches!(
        state.backend,
        BackendStatus::Ready | BackendStatus::Unavailable
    );
    let status = (!quiet).then(|| {
        enz_text(state.backend.status_line())
            .size(TextSize::Caption)
            .tone(TextTone::Muted)
            .into_element()
    });

    // Saved servers and cloud drives from the mount service, with their
    // state and commands, as the Computer page lists them.
    let saved: Vec<_> = model
        .storage_locations
        .iter()
        .filter(|location| {
            location.service.is_some()
                && matches!(
                    location.kind,
                    crate::storage::Kind::Network | crate::storage::Kind::Cloud
                )
        })
        .collect();
    let saved_section: Option<Element<'a, Msg>> = (!saved.is_empty()).then(|| {
        let mut rows = column![
            enz_section_header::<Msg>(crate::i18n::tr("Your servers and cloud drives"))
                .into_element()
        ]
        .spacing(theme.spacing.xs())
        .width(Length::Fill);
        for location in saved {
            rows = rows.push(super::storage::location_row(location, model, theme));
        }
        rows.into()
    });

    let has_saved = saved_section.is_some();
    // The found servers list, as opposed to a status page about the scan.
    let discovered_list = !state.pending
        && state.operation_error.is_none()
        && state.discovery.message_for(&state.context.trust).is_none()
        && !matches!(state.discovery, DiscoveryState::Idle);
    let body: Element<'a, Msg> = if state.pending {
        enz_status_page(if state.pending_mount {
            crate::i18n::tr("Connecting…")
        } else {
            crate::i18n::tr("Searching for shares…")
        })
        .description(crate::i18n::tr(
            "You can cancel this operation; the window remains responsive.",
        ))
        .announce_heading(false)
        .icon("radar")
        .secondary_action(
            Action::new("network.cancel", Msg::CancelNetworkOperation)
                .with_label(crate::i18n::tr("Cancel")),
        )
        .secondary_action_kind(enzui_widgets::button::ButtonKind::Quiet)
        .into_element()
    } else if let Some(message) = &state.operation_error {
        enz_status_page(crate::i18n::tr("Could not connect"))
            .description(message.clone())
            .announce_heading(false)
            .icon("wifi-off")
            .tone(StatusPageTone::Error)
            .primary_action(connect_action())
            .into_element()
    } else if state.discovery.is_failure()
        && state.backend == BackendStatus::Unavailable
        && !state.context.trust.discovery_blocked()
    {
        // Discovery is optional: without its service the page is not broken,
        // it just cannot find servers by itself.
        enz_status_page(crate::i18n::tr("Network discovery is off"))
            .description(crate::i18n::tr(
                "Servers on this network can't be found automatically because the Avahi \
                 service isn't running. You can still connect to a server by its address.",
            ))
            .announce_heading(false)
            .icon("network")
            .tone(StatusPageTone::Empty)
            .primary_action(connect_action())
            .into_element()
    } else if let Some(message) = state.discovery.message_for(&state.context.trust) {
        let untrusted = state.context.trust.discovery_blocked();
        let failed = state.discovery.is_failure() && !untrusted;
        let mut page = enz_status_page(if failed {
            crate::i18n::tr("Network scan failed")
        } else {
            state.context.trust.empty_title()
        })
        .description(message)
        .announce_heading(false)
        .icon(if failed {
            "wifi-off"
        } else if untrusted {
            "shield-alert"
        } else {
            "network"
        })
        .tone(if failed {
            StatusPageTone::Error
        } else {
            StatusPageTone::Empty
        })
        .primary_action(connect_action());
        if untrusted && state.context.can_open_network_settings {
            page = page
                .secondary_action(
                    Action::new(OPEN_SETTINGS_ACTION_ID, Msg::OpenNetworkSettings)
                        .with_label(crate::i18n::tr(OPEN_SETTINGS_LABEL)),
                )
                .secondary_action_kind(enzui_widgets::button::ButtonKind::Secondary);
        }
        page.into_element()
    } else if matches!(state.discovery, DiscoveryState::Idle) {
        workgroup_hero(
            enz_status_page(crate::i18n::tr("Browse your network"))
                .description(crate::i18n::tr(
                    "Scan for advertised SMB shares, or connect directly by server address.",
                ))
                .announce_heading(false),
            model,
            "network",
        )
        .primary_action(
            Action::new("network.scan-empty", Msg::DiscoverNetwork)
                .with_label(crate::i18n::tr(DISCOVER_LABEL)),
        )
        .secondary_action(connect_action())
        .secondary_action_kind(enzui_widgets::button::ButtonKind::Quiet)
        .into_element()
    } else {
        let mut rows = column![].spacing(theme.spacing.xs()).width(Length::Fill);
        for (index, share) in state.discovered.iter().enumerate() {
            let uri = share.uri();
            // A discovered server has no share yet; connecting lists them.
            let title = if share.share.is_empty() {
                share.host.clone()
            } else {
                share.share.clone()
            };
            rows = rows.push(super::storage::device_row(
                super::storage::DeviceRow {
                    key: format!("share:{uri}"),
                    icon: "server",
                    // A discovered server, or one share on it.
                    art: if share.share.is_empty() {
                        &["network-server"]
                    } else {
                        &["folder-remote", "network-server"]
                    },
                    name: title,
                    status: uri.clone(),
                    warning: false,
                    usage: None,
                    primary: Some((
                        crate::i18n::tr("Connect").to_owned(),
                        Msg::MountDiscovered(index),
                    )),
                    icons: Vec::new(),
                    menu: None,
                    open: Some(Msg::MountDiscovered(index)),
                    busy: false,
                },
                model,
                theme,
            ));
        }
        rows.into()
    };

    let mut sections = vec![header];
    sections.extend(status);
    sections.extend(saved_section);
    if !discovered_list || !state.discovered.is_empty() {
        let mut found = column![].spacing(theme.spacing.xs()).width(Length::Fill);
        if discovered_list && has_saved {
            found = found
                .push(enz_section_header::<Msg>(crate::i18n::tr("On this network")).into_element());
        }
        sections.push(found.push(container(body).width(Length::Fill)).into());
    }
    enz_scroll_column(sections)
        .spacing(theme.spacing.lg())
        .inset(theme.spacing.space9)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

// The network status pages' hero: the theme's workgroup artwork when the view
// draws theme art, else `glyph`.
fn workgroup_hero<'a>(
    page: enzui_widgets::status_page::EnzStatusPage<'a, Msg>,
    model: &Model,
    glyph: &str,
) -> enzui_widgets::status_page::EnzStatusPage<'a, Msg> {
    const HERO_ART: f32 = 96.0;
    match enzui_widgets::file_icon::themed_icon(&["network-workgroup"], HERO_ART) {
        Some(art) if crate::icon::uses_art(model) => page.artwork(art.into_element(HERO_ART)),
        _ => page.icon(glyph),
    }
}
