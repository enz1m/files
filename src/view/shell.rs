use enzui_core::action_overflow::{Descriptor, Presentation};

use enzui_core::operation_activity::ActivityCenter;
use enzui_core::{Action, Icon};
use enzui_theme::EnzTheme;
use enzui_widgets::action_bar::ActionItem;
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, enz_app_shell_with_geometry};

use enzui_widgets::breadcrumb::{
    BreadcrumbEditEvent, BreadcrumbTokens, EDIT_FIELD_SIZE, enz_breadcrumb,
};
use enzui_widgets::drag_drop_host::{DragHostChannel, enz_drag_drop_host};
use enzui_widgets::drag_item_surface::{DragSurfaceItem, enz_drag_item_surface};
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::icon::enz_icon;
use enzui_widgets::image_widget::{EnzImageFit, enz_image_handle};
use enzui_widgets::labeled_content::enz_labeled_content;
use enzui_widgets::list_row_surface::{TrailingVisibility, enz_list_row_surface};
use enzui_widgets::page_header::enz_page_header;

use enzui_widgets::scroll_column::enz_scroll_column;
use enzui_widgets::search_field::{SearchPresentation, enz_search_field};
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::status_page::{StatusPageTone, enz_status_page};
use enzui_widgets::title_bar::enz_title_bar;
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use enzui_widgets::zoom_wheel::enz_zoom_wheel;
use iced::widget::{container, row};
use iced::{Alignment, Element, Length, Padding};

use super::widgets::{
    PATH_BAR_ID, SIDEBAR_WIDTH, copy_path_icon_id, search_field_id, view_settings_invoker_id,
};
use super::{computer, downloads, grid, list, network, sidebar};
use crate::message::Msg;
use crate::model::{Model, Screen, ViewMode};

// The parent-folder control's accessible name; the control runs `Msg::GoUp`.
pub(crate) const PARENT_FOLDER_LABEL: &str = "Go to parent folder";
/// The location bar keeps room for the current folder name and Copy path.
const LOCATION_MIN_WIDTH: f32 = 180.0;

/// The fluid floor covers the whole leading run: Back, Forward and Up share the
/// content row with the location bar, so they count against the same minimum.
fn location_run_min_width(theme: &EnzTheme) -> f32 {
    let button =
        enzui_widgets::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.effective_control_size();
    3.0 * (button + theme.spacing.sm()) + LOCATION_MIN_WIDTH
}

pub(crate) const TOGGLE_DETAILS_LABEL: &str = "Toggle Details";

pub(crate) const SEARCH_SUBFOLDERS_PLACEHOLDER: &str = "Search subfolders\u{2026}";

// The stable widget id of the sidebar's activity status row.
//
// Keeps focus across activity rebuilds and lets the all-activity dialog restore
// focus to its opener on close.
pub(crate) const ACTIVITY_OVERFLOW_ID: &str = super::activity_status::ACTIVITY_STATUS_ID;

// The "Select items matching" dialog title; see
// [`crate::model::Model::apply_select_matching`].
pub(crate) const SELECT_MATCHING_TITLE: &str = "Select items matching";

pub(crate) const SELECT_MATCHING_PLACEHOLDER: &str = "Select items matching\u{2026}";

// The helper line under the glob field, stating the wildcards and that matching
// is case-insensitive, as `crate::model::glob_match` implements.
pub(crate) const SELECT_MATCHING_HINT: &str = "Use `*` for any run of characters and `?` for a single character. Matching is case-insensitive.";

pub(crate) fn view<'a>(
    model: &'a Model,
    activity_center: &'a ActivityCenter,
    theme: &'a EnzTheme,
    window_height: f32,
    native_drop_enabled: bool,
) -> Element<'a, Msg> {
    // One drag host for the window: a row dragged from the file view can be
    // dropped on a breadcrumb crumb in the titlebar as well as on a folder.
    let channel = DragHostChannel::new();
    let shell = shell_composition(
        model,
        activity_center,
        theme,
        window_height,
        channel.clone(),
        native_drop_enabled,
    );
    let shell: Element<'a, Msg> = enz_drag_drop_host(&channel, shell)
        .on_item_drop(Msg::FileItemDropped)
        .into();
    enzui_widgets::breadcrumb::editable_focus_scope(
        shell,
        model.path_edit_active,
        &model.breadcrumb_edit_bounds,
        iced::advanced::widget::Id::new(PATH_BAR_ID),
        Msg::DismissPathEdit,
    )
}

fn titlebar<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    window_width: f32,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    enz_title_bar()
        .id("files-app-window")
        .leading(
            enz_toolbar_icon("navigation.back", Msg::GoBack)
                .label(crate::i18n::tr("Back"))
                .id(iced::advanced::widget::Id::new("files-app-navigation-back"))
                .enabled(model.can_go_back())
                .into_element(),
        )
        // Back and Forward travel together at the leading edge, as in every
        // browser and file manager; Forward among the trailing commands put
        // the pair a window-width apart.
        .leading(
            enz_toolbar_icon("navigation.forward", Msg::GoForward)
                .label(crate::i18n::tr("Forward"))
                .id(iced::advanced::widget::Id::new(
                    "files-app-navigation-forward",
                ))
                .enabled(model.can_go_forward())
                .into_element(),
        )
        // Up is the third history-free navigation step, so it sits with Back
        // and Forward rather than among the view commands at the far edge.
        .leading(
            enz_toolbar_icon("arrow-up", Msg::GoUp)
                .label(crate::i18n::tr(PARENT_FOLDER_LABEL))
                .id(iced::advanced::widget::Id::new("files-app-navigation-up"))
                .enabled(model.can_go_up())
                .into_element(),
        )
        // The location bar is the window's main control: it takes every pixel
        // between the navigation buttons and the commands, and commands fold
        // into the overflow before it shrinks below a readable trail.
        .push(centre_region(model, theme, window_width, channel))
        .fluid_content(location_run_min_width(theme))
        .actions(toolbar_actions(model), Msg::ToggleToolbarOverflow)
        .action_dismiss(Msg::CloseToolbarOverflow)
        .overflow_open(model.toolbar_overflow_open)
        .action_menu_id_prefix("files-app-titlebar-actions")
        .action_overflow_trigger_id(iced::advanced::widget::Id::new(
            "files-app-titlebar-overflow",
        ))
        .into_element()
}

fn centre_region<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    window_width: f32,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    let palette = &theme.palette;
    let sp = &theme.spacing;

    if model.search_active {
        let recursive = model.search_scope == crate::model::SearchScope::Everywhere;
        // Search shares the breadcrumb's field size and residual, so switching modes
        // keeps height, radius, inset and width.
        let search = enz_search_field(&model.search_query, Msg::SearchChanged)
            .size(enzui_widgets::breadcrumb::EDIT_FIELD_SIZE)
            // The breadcrumb's corners, not the theme's pill search shape.
            .radius(enzui_widgets::breadcrumb::EDIT_FIELD_SIZE.tokens().radius)
            .presentation(SearchPresentation::Popover)
            .width(Length::Fill)
            .placeholder(if recursive {
                crate::i18n::tr(SEARCH_SUBFOLDERS_PLACEHOLDER)
            } else {
                crate::i18n::tr("Search this folder\u{2026}")
            })
            .on_clear(Msg::ToggleSearch)
            .palette(palette.clone())
            .focus_on_mount(true)
            .id(search_field_id())
            .filters(crate::dialog::search_filters_surface(theme, model))
            .on_toggle_filters(Msg::ToggleFilters)
            .results_open(model.filters_open)
            .on_results_open(Msg::SearchFiltersOpenChanged)
            .into_element();
        return titlebar_mode_band(search, model);
    }
    // The caller owns edit mode because Ctrl+L enters it too; the widget owns the
    // narrow current-folder fallback because only it knows the trail's residual.
    let compact_path = matches!(
        theme.breakpoints.classify(window_width),
        enzui_theme::EnzSizeClass::Compact
    );
    let segments = model.breadcrumb_segments();
    let drop_paths = segments.clone();
    let labels = segments.iter().map(|(label, _)| label.clone()).collect();
    let paths: Vec<_> = segments.into_iter().map(|(_, path)| path).collect();

    let trailing: Element<'a, Msg> = row![
        enz_toolbar_icon("copy", Msg::CopyCwdPath)
            .label(crate::i18n::tr("Copy path"))
            .id(copy_path_icon_id())
            .into_element()
    ]
    .mirrored(crate::i18n::rtl())
    .align_y(Alignment::Center)
    .spacing(sp.xs())
    .into();

    // Three entries keep the compact location trail readable.
    let max_visible = if window_width < theme.breakpoints.xl() {
        3
    } else {
        5
    };

    use enzui_widgets::autocomplete_field::{AutocompleteOption, enz_autocomplete_field};
    let options = model
        .path_suggestions
        .iter()
        .map(|path| {
            let label = path.to_string_lossy().into_owned();
            AutocompleteOption::new(label.clone(), label, path.clone())
        })
        .collect();
    let editor = enz_autocomplete_field(
        "",
        &model.path_edit_draft,
        options,
        Msg::PathDraftChanged,
        |path| Msg::PathSuggestionAccepted(path.clone()),
    )
    .id_prefix("files-location")
    .placeholder(crate::i18n::tr("Folder path"))
    .palette(palette.clone())
    .on_custom_value(|value| {
        Msg::PathSuggestionAccepted(crate::services::expand_user_path(&value, &model.home))
    });

    let breadcrumb = enz_breadcrumb(labels, move |index| {
        paths.get(index).cloned().map_or(Msg::Noop, Msg::NavigateTo)
    })
    .id(PATH_BAR_ID)
    .max_visible(max_visible)
    // Below this the widget drops ancestors and the ellipsis entirely, so the
    // current folder is readable rather than truncated behind `Home › … ›`.
    .min_trail_width(MIN_TRAIL_CRUMB_WIDTH + 2.0 * EDIT_FIELD_SIZE.tokens().padding_x)
    .tokens(BreadcrumbTokens::from_palette(palette))
    .editable(
        model.path_edit_active,
        model.path_edit_draft.clone(),
        |event| match event {
            BreadcrumbEditEvent::Begin => Msg::BeginPathEdit,
            BreadcrumbEditEvent::Changed(value) => Msg::PathDraftChanged(value),
            BreadcrumbEditEvent::Commit => Msg::CommitPathEdit,
            BreadcrumbEditEvent::Cancel => Msg::CancelPathEdit,
        },
    )
    .autocomplete_editor(editor);
    // Every ancestor crumb is a drop target, as in Nautilus and Finder: a row
    // dropped on one moves (or with Ctrl copies) into that folder, through the
    // same validated route as a drop on a folder row. The current folder is
    // the list itself, and a collapsed crumb is not there to drop on.
    let native_paths = drop_paths
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    let breadcrumb =
        breadcrumb.decorate_crumb(move |index, content| match native_paths.get(index) {
            Some(path) => super::native_transfer::target(model, path.clone(), content),
            None => content,
        });
    let crumb_ids = breadcrumb.crumb_ids();
    let targets = crumb_ids
        .into_iter()
        .zip(&drop_paths)
        .take(drop_paths.len().saturating_sub(1))
        .map(|(id, (label, path))| {
            (
                id,
                DragSurfaceItem::new(
                    crate::drag_drop::path_to_id(path),
                    iced::Rectangle::default(),
                    label.clone(),
                )
                .drag_source(false)
                .drop_target(!crate::services::is_virtual(path)),
            )
        })
        .collect();
    let breadcrumb: Element<'a, Msg> = if compact_path {
        breadcrumb.into()
    } else {
        breadcrumb.trailing(trailing).into()
    };
    let breadcrumb: Element<'a, Msg> = enz_drag_item_surface(
        channel,
        breadcrumb,
        crate::drag_drop::BREADCRUMB_COLLECTION,
        Vec::new(),
    )
    .items_at_widgets(targets)
    .accepts_background(false)
    .source_capabilities(crate::drag_drop::source_capabilities())
    .offering(crate::drag_drop::files_payload())
    .accepting(crate::drag_drop::files_payload())
    .destination_operations(vec![
        enzui_core::drag::DragOperation::Move,
        enzui_core::drag::DragOperation::Copy,
    ])
    .into();
    titlebar_mode_band(breadcrumb, model)
}

#[path = "shell_parts/toolbar.rs"]
mod toolbar;
use toolbar::MIN_TRAIL_CRUMB_WIDTH;
pub(crate) use toolbar::shell_tokens_for;
use toolbar::{titlebar_mode_band, toolbar_actions};

pub(crate) const SIDEBAR_HEADER_TITLE: &str = "Files";

fn sidebar_titlebar<'a>() -> Element<'a, Msg> {
    enz_title_bar()
        .title(crate::i18n::tr(SIDEBAR_HEADER_TITLE))
        .id("files-app-sidebar-header")
        .into_element()
}

fn shell_composition<'a>(
    model: &'a Model,
    activity_center: &'a ActivityCenter,
    theme: &'a EnzTheme,
    window_height: f32,
    channel: DragHostChannel,
    native_drop_enabled: bool,
) -> Element<'a, Msg> {
    let content_channel = channel.clone();
    let tokens = shell_tokens_for();
    let shell = enz_app_shell_with_geometry(move |geometry| {
        main_area(
            model,
            theme,
            geometry.content_width,
            window_height,
            &content_channel,
            native_drop_enabled,
        )
    })
    .layout(ShellLayout::TitlebarSidebarContent)
    .tokens(tokens)
    .breakpoints(theme.breakpoints)
    .state(model.nav)
    .on_window_gesture(Msg::Window)
    .titlebar_with_geometry(move |geometry| {
        titlebar(model, theme, geometry.content_width, &channel)
    });

    shell
        .sidebar_titlebar(sidebar_titlebar)
        .sidebar_label(crate::i18n::tr(SIDEBAR_HEADER_TITLE))
        .sidebar(move || sidebar::view(model, theme))
        .on_sidebar_state(Msg::SidebarState)
        // One compact status row; per-operation controls live on the
        // Downloads page and in the all-activity dialog it opens.
        .activity(move |geometry| {
            (!activity_center.is_empty()).then(|| {
                super::activity_status::view(
                    activity_center,
                    theme,
                    geometry.sidebar,
                    model.nav.effective_overlay_open(geometry.sidebar),
                )
            })
        })
        .into_element()
}

// The shell owns the content background and outside clipping.
fn main_area<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    content_width: f32,
    window_height: f32,
    channel: &DragHostChannel,
    native_drop_enabled: bool,
) -> Element<'a, Msg> {
    let details_width = details_pane_width(content_width);
    // Shared with the thumbnail scheduler so the requested tile band matches the
    // one the virtual grid realises.
    let primary_width = directory_view_width(model, content_width);
    let body: Element<'a, Msg> = match model.screen {
        Screen::Directory => super::native_transfer::target_if(
            model,
            model.cwd.clone(),
            directory_body(model, theme, primary_width, window_height, channel),
            native_drop_enabled,
        ),
        Screen::Computer => computer::view(model, theme),
        // Not a sidebar destination; reached only by render gates sweeping `Screen`.
        Screen::Recent => virtual_placeholder(crate::i18n::tr("Recent"), "clock", theme),
        Screen::Trash => trash_body(model, theme, primary_width),
        Screen::Downloads => downloads::view(model, theme, primary_width, window_height),
        Screen::Network => network::view(model, theme),
    };
    let inner: Element<'a, Msg> = if model.tabs().len() > 1 {
        let strip = container(tab_strip(model))
            .height(Length::Fixed(
                enzui_widgets::title_bar::DEFAULT_TITLEBAR_HEIGHT,
            ))
            .width(Length::Fill)
            .padding(
                Padding::ZERO
                    .left(theme.spacing.sm())
                    .right(theme.spacing.sm()),
            );
        iced::widget::column![strip, body].into()
    } else {
        body
    };
    let rtl = crate::i18n::rtl();
    let details_panel = details::details_panel(model, theme, details_width, rtl);
    let body: Element<'a, Msg> = row![inner, details_panel]
        .mirrored(rtl)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// Minimum width of the trailing details pane at laptop/tablet rungs.
const DETAILS_PANE_WIDTH: f32 = 240.0;

const DETAILS_PANE_DESKTOP_WIDTH: f32 = 360.0;

// Content width at which the details pane takes its desktop width.
const DETAILS_DESKTOP_THRESHOLD: f32 = 1_024.0;

// Width offered to the virtual directory view, shared with the thumbnail
// scheduler.
pub(crate) fn directory_view_width(model: &Model, content_width: f32) -> f32 {
    if model.details_open {
        (content_width - details_pane_width(content_width)).max(120.0)
    } else {
        content_width
    }
}

pub(crate) fn details_pane_width(content_width: f32) -> f32 {
    if content_width >= DETAILS_DESKTOP_THRESHOLD {
        DETAILS_PANE_DESKTOP_WIDTH
    } else {
        DETAILS_PANE_WIDTH
    }
}

#[path = "shell_parts/details.rs"]
mod details;

fn directory_body<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    content_width: f32,
    window_height: f32,
    channel: &DragHostChannel,
) -> Element<'a, Msg> {
    if let Some(err) = &model.load_error {
        // The framework error state, so a failure does not read as content.
        return container(
            enz_status_page(crate::i18n::tr("Could not open this folder"))
                .description(err.clone())
                .icon("folder-x")
                .tone(StatusPageTone::Error)
                .into_element(),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(theme.spacing.xl())
        .into();
    }

    if model.listing_pending && model.entries.is_empty() {
        // Neither the old folder's files nor "empty" while the listing is out.
        let pane = container(if model.listing_slow {
            enz_status_page(crate::i18n::tr("Loading…"))
                .icon("loader")
                .into_element()
        } else {
            iced::widget::Space::new().into()
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .padding(theme.spacing.xl());
        return pane.into();
    }

    let pending = pending_copy_rows(model, theme);
    let body = if pending.is_some() && model.visible_provider.rows.is_empty() {
        iced::widget::Space::new().height(Length::Fill).into()
    } else {
        match model.view_mode {
            ViewMode::List => list::view_hosted_by(
                model,
                theme,
                content_width,
                window_height.max(480.0),
                channel,
            ),
            ViewMode::Grid => grid::view_hosted_by(model, theme, window_height.max(480.0), channel),
        }
    };
    // Ctrl+wheel over the file view zooms it instead of scrolling it.
    let body = enz_zoom_wheel(body, Msg::Zoom).into_element();
    // The folder's repository, in a status strip below the list. A column
    // whose optional child comes after the list, so the list keeps its place
    // in the tree when the strip appears.
    let strip = super::project_strip::strip(model, theme, content_width);
    iced::widget::Column::with_children(
        [
            pending.unwrap_or_else(|| iced::widget::Space::new().into()),
            body,
        ]
        .into_iter()
        .chain(strip),
    )
    .into()
}

fn pending_copy_rows<'a>(model: &Model, theme: &EnzTheme) -> Option<Element<'a, Msg>> {
    let mut paths = model
        .pending_copy_folders
        .iter()
        .filter_map(|folder| {
            let name = folder.path.file_name()?.to_string_lossy();
            (folder.path.parent() == Some(model.cwd.as_path())
                && (!model.search_active || model.search_query.is_empty())
                && (model.show_hidden || !name.starts_with('.'))
                && model.entry_for_path(&folder.path).is_none())
            .then(|| name.into_owned())
        })
        .take(3)
        .peekable();
    paths.peek()?;
    let copying = crate::i18n::localizer().text("files-copy-folder-pending");
    let rows = iced::widget::Column::with_children(paths.map(|name| {
        container(
            row![
                enz_icon("folder").size(18.0).into_element(),
                enz_text(name)
                    .single_line_elided()
                    .width(Length::Fill)
                    .into_element(),
                enz_text(copying.clone())
                    .size(TextSize::Footnote)
                    .tone(TextTone::Muted)
                    .into_element(),
            ]
            .align_y(Alignment::Center)
            .mirrored(crate::i18n::rtl())
            .spacing(theme.spacing.sm()),
        )
        .padding([theme.spacing.sm(), theme.spacing.md()])
        .width(Length::Fill)
        .into()
    }));
    Some(rows.into())
}

// The id of the tab strip's "all tabs" list trigger.
pub(crate) const TAB_LIST_TRIGGER_ID: &str = "files-app-tab-list";

fn tab_strip<'a>(model: &'a Model) -> Element<'a, Msg> {
    use enzui_widgets::titlebar_tab::{TitlebarTabItem, enz_titlebar_tab_row};
    let items = model
        .tabs()
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let location = model
                .tab_location(index)
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            TitlebarTabItem::new(
                format!("tab-{}", tab.id),
                model.tab_title(index),
                Msg::SwitchTab(tab.id),
            )
            .on_close(Msg::CloseTab(Some(tab.id)))
            .description(location)
            .icon("folder")
        })
        .collect();
    enz_titlebar_tab_row(items, model.tabs().active, Msg::ToggleTabList)
        .list_open(model.tab_list_open)
        .on_dismiss(Msg::ToggleTabList)
        .id_prefix("files-app-tab")
        .trigger_id(iced::advanced::widget::Id::new(TAB_LIST_TRIGGER_ID))
        .trailing(|| {
            enz_toolbar_icon("plus", Msg::NewTab)
                .label(crate::i18n::tr("New tab (Ctrl+T)"))
                .id(iced::advanced::widget::Id::new("files-app-new-tab"))
                .into()
        })
        .into()
}

// The Recent and Trash placeholders, built on [`enz_status_page`].
fn virtual_placeholder<'a>(title: &str, icon: &str, _theme: &EnzTheme) -> Element<'a, Msg> {
    container(
        enz_status_page(title.to_owned())
            .description(crate::i18n::tr("Coming in a later milestone."))
            .icon(icon.to_owned())
            .into_element(),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

pub(crate) const TRASH_TITLE: &str = "Trash";
// The Trash screen's primary action label (and its canonical action id stem).
pub(crate) const EMPTY_TRASH_LABEL: &str = "Empty Trash";

pub(crate) const TRASH_EMPTY_TITLE: &str = "Trash is empty";

pub(crate) const TRASH_EMPTY_DESCRIPTION: &str = "Files you delete are moved here.";

pub(crate) const RESTORE_TRASH_LABEL: &str = "Restore";
#[path = "shell_parts/trash.rs"]
mod trash;
pub(crate) use trash::TRASH_ROW_ID_PREFIX;
use trash::trash_body;
