use crate::message::Msg;
use crate::model::Model;
use crate::storage::{Kind, Location};
use enzui_theme::EnzTheme;
use enzui_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_widgets::enz_text::TextSize;
use enzui_widgets::icon::{IconTone, enz_icon};
use enzui_widgets::list_row_surface::enz_list_row_surface;
use enzui_widgets::sidebar_row::{SidebarRowTokens, enz_sidebar_row};
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use iced::widget::{column, container};
use iced::{Element, Length};

pub(crate) fn sidebar_row<'a>(
    location: &Location,
    model: &Model,
    theme: &EnzTheme,
) -> Element<'a, Msg> {
    let selected = model.screen == crate::model::Screen::Directory
        && location.contains(&model.cwd)
        && !model.storage_locations.iter().any(|other| {
            other.id != location.id
                && other.contains(&model.cwd)
                && other.mount.as_ref().is_some_and(|root| {
                    location
                        .mount
                        .as_ref()
                        .is_some_and(|parent| root.starts_with(parent) && root != parent)
                })
        });
    let busy = model.storage_busy.contains(&location.id);
    let mut item = enz_sidebar_row(location.name.clone(), Msg::OpenStorage(location.id.clone()))
        .id_prefix(format!("files-storage-{}", location.id))
        .icon(crate::storage::icon(location.kind))
        .selected(selected)
        .enabled(!busy)
        .reduced_motion(true)
        .tokens(SidebarRowTokens::from_palette(&theme.palette))
        .sizing(theme.sizing);
    if !crate::mount_service::location_actions(location).is_empty() {
        let id = location.id.clone();
        item = item
            .on_context(move |at| Msg::OpenLocationContextMenu(id.clone(), at))
            .context_label(crate::i18n::format(
                "files-actions-for-name-3f1d6a2c",
                &[("name", location.name.clone())],
            ));
    }
    // The trailing slot says what the location is waiting for: work under
    // way, a passphrase, a server that stopped answering, or that it can be
    // unmounted.
    let service = location.service.as_ref();
    if busy || service.is_some_and(|s| s.busy || s.formatting || s.signing_in) {
        item = item.trailing(
            enzui_widgets::spinner::enz_spinner()
                .size(theme.sizing.icon_sm)
                .into_element(),
        );
    } else if let Some(service) = service.filter(|s| s.not_responding || s.failed) {
        item = item.trailing(status_icon(
            "triangle-alert",
            IconTone::Warning,
            service.reason.clone().unwrap_or_default(),
            theme,
        ));
    } else if service.is_some_and(|s| s.locked) {
        item = item.trailing(status_icon(
            "lock",
            IconTone::Muted,
            crate::i18n::tr("Locked. Click to unlock.").to_owned(),
            theme,
        ));
    } else if location.can_unmount {
        // A server is disconnected; a drive is unmounted.
        let label = if matches!(location.kind, Kind::Network | Kind::Cloud) && service.is_some() {
            crate::i18n::format("files-disconnect-name", &[("name", location.name.clone())])
        } else {
            crate::i18n::format(
                "files-unmount-274e6add",
                &[("arg0", (location.name).clone())],
            )
        };
        item = item.trailing(
            enz_toolbar_icon(
                "arrow-up-from-line",
                Msg::UnmountStorage(location.id.clone()),
            )
            .aria_label(label.clone())
            .tooltip(label)
            .enabled(!busy)
            .into_element(),
        );
    }

    item.into_element()
}

// A small state icon in a sidebar row's trailing slot, explained on hover.
fn status_icon<'a>(name: &str, tone: IconTone, tip: String, theme: &EnzTheme) -> Element<'a, Msg> {
    let icon = container(
        enz_icon(name)
            .size(theme.sizing.icon_sm)
            .tint_theme_tone(tone)
            .into_element(),
    )
    .center(Length::Fixed(theme.sizing.icon_sm + theme.spacing.sm()))
    .into();
    enzui_widgets::tooltip::enz_tooltip(icon, tip, iced::widget::tooltip::Position::Top)
}

// The width of the usage column on Computer and Network rows, so every bar
// starts and ends at the same x.
pub(crate) const USAGE_WIDTH: f32 = 140.0;
// The width of the trailing actions column: a compact button and two icon
// buttons, or three icon buttons.
pub(crate) const ACTIONS_WIDTH: f32 = 176.0;

// One row of the Computer and Network pages, in the Downloads page's shape:
// an icon, the name over one muted status line, a usage bar in a fixed
// column, and the actions in a fixed trailing column.
pub(crate) struct DeviceRow<'a> {
    // Stable key: the row's identity and its double-click latch.
    pub(crate) key: String,
    pub(crate) icon: &'a str,
    // Icon-theme names drawn instead of `icon` when the theme has one.
    pub(crate) art: &'static [&'static str],
    pub(crate) name: String,
    pub(crate) status: String,
    // The status is a problem (a server stopped answering).
    pub(crate) warning: bool,
    // Used fraction of the space, when it is known and the location is open.
    pub(crate) usage: Option<f32>,
    // The one natural next step, as a compact button.
    pub(crate) primary: Option<(String, Msg)>,
    // Further commands as icon buttons: (icon, tooltip, message).
    pub(crate) icons: Vec<(&'a str, String, Msg)>,
    // A mount service location id: the row gets the ⋯ menu and right-click.
    pub(crate) menu: Option<String>,
    // What double-click and Enter do.
    pub(crate) open: Option<Msg>,
    pub(crate) busy: bool,
}

// The ⋯ button's context target, which anchors the location menu.
pub(crate) fn location_menu_target_id(id: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id("files-location-menu", id)
}

// Theme artwork beside a two-line device row.
const DEVICE_ART: f32 = 32.0;

pub(crate) fn device_row<'a>(
    row: DeviceRow<'a>,
    model: &Model,
    theme: &EnzTheme,
) -> Element<'a, Msg> {
    use enzui_widgets::enz_text::{TextTone, enz_text};
    let sp = &theme.spacing;
    let text = column![
        enz_text(row.name.clone())
            .size(TextSize::Body)
            .single_line_elided()
            .full_width(),
        enz_text(row.status)
            .size(TextSize::Caption)
            .tone(if row.warning {
                TextTone::Warning
            } else {
                TextTone::Muted
            })
            .single_line_elided()
            .full_width(),
    ]
    .spacing(sp.space1)
    .width(Length::Fill);
    let usage: Element<'a, Msg> = match row.usage {
        Some(used) => super::computer::capacity_bar(used, &theme.palette),
        None => iced::widget::Space::new().into(),
    };
    // Fixed slots from the trailing edge: ⋯, eject, open. A compact button
    // takes the place of the icons; empty slots keep their room so the same
    // control sits at the same x on every row.
    // The toolbar icon button's square.
    let slot = 28.0;
    let gap = sp.xs();
    let icon_button = |(icon, tip, message): (&'a str, String, Msg)| -> Element<'a, Msg> {
        enz_toolbar_icon(icon, message)
            .aria_label(tip.clone())
            .tooltip(tip)
            .into_element()
    };
    let empty = || -> Element<'a, Msg> { iced::widget::Space::new().width(slot).into() };
    let mut actions = iced::widget::row![]
        .spacing(gap)
        .align_y(iced::Alignment::Center);
    if row.busy {
        actions = actions.push(
            container(
                enzui_widgets::spinner::enz_spinner()
                    .size(theme.sizing.icon_sm)
                    .into_element(),
            )
            .center_x(slot),
        );
    } else if let Some((label, message)) = row.primary {
        for icon in row.icons {
            actions = actions.push(icon_button(icon));
        }
        actions = actions.push(
            enz_button(label, message)
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm)
                .into_element(),
        );
    } else {
        let mut icons = row.icons.into_iter();
        let open = icons.next();
        let eject = icons.next();
        for icon in icons {
            actions = actions.push(icon_button(icon));
        }
        actions = actions.push(open.map_or_else(empty, icon_button));
        actions = actions.push(eject.map_or_else(empty, icon_button));
    }
    match &row.menu {
        Some(id) => {
            let tip = crate::i18n::tr("More actions");
            let kebab = enz_toolbar_icon("ellipsis", Msg::ShowLocationMenu(id.clone()))
                .aria_label(tip)
                .tooltip(tip)
                .into_element();
            let target = id.clone();
            actions = actions.push(
                enzui_widgets::context_target::enz_context_target(kebab, move |at| {
                    Msg::OpenLocationContextMenu(target.clone(), at)
                })
                .id(location_menu_target_id(id))
                .width(Length::Shrink)
                .height(Length::Shrink)
                .into_element(),
            );
        }
        None => actions = actions.push(empty()),
    }
    // Theme artwork takes a larger slot (it is drawn on a padded canvas);
    // glyph rows centre in the same slot so every name starts at one x.
    let art_slot = crate::icon::uses_art(model).then_some(DEVICE_ART);
    let leading: Element<'a, Msg> = match art_slot
        .filter(|_| !row.art.is_empty())
        .and_then(|size| enzui_widgets::file_icon::themed_icon(row.art, size))
    {
        Some(art) => art.into_element(DEVICE_ART),
        None => {
            let glyph = enz_icon(row.icon)
                .size(theme.sizing.icon_md)
                .tint_theme_tone(if row.warning {
                    IconTone::Warning
                } else {
                    IconTone::Muted
                })
                .into_element();
            match art_slot {
                Some(slot) => container(glyph).center_x(Length::Fixed(slot)).into(),
                None => glyph,
            }
        }
    };
    let body = iced::widget::row![
        leading,
        text,
        container(usage)
            .width(Length::Fixed(USAGE_WIDTH))
            .center_y(Length::Shrink),
        container(actions.mirrored(crate::i18n::rtl()))
            .width(Length::Fixed(ACTIONS_WIDTH))
            .align_x(if crate::i18n::rtl() {
                iced::Alignment::Start
            } else {
                iced::Alignment::End
            }),
    ]
    .mirrored(crate::i18n::rtl())
    .spacing(sp.lg())
    .align_y(iced::Alignment::Center)
    .width(Length::Fill);
    let content = container(body)
        .padding(iced::Padding::from([sp.sm(), sp.lg()]))
        .width(Length::Fill);
    let selected = model.device_row_selected.as_deref() == Some(row.key.as_str());
    let label = crate::i18n::format("files-open-de7585f8", &[("arg0", row.name.clone())]);
    let id = enzui_core::identity::derive_id("files-device-row", &row.key);
    let mut surface = match row.open {
        Some(open) => {
            let enter = open.clone();
            let press = match open {
                Msg::MountDiscovered(_) | Msg::LocationShareChosen(_) => open,
                open => Msg::DeviceRowPressed {
                    key: row.key.clone(),
                    open: Box::new(open),
                },
            };
            enz_list_row_surface(content, press)
                .on_enter(move || enter.clone())
                .selectable(true)
                .selected(selected)
        }
        None => enzui_widgets::list_row_surface::enz_list_row_surface_lazy(content),
    };
    if let Some(menu) = row.menu {
        surface = surface.on_context(move |at| Msg::OpenLocationContextMenu(menu.clone(), at));
    }
    surface
        .id(id)
        .label(label)
        .content_height(true)
        .content_padding(0.0)
        .width(Length::Fill)
        .into()
}

// "21.4 GB free of 31.9 GB", when the location is open and its space known.
fn space_of(location: &Location, model: &Model) -> Option<(f32, String)> {
    location.mount.as_ref()?;
    let (free, total) = model
        .drives
        .iter()
        .find(|drive| location.mount.as_ref() == Some(&drive.mount) && drive.total > 0)
        .map(|drive| (drive.free, drive.total))
        .or_else(|| location.service.as_ref()?.space)?;
    let used = (total.saturating_sub(free)) as f64 / total as f64;
    Some((
        used.clamp(0.0, 1.0) as f32,
        crate::i18n::format(
            "files-free-of-39972326",
            &[
                ("arg0", crate::services::format_bytes(free)),
                ("arg1", crate::services::format_bytes(total)),
            ],
        ),
    ))
}

// A storage location (drive, phone, server, cloud drive) as a Computer or
// Network row. The next step is a compact button (Unlock…, Connect), opening
// and ejecting are icon buttons, and everything else is in the ⋯ menu, the
// same menu as right-click.
pub(crate) fn location_row<'a>(
    location: &Location,
    model: &Model,
    theme: &EnzTheme,
) -> Element<'a, Msg> {
    use crate::mount_service::LocationAction;
    let service = location.service.as_ref();
    let busy = model.storage_busy.contains(&location.id)
        || service.is_some_and(|s| s.busy || s.formatting || s.signing_in);
    let warning = service.is_some_and(|s| s.not_responding || s.failed);
    let network = matches!(location.kind, Kind::Network | Kind::Cloud);
    let space = space_of(location, model);
    // Open and with space: the space first, then what it is.
    let status = match (&space, service) {
        (Some((_, free)), Some(service)) if !service.busy && !service.formatting => {
            let what = if location.kind == Kind::Network {
                service.uri.clone()
            } else if network {
                None
            } else {
                // The drive's size is already in the free-of-total part.
                service
                    .detail
                    .as_deref()
                    .map(|detail| match detail.split_once(" · ") {
                        Some((size, rest))
                            if size.starts_with(|c: char| c.is_ascii_digit())
                                && size.ends_with('B') =>
                        {
                            rest.to_owned()
                        }
                        _ => detail.to_owned(),
                    })
            };
            match what {
                Some(what) => crate::i18n::format(
                    "files-state-detail",
                    &[("state", free.clone()), ("detail", what)],
                ),
                None => free.clone(),
            }
        }
        _ => crate::storage::description(location),
    };
    let id = location.id.clone();
    let open =
        (location.mount.is_some() || location.can_mount || service.is_some_and(|s| s.signing_in))
            .then(|| Msg::OpenStorage(id.clone()));
    let mut icons: Vec<(&str, String, Msg)> = Vec::new();
    let named = |key: &str| crate::i18n::format(key, &[("name", location.name.clone())]);
    if service.is_some() {
        let actions = crate::mount_service::location_actions(location);
        let has = |action| actions.contains(&action);
        let mut primary = None;
        if has(LocationAction::Unlock) {
            primary = Some((
                LocationAction::Unlock.label().to_owned(),
                LocationAction::Unlock,
            ));
        } else if has(LocationAction::Connect) {
            primary = Some((
                LocationAction::Connect.label().to_owned(),
                LocationAction::Connect,
            ));
        } else if has(LocationAction::ShowSignIn) {
            primary = Some((
                LocationAction::ShowSignIn.label().to_owned(),
                LocationAction::ShowSignIn,
            ));
        }
        if location.mount.is_some() && has(LocationAction::Open) {
            icons.push((
                "folder-open",
                named("files-open-name"),
                Msg::LocationAction(id.clone(), LocationAction::Open),
            ));
        }
        // One eject: safe removal for a drive that can come out, otherwise
        // unmounting or disconnecting.
        if has(LocationAction::SafelyRemove) && location.mount.is_some() {
            icons.push((
                "arrow-up-from-line",
                named("files-safely-remove-name"),
                Msg::LocationAction(id.clone(), LocationAction::SafelyRemove),
            ));
        } else if has(LocationAction::Disconnect) {
            icons.push((
                "arrow-up-from-line",
                named("files-disconnect-name"),
                Msg::LocationAction(id.clone(), LocationAction::Disconnect),
            ));
        } else if has(LocationAction::Unmount) {
            icons.push((
                "arrow-up-from-line",
                named("files-unmount-name"),
                Msg::LocationAction(id.clone(), LocationAction::Unmount),
            ));
        }
        let primary =
            primary.map(|(label, action)| (label, Msg::LocationAction(id.clone(), action)));
        return device_row(
            DeviceRow {
                key: location.id.clone(),
                icon: crate::storage::icon(location.kind),
                art: crate::storage::art(location),
                name: location.name.clone(),
                status,
                warning,
                usage: space.map(|(used, _)| used),
                primary,
                icons,
                menu: (!actions.is_empty() || service.is_some_and(|s| s.signing_in))
                    .then(|| id.clone()),
                open,
                busy: busy && !service.is_some_and(|s| s.signing_in),
            },
            model,
            theme,
        );
    }
    // A location from the desktop's own storage monitor.
    let mut primary = None;
    if location.mount.is_some() {
        icons.push((
            "folder-open",
            named("files-open-name"),
            Msg::OpenStorage(id.clone()),
        ));
    } else if location.can_mount {
        primary = Some((
            crate::i18n::tr("Mount").to_owned(),
            Msg::OpenStorage(id.clone()),
        ));
    }
    if location.can_unmount {
        icons.push((
            "arrow-up-from-line",
            named("files-unmount-name"),
            Msg::UnmountStorage(id),
        ));
    }
    if let Some(device) = model
        .removable_devices
        .iter()
        .find(|device| location.device.as_ref() == Some(&device.device))
    {
        for operation in device
            .operations()
            .into_iter()
            .filter(|operation| operation.needs_confirmation())
        {
            icons.push((
                operation.icon(),
                operation.label().to_owned(),
                Msg::DeviceAction(device.id.clone(), operation),
            ));
        }
    }
    device_row(
        DeviceRow {
            key: location.id.clone(),
            icon: crate::storage::icon(location.kind),
            art: crate::storage::art(location),
            name: location.name.clone(),
            status: match &space {
                Some((_, free)) => free.clone(),
                None => status,
            },
            warning,
            usage: space.map(|(used, _)| used),
            primary,
            icons,
            menu: None,
            open,
            busy,
        },
        model,
        theme,
    )
}

pub(crate) fn section(location: &Location) -> &'static str {
    match location.kind {
        Kind::Cloud | Kind::Network => crate::i18n::tr("Cloud and network"),
        Kind::Phone => crate::i18n::tr("Phones and cameras"),
        _ => crate::i18n::tr("Drives"),
    }
}
