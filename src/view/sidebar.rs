// The places sidebar: real folders in one section, virtual locations in another.

use enzui_theme::EnzTheme;
use enzui_theme::SpaceToken;
use enzui_widgets::banner::{BannerTone, enz_banner};
use enzui_widgets::button::{ButtonKind, enz_button};
use enzui_widgets::reorder_list::enz_reorder_list;
use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::sidebar_content::enz_sidebar_content;
use enzui_widgets::sidebar_row::{SidebarRowTokens, enz_sidebar_row};
use enzui_widgets::status_page::enz_status_page;
use iced::{Element, Length};

use crate::message::Msg;
use crate::model::{Model, Place, PlaceKind, SIDEBAR_PINS_COLLECTION_ID, Screen};

// The drag-hover invitation on the sidebar's pin target, shown only while an OS
// file drag is over the window.
pub(crate) const DROP_TO_PIN_LABEL: &str = "Drop to pin";

pub(crate) const DROP_TO_PIN_HINT: &str = "Release to add this folder to Places.";

// Title for the refused-pins banner in the Places section.
pub(crate) const PINS_REFUSED_TITLE: &str = "Bookmarks could not be loaded";

// Action label on the refused-pins banner: re-read the file.
pub(crate) const PINS_RETRY_LABEL: &str = "Retry";

// Explicit action label for replacing refused bookmarks with defaults.
pub(crate) const PINS_RETIRE_LABEL: &str = "Replace with defaults";

pub(crate) const PINS_RETIRE_ID: &str = "files-app-pins-retire";

// Title for the durability-debt banner (Rotated custody).
pub(crate) const PINS_DURABILITY_TITLE: &str = "Bookmarks may not be saved";

// Body for the durability-debt banner.
pub(crate) const PINS_DURABILITY_BODY: &str =
    "Changes were written but could not be confirmed durable. Retry to confirm.";

pub(crate) fn view<'a>(model: &'a Model, theme: &'a EnzTheme) -> Element<'a, Msg> {
    let palette = &theme.palette;

    let mut folders = section(crate::i18n::tr("Places"), theme);
    let mut utilities = section(crate::i18n::tr("Locations"), theme);
    for place in &model.places {
        if (place.kind == PlaceKind::Downloads
            && !model
                .features
                .enabled(crate::features::FeatureId::Downloads))
            || (place.kind == PlaceKind::Network
                && !model.features.enabled(crate::features::FeatureId::Network))
        {
            continue;
        }
        let el = place_row(place, model, theme);
        match place.kind {
            // Plugin panels such as the Download Manager are not directories, so
            // they sit with the virtual entries.
            PlaceKind::Recent
            | PlaceKind::Trash
            | PlaceKind::Computer
            | PlaceKind::Downloads
            | PlaceKind::Network => {
                utilities = utilities.push(el);
            }
            _ => folders = folders.push(el),
        }
    }

    // The scrollbar is a hover overlay and place names are short, so no gutter is
    // reserved; use `.scrollbar_gutter(true)` rather than restating its width.
    // The "Drop to pin" target appears only while an OS file drag hovers the
    // window, inside the Places section where the pin will land.
    if let Some(refusal) = &model.pins_refused {
        folders = folders.push(
            enz_banner(crate::i18n::tr(PINS_REFUSED_TITLE))
                .body(refusal.to_string())
                .tone(BannerTone::Warning)
                .action_text(crate::i18n::tr(PINS_RETRY_LABEL))
                .on_action(Msg::RetryPins)
                .dismissible(false)
                .width(Length::Fill)
                .into_element(),
        );
        folders = folders.push(
            enz_button(crate::i18n::tr(PINS_RETIRE_LABEL), Msg::RetirePins)
                .kind(ButtonKind::Quiet)
                .id(iced::advanced::widget::Id::new(PINS_RETIRE_ID))
                .into_element(),
        );
    }
    if model.pins_refused.is_none() && model.pin_durability_debt {
        folders = folders.push(
            enz_banner(crate::i18n::tr(PINS_DURABILITY_TITLE))
                .body(crate::i18n::tr(PINS_DURABILITY_BODY))
                .tone(BannerTone::Warning)
                .action_text(crate::i18n::tr(PINS_RETRY_LABEL))
                .on_action(Msg::RetryPins)
                .dismissible(false)
                .width(Length::Fill)
                .into_element(),
        );
    }
    if model.pin_drop_active() {
        folders = folders.push(
            enz_status_page::<Msg>(crate::i18n::tr(DROP_TO_PIN_LABEL))
                .description(crate::i18n::tr(DROP_TO_PIN_HINT))
                .icon("bookmark-plus")
                .compact(true)
                .into_element(),
        );
    }
    let pin_row_tokens = enzui_widgets::sidebar_row::SidebarRowTokens::from_palette(palette);
    let pinned_rows = model
        .bookmarks
        .iter()
        .map(|bookmark| (bookmark.id.clone(), place_row(bookmark, model, theme)));
    folders = folders.push(
        enz_reorder_list(pinned_rows, Msg::ReorderSidebarPins)
            .collection_id(SIDEBAR_PINS_COLLECTION_ID)
            // A lifted row and the focus ring hug the row's own pill.
            .row_radius(pin_row_tokens.row_radius)
            .row_inset(pin_row_tokens.outer_gutter)
            // Deadband arbitration keeps a click as navigation; a real drag
            // publishes one reorder instead.
            .drag_through_children(true),
    );

    // Folders are pinned from their context menu or by dragging, not a picker.

    // Captions and the section gap separate the groups, as in Finder and
    // GNOME Files; a full-width rule on top of them only added noise.
    let mut sections: Vec<Element<'a, Msg>> = vec![folders.into(), utilities.into()];
    for title in [
        crate::i18n::tr("Drives"),
        crate::i18n::tr("Cloud and network"),
        crate::i18n::tr("Phones and cameras"),
    ] {
        let locations: Vec<_> = model
            .storage_locations
            .iter()
            .filter(|location| super::storage::section(location) == title)
            .collect();
        if locations.is_empty() {
            continue;
        }
        let mut entries = section(title, theme);
        for location in locations {
            entries = entries.push(super::storage::sidebar_row(location, model, theme));
        }
        sections.push(entries.into());
    }
    let fallback: Vec<_> = model
        .removable_devices
        .iter()
        .filter(|device| {
            !model
                .storage_locations
                .iter()
                .any(|location| location.device.as_ref() == Some(&device.device))
        })
        .collect();
    if !fallback.is_empty() && (!model.storage_ready || model.storage_error.is_some()) {
        let mut entries = section(crate::i18n::tr("Removable media"), theme);
        for device in fallback {
            let message = device.mount.as_ref().map_or_else(
                || Msg::DeviceAction(device.id.clone(), crate::devices::DeviceOp::Mount),
                |root| Msg::NavigateTo(root.clone()),
            );
            entries = entries.push(
                enz_sidebar_row(device.label.clone(), message)
                    .id_prefix(format!("files-storage-fallback-{}", device.id))
                    .icon(if device.ejectable && !device.can_power_off {
                        "disc"
                    } else {
                        "usb"
                    })
                    .selected(
                        device
                            .mount
                            .as_ref()
                            .is_some_and(|root| model.cwd.starts_with(root)),
                    )
                    .tokens(SidebarRowTokens::from_palette(palette))
                    .sizing(theme.sizing)
                    .into_element(),
            );
        }
        sections.push(entries.into());
    }
    enz_scroll_column(sections)
        .gap(SpaceToken::Sm)
        .height(Length::Fill)
        .into()
}

fn section<'a>(
    label: &str,
    theme: &EnzTheme,
) -> enzui_widgets::sidebar_content::EnzSidebarContent<'a, Msg> {
    enz_sidebar_content()
        .spacing_scale(theme.spacing)
        .spacing(SpaceToken::Xxs)
        .height(Length::Shrink)
        .push_section(label)
}

// One places row, built on `enz_sidebar_row`.
fn place_row<'a>(place: &Place, model: &Model, theme: &EnzTheme) -> Element<'a, Msg> {
    let selected = is_place_selected(place, model);

    let msg = match Model::screen_for_place(place) {
        Screen::Directory => Msg::NavigateTo(place.path.clone()),
        screen => Msg::NavigateScreen(screen, place.path.clone()),
    };

    let mut row = enz_sidebar_row(place.label.clone(), msg)
        .icon(place.icon.clone())
        .selected(selected)
        .show_highlight(true)
        // Snap, so the previously selected row does not linger in a fade.
        .reduced_motion(true)
        .tokens(SidebarRowTokens::from_palette(&theme.palette))
        .sizing(theme.sizing);

    // Only user-pinned rows have a context action; the row owns the secondary-press
    // capture so its hit region hugs the row.
    if model.is_pinned(&place.path) {
        let path = place.path.clone();
        row = row
            .on_context(move |at| Msg::OpenSidebarContextMenu(path.clone(), at))
            .context_label(crate::i18n::format(
                "files-actions-a9474105",
                &[("arg0", (place.label).clone())],
            ));
    }

    // `services::default_places` labels are distinct across both sections, so the
    // stock `id_prefix` is unambiguous.
    let content = row.into_element();
    if Model::screen_for_place(place) == Screen::Directory {
        super::native_transfer::target(model, place.path.clone(), content)
    } else {
        content
    }
}

fn is_place_selected(place: &Place, model: &Model) -> bool {
    match Model::screen_for_place(place) {
        Screen::Directory => model.screen == Screen::Directory && model.cwd == place.path,
        screen => model.screen == screen,
    }
}
