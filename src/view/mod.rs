// The view layer. Reads only the `Model`; never touches the filesystem.

pub(crate) mod activity_status;
pub(crate) mod computer;
pub(crate) mod downloads;
pub(crate) mod grid;
pub(crate) mod journal_prompt;
pub(crate) mod list;
pub(crate) mod network;
pub(crate) mod preview;
pub(crate) mod project_strip;
pub(crate) mod shell;
pub(crate) mod sidebar;
pub(crate) mod storage;
pub(crate) mod widgets;

use crate::model::Model;

pub(crate) const EMPTY_FOLDER_TITLE: &str = "This folder is empty";

pub(crate) const EMPTY_FOLDER_DESCRIPTION: &str = "Drop files or create a new one to get started.";

pub(crate) const NO_MATCHES_TITLE: &str = "No matches";
pub(crate) const NO_MATCHES_DESCRIPTION: &str = "Try a different search.";

pub(crate) fn empty_directory_page(model: &Model) -> iced::Element<'static, crate::message::Msg> {
    use crate::{i18n::tr, message::Msg};
    use enzui_core::Action;
    use enzui_widgets::action_emphasis::ActionEmphasis;
    use enzui_widgets::status_page::enz_status_page;

    if model.search_active && !model.search_query.is_empty() {
        return enz_status_page(tr(NO_MATCHES_TITLE))
            .description(tr(NO_MATCHES_DESCRIPTION))
            .icon("search-x")
            .into_element();
    }

    let mut page = enz_status_page(tr(EMPTY_FOLDER_TITLE))
        .description(tr(EMPTY_FOLDER_DESCRIPTION))
        .icon("folder-open");
    if !crate::services::is_virtual(&model.cwd) {
        page = page
            .primary_action(Action::new("new-folder", Msg::NewFolder).with_label(tr("New Folder")))
            .primary_action_emphasis(ActionEmphasis::Neutral)
            .secondary_action(Action::new("new-file", Msg::NewFile).with_label(tr("New File")));
    }
    page.into_element()
}

pub(crate) const DETAILS_EMPTY_HINT: &str = "Select a file to view details.";

// Thumbnail scheduling and shortcut dispatch share the shell's geometry resolver.
pub(crate) fn shell_geometry(
    model: &Model,
    theme: &enzui_theme::EnzTheme,
    window_width: f32,
) -> enzui_widgets::app_shell::ShellGeometry {
    enzui_widgets::app_shell::enz_app_shell::<crate::message::Msg>(|| {
        iced::widget::Space::new().into()
    })
    .layout(enzui_widgets::app_shell::ShellLayout::TitlebarSidebarContent)
    .tokens(shell::shell_tokens_for())
    .breakpoints(theme.breakpoints)
    .state(model.nav)
    .geometry_for(window_width.max(0.0))
}

mod native_transfer;
