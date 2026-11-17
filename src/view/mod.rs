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

// The empty-folder title, shared by the list and grid views; see
// [`EMPTY_FOLDER_DESCRIPTION`].
pub(crate) const EMPTY_FOLDER_TITLE: &str = "This folder is empty";

pub(crate) const EMPTY_FOLDER_DESCRIPTION: &str = "Drop files or create a new one to get started.";

pub(crate) const NO_MATCHES_TITLE: &str = "No matches";
pub(crate) const NO_MATCHES_DESCRIPTION: &str = "Try a different search.";

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
