// The Computer view: compact drive rows with thin capacity bars.
//
// One row per drive, no cards: the name aligned with its label, a thin usage
// bar, and the free/total text, without needless truncation.

use enzui_theme::EnzTheme;
use enzui_widgets::page_header::enz_page_header;
use enzui_widgets::progress_bar::{ProgressBarTokens, ProgressTone, enz_progress_bar};
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::section_header::enz_section_header;
use enzui_widgets::status_page::enz_status_page;
use iced::widget::{column, container};
use iced::{Element, Length};

use enzui_core::Action;

use crate::message::Msg;
use crate::model::{Drive, DriveKind, Model};
use crate::services;

// The label of the Computer screen's "attach a network share" action, the
// same as the Network page's.
pub(crate) const ATTACH_LABEL: &str = crate::view::network::CONNECT_LABEL;

// The `Msg::ShowAttach` action, offered on the page header with drives and on
// the empty state without, so a share can always be mounted.
fn attach_action() -> Action<Msg> {
    Action::new("attach", Msg::ShowAttach).with_label(crate::i18n::tr(ATTACH_LABEL))
}

// The computer's own mounted filesystems; other drives are "Drives".
pub(crate) const DRIVES_SECTION_HEADER: &str = "This computer";

pub(crate) fn view<'a>(model: &Model, theme: &EnzTheme) -> Element<'a, Msg> {
    // An unmounted stick still shows, because that is how it gets mounted.
    if model.drives.is_empty()
        && model.removable_devices.is_empty()
        && model.phones.is_empty()
        && model.storage_locations.is_empty()
    {
        let mut page = enz_status_page(crate::i18n::tr("No drives found"))
            .description(crate::i18n::tr(
                "Nothing is mounted that this app can enumerate.",
            ))
            .icon("hard-drive");
        if model.features.enabled(crate::features::FeatureId::Network) {
            page = page.primary_action(attach_action());
        }
        return container(page.into_element())
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    }

    // Sections as on the Network page: a header over its rows, apart from
    // the next section.
    let section = |title: String, rows: Vec<Element<'a, Msg>>| -> Element<'a, Msg> {
        let mut column = column![enz_section_header::<Msg>(title).into_element()]
            .spacing(theme.spacing.xs())
            .width(Length::Fill);
        for row in rows {
            column = column.push(row);
        }
        column.into()
    };
    let mut sections: Vec<Element<'a, Msg>> = Vec::new();
    let drives: Vec<Element<'a, Msg>> = model
        .drives
        .iter()
        .filter(|drive| {
            !(model.storage_ready
                && matches!(drive.kind, DriveKind::Removable | DriveKind::Network))
                && !model.storage_locations.iter().any(|location| {
                    location.mount.as_ref() == Some(&drive.mount)
                        || location.device.as_ref().is_some_and(|device| {
                            device.as_os_str() == std::ffi::OsStr::new(&drive.source)
                        })
                })
        })
        .map(|drive| drive_row(drive, model, theme))
        .collect();
    if !drives.is_empty() {
        sections.push(section(
            crate::i18n::tr(DRIVES_SECTION_HEADER).to_owned(),
            drives,
        ));
    }
    for title in [
        crate::i18n::tr("Drives"),
        crate::i18n::tr("Cloud and network"),
        crate::i18n::tr("Phones and cameras"),
    ] {
        let rows: Vec<Element<'a, Msg>> = model
            .storage_locations
            .iter()
            .filter(|location| super::storage::section(location) == title)
            .map(|location| super::storage::location_row(location, model, theme))
            .collect();
        if !rows.is_empty() {
            sections.push(section(title.to_owned(), rows));
        }
    }
    // Removable media, mounted or not, with their device operations.
    if !model.removable_devices.is_empty()
        && (!model.storage_ready || model.storage_error.is_some())
    {
        let rows: Vec<Element<'a, Msg>> = model
            .removable_devices
            .iter()
            .filter(|device| {
                !model
                    .storage_locations
                    .iter()
                    .any(|location| location.device.as_ref() == Some(&device.device))
            })
            .map(|device| removable_row(device, model, theme))
            .collect();
        if !rows.is_empty() {
            sections.push(section(
                crate::i18n::tr(REMOVABLE_SECTION_HEADER).to_owned(),
                rows,
            ));
        }
    }
    // Phones and cameras sharing files over MTP, opened on request.
    let phones: Vec<Element<'a, Msg>> = model
        .phones
        .iter()
        .filter(|phone| {
            !model
                .storage_locations
                .iter()
                .any(|location| location.id == phone.id)
        })
        .map(|phone| phone_row(phone, model, theme))
        .collect();
    if !phones.is_empty() {
        sections.push(section(
            crate::i18n::tr(PHONES_SECTION_HEADER).to_owned(),
            phones,
        ));
    }
    let rows = iced::widget::Column::with_children(sections)
        .spacing(theme.spacing.lg())
        .width(Length::Fill);

    // `enz_page_header` owns the title ramp and reflows against its own width.
    let mut header_builder = enz_page_header(crate::i18n::tr("Computer"))
        .spacing_scale(theme.spacing)
        .primary_action_kind(enzui_widgets::button::ButtonKind::Quiet);
    if model.features.enabled(crate::features::FeatureId::Network) {
        header_builder = header_builder.primary_action(attach_action());
        if let Some(action) = super::network::add_cloud_action(model) {
            header_builder = header_builder.secondary_action(action);
        }
    }
    let header: Element<'a, Msg> = header_builder.into();

    // No scrollbar gutter: it composes as a maximum with the 20 px inset, so it
    // would change nothing. `space7` (14) and `space9` (20) are not `SpaceToken`
    // rungs, so they use the raw-px setters.
    enz_scroll_column(vec![header, rows.into()])
        .spacing(theme.spacing.space7)
        .inset(theme.spacing.space9)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
}

pub(crate) const REMOVABLE_SECTION_HEADER: &str = "Removable Media";

pub(crate) const PHONES_SECTION_HEADER: &str = "Phones and Cameras";

pub(crate) const OPEN_PHONE_LABEL: &str = "Open";

fn phone_row<'a>(
    phone: &crate::devices::phones::Phone,
    model: &Model,
    theme: &EnzTheme,
) -> Element<'a, Msg> {
    let (status, primary, icons, open) = match &phone.mount {
        Some(at) => (
            crate::i18n::tr("Opened").to_owned(),
            None,
            vec![(
                "folder-open",
                crate::i18n::format("files-open-name", &[("name", phone.name.clone())]),
                Msg::NavigateTo(at.clone()),
            )],
            Some(Msg::NavigateTo(at.clone())),
        ),
        None => (
            crate::i18n::tr("Unlock it and choose File transfer, then open it").to_owned(),
            Some((
                crate::i18n::tr(OPEN_PHONE_LABEL).to_owned(),
                Msg::OpenPhone(phone.id.clone()),
            )),
            Vec::new(),
            Some(Msg::OpenPhone(phone.id.clone())),
        ),
    };
    super::storage::device_row(
        super::storage::DeviceRow {
            key: format!("phone:{}", phone.id),
            icon: "smartphone",
            art: if crate::storage::is_camera_name(&phone.name) {
                &["camera-photo", "phone"]
            } else {
                &["phone"]
            },
            name: phone.name.clone(),
            status,
            warning: false,
            usage: None,
            primary,
            icons,
            menu: None,
            open,
            busy: false,
        },
        model,
        theme,
    )
}

fn removable_row<'a>(
    device: &crate::devices::RemovableDevice,
    model: &Model,
    theme: &EnzTheme,
) -> Element<'a, Msg> {
    let state = match &device.mount {
        Some(at) => crate::i18n::format(
            "files-mounted-at-f717b94b",
            &[
                ("arg0", (services::format_bytes(device.size))),
                ("arg1", (at.display()).to_string()),
            ],
        ),
        None => crate::i18n::format(
            "files-not-mounted-d2757e76",
            &[("arg0", (services::format_bytes(device.size)))],
        ),
    };
    let mut icons = Vec::new();
    if let Some(at) = &device.mount {
        icons.push((
            "folder-open",
            crate::i18n::format("files-open-name", &[("name", device.label.clone())]),
            Msg::NavigateTo(at.clone()),
        ));
    }
    let mut primary = None;
    for op in device.operations() {
        if op == crate::devices::DeviceOp::Mount {
            primary = Some((
                op.label().to_owned(),
                Msg::DeviceAction(device.id.clone(), op),
            ));
        } else {
            icons.push((
                op.icon(),
                op.label().to_owned(),
                Msg::DeviceAction(device.id.clone(), op),
            ));
        }
    }
    super::storage::device_row(
        super::storage::DeviceRow {
            key: format!("device:{}", device.id),
            icon: if device.ejectable && !device.can_power_off {
                "disc"
            } else {
                "usb"
            },
            art: removable_art(&device.drive),
            name: device.label.clone(),
            status: state,
            warning: false,
            usage: None,
            primary,
            icons,
            menu: None,
            open: device.mount.clone().map(Msg::NavigateTo),
            busy: false,
        },
        model,
        theme,
    )
}

// The icon-theme names for a removable drive, by its kernel name.
fn removable_art(drive: &std::path::Path) -> &'static [&'static str] {
    let name = drive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.starts_with("sr") {
        &["drive-optical"]
    } else if name.starts_with("mmcblk") {
        &["media-flash", "drive-removable-media"]
    } else {
        &["drive-removable-media"]
    }
}

// The icon-theme names for a mounted filesystem.
fn drive_art(drive: &Drive) -> &'static [&'static str] {
    match drive.kind {
        DriveKind::System | DriveKind::Local => &["drive-harddisk"],
        DriveKind::Removable => removable_art(std::path::Path::new(&drive.source)),
        DriveKind::Network => &["folder-remote", "network-server"],
    }
}

fn drive_row<'a>(drive: &Drive, model: &Model, theme: &EnzTheme) -> Element<'a, Msg> {
    let icon = match drive.kind {
        DriveKind::System | DriveKind::Local => "hard-drive",
        DriveKind::Removable => "usb",
        DriveKind::Network => "network",
    };
    // The space first, then where it comes from.
    let status = if drive.total > 0 {
        crate::i18n::format(
            "files-state-detail",
            &[
                (
                    "state",
                    crate::i18n::format(
                        "files-free-of-39972326",
                        &[
                            ("arg0", (services::format_bytes(drive.free))),
                            ("arg1", (services::format_bytes(drive.total))),
                        ],
                    ),
                ),
                ("detail", drive.source.clone()),
            ],
        )
    } else {
        drive.source.clone()
    };
    let mut icons = vec![(
        "folder-open",
        crate::i18n::format("files-open-name", &[("name", drive.label.clone())]),
        Msg::NavigateTo(drive.mount.clone()),
    )];
    if drive.is_unmountable() {
        icons.push((
            "arrow-up-from-line",
            crate::i18n::format("files-unmount-name", &[("name", drive.label.clone())]),
            Msg::UnmountDrive(drive.mount.clone()),
        ));
    }
    super::storage::device_row(
        super::storage::DeviceRow {
            key: format!("drive:{}", drive.mount.display()),
            icon,
            art: drive_art(drive),
            name: drive.label.clone(),
            status,
            warning: false,
            usage: (drive.total > 0).then(|| drive.used_fraction()),
            primary: None,
            icons,
            menu: None,
            open: Some(Msg::NavigateTo(drive.mount.clone())),
            busy: false,
        },
        model,
        theme,
    )
}

// Capacity is a static measurement, not an ongoing transfer.
pub(crate) fn capacity_bar<'a>(frac: f32, _palette: &enzui_theme::Palette) -> Element<'a, Msg> {
    enz_progress_bar(frac)
        .animated(false)
        .tone(capacity_tone(frac))
        .bar_height(CAPACITY_BAR_HEIGHT)
        .bar_radius(CAPACITY_BAR_HEIGHT / 2.0)
        .tokens(ProgressBarTokens::from_palette(_palette))
        .into_element()
}

// Height of the capacity bar in px; `SizingTokens` has no progress-bar height.
const CAPACITY_BAR_HEIGHT: f32 = 6.0;

pub(crate) fn capacity_tone(frac: f32) -> ProgressTone {
    // A non-finite fraction is not "nearly full".
    if !frac.is_finite() {
        return ProgressTone::Accent;
    }
    // Only a nearly full disk is worth a colour.
    if frac >= 0.9 {
        ProgressTone::Danger
    } else {
        ProgressTone::Accent
    }
}
