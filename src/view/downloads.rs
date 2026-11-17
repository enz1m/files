// The Downloads panel: direct HTTP, inspected media-service transfers, and
// optional aria2c torrents.
//
// Every row shares one column grid, so the progress bars and the action
// buttons line up whatever the name, status or phase:
//
//   [file icon] [name / one muted status line] [progress, fixed] [icons, fixed]
//
// Colour carries meaning only: the accent fill for live progress, a muted fill
// for paused work, and the danger tone for a failure's status line. Actions are
// the framework's quiet toolbar icons with tooltips. Rows are realized through
// the framework virtual list, so a long retained history costs nothing
// offscreen.

use std::rc::Rc;

use enzui_core::Action;
use enzui_core::download::{Download, DownloadAction, DownloadId, Phase};
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};
use enzui_theme::EnzTheme;
use enzui_widgets::banner::{BannerTone, enz_banner};
use enzui_widgets::download_list::DownloadRow;
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::page_header::enz_page_header;
use enzui_widgets::progress_bar::enz_progress_bar;
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::spinner::enz_spinner;
use enzui_widgets::status_page::enz_status_page;
use enzui_widgets::toolbar_icon::{TitlebarAffordanceTokens, enz_toolbar_icon};
use enzui_widgets::virtual_list::{ProviderHandle, enz_virtual_list};
use iced::widget::{Space, column, container, row};
use iced::{Alignment, Element, Length, Padding};

use crate::downloads::{ADD_DOWNLOAD_LABEL, ARIA2C_INSTALL_HINT, ARIA2C_MISSING_TITLE, EmptyState};
use crate::message::Msg;
use crate::model::Model;

// The sidebar row, tab, path bar and page share one name, distinct from
// the Downloads folder.
pub(crate) const DOWNLOADS_TITLE: &str = "Download Manager";

// The status-page title shared by both empty states; their descriptions
// differ (see `EmptyState::message`).
pub(crate) const EMPTY_TITLE: &str = "No downloads yet";

// Tooltip and accessible name of a finished download's reveal action.
pub(crate) const SHOW_IN_FOLDER_LABEL: &str = "Show in Folder";

// The stable id of the list's scrollable.
pub(crate) const DOWNLOADS_LIST_ID: &str = "files-app-downloads-list";

// One row's fixed height: two text lines beside a 24 px file icon, with room
// for the toolbar-icon hit targets. Uniform, so the virtual list's pitch is
// exact and every row's columns sit on the same grid.
pub(crate) const ROW_HEIGHT: f32 = 56.0;

// The file-type glyph at the start of a row.
const FILE_ICON_SIZE: f32 = 24.0;

// The progress column's width is a share of the content width, bounded, so a
// narrow window keeps a readable name column.
const PROGRESS_SHARE: f32 = 0.22;
const PROGRESS_MIN: f32 = 96.0;
const PROGRESS_MAX: f32 = 200.0;

// Paused, queued, failed and cancelled bars are not moving; the busy sweep is
// for live work only.
const BAR_HEIGHT: f32 = 6.0;

// Most actions any phase offers at once (pause or resume, then cancel).
const ACTION_SLOTS: f32 = 2.0;

pub(crate) fn progress_column_width(content_width: f32) -> f32 {
    (content_width * PROGRESS_SHARE).clamp(PROGRESS_MIN, PROGRESS_MAX)
}

fn actions_column_width(theme: &EnzTheme) -> f32 {
    let control = TitlebarAffordanceTokens::DEFAULT.effective_control_size();
    control * ACTION_SLOTS + theme.spacing.xs() * (ACTION_SLOTS - 1.0)
}

// The downloads in admission order, keyed by their stable id so rows keep
// their widget state when history is pruned above them.
struct DownloadIds<'a> {
    state: &'a crate::downloads::DownloadsState,
    len: u64,
}

impl ItemProvider<DownloadId> for DownloadIds<'_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn item(&self, index: u64) -> Option<DownloadId> {
        self.state
            .downloads()
            .nth(usize::try_from(index).ok()?)
            .map(|download| download.id)
    }
}

impl KeyedProvider<DownloadId> for DownloadIds<'_> {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.item(index)
            .map(|id| SequenceKey::new(format!("download::{}", id.0)))
    }
}

pub(crate) fn view<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    content_width: f32,
    viewport_height: f32,
) -> Element<'a, Msg> {
    let state = &model.downloads;
    let inset = theme.spacing.space9;

    // HTTP/HTTPS always works through the in-process transport, so Add stays
    // available while the torrent backend is probing or absent. The header is
    // the same `enz_page_header` as Computer and Network, with Add as its
    // page action.
    let mut header = enz_page_header(crate::i18n::tr(DOWNLOADS_TITLE))
        .spacing_scale(theme.spacing)
        .primary_action_kind(enzui_widgets::button::ButtonKind::Quiet);
    if state.can_add() {
        header = header.primary_action(
            Action::new("files-app-add-download", Msg::ShowAddDownload)
                .with_label(crate::i18n::tr(ADD_DOWNLOAD_LABEL)),
        );
    }
    let mut top = column![header.into_element()]
        .spacing(theme.spacing.space7)
        .width(Length::Fill);

    // Scoped to torrent support; HTTP rows continue independently, so this is
    // information, not a warning.
    if state.backend == crate::downloads::Aria2Status::Absent {
        top = top.push(
            enz_banner(crate::i18n::tr(ARIA2C_MISSING_TITLE))
                .body(crate::i18n::tr(ARIA2C_INSTALL_HINT))
                .tone(BannerTone::Info)
                .width(Length::Fill)
                .into_element(),
        );
    }
    let top: Element<'a, Msg> = container(top)
        .padding(Padding {
            top: inset,
            right: inset,
            bottom: theme.spacing.space7,
            left: inset,
        })
        .width(Length::Fill)
        .into();

    // A failed transfer is also reported as a sticky error toast
    // (`App::reconcile_transfer_failure_toast`), not a page banner.

    let body: Element<'a, Msg> = match state.empty_state() {
        Some(empty) => {
            let icon = match empty {
                EmptyState::QueueEmpty => "download",
            };
            container(
                enz_status_page(crate::i18n::tr(EMPTY_TITLE))
                    .description(empty.message())
                    .icon(icon)
                    // The page header already publishes the frame's single
                    // Page heading; the empty state is a region of this page.
                    .announce_heading(false)
                    .into_element(),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(inset)
            .into()
        }
        None => {
            let columns = Columns {
                progress: progress_column_width(content_width - 2.0 * inset),
                actions: actions_column_width(theme),
                inset,
            };
            let provider: Rc<dyn KeyedProvider<DownloadId> + 'a> = Rc::new(DownloadIds {
                state,
                len: state.downloads().count() as u64,
            });
            let list =
                enz_virtual_list(ProviderHandle::Owned(provider), ROW_HEIGHT, move |_, id| {
                    match state.queue.get(*id) {
                        Some(download) => download_row(state, download, theme, columns),
                        None => Space::new().height(Length::Fixed(ROW_HEIGHT)).into(),
                    }
                })
                .scroll_offset(model.downloads_scroll_y)
                .viewport_height(viewport_height)
                .id(iced::widget::Id::new(DOWNLOADS_LIST_ID))
                .on_scroll(|viewport| Msg::DownloadsScrolled(viewport.absolute_offset().y))
                .height(Length::Fill)
                .width(Length::Fill);
            let list: Element<'a, Msg> = list.into_element();
            let rows = state.downloads().count();
            SemanticsContainer::new(
                container(list)
                    .padding(Padding::ZERO.bottom(theme.spacing.md()))
                    .width(Length::Fill)
                    .height(Length::Fill),
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label(crate::i18n::tr(DOWNLOADS_TITLE))
                    .description(enzui_core::localized!("enzui-download-count", "count" => rows)),
            )
            .into()
        }
    };

    column![top, body]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[derive(Debug, Clone, Copy)]
struct Columns {
    progress: f32,
    actions: f32,
    inset: f32,
}

// The one muted line under a row's name: bytes, then the state that matters.
// A failure names its cause and takes the danger tone; nothing else is
// coloured.
pub(crate) fn status_line(
    state: &crate::downloads::DownloadsState,
    download: &Download,
) -> (String, TextTone) {
    let received = crate::services::format_bytes(download.received);
    let bytes = match download.total() {
        Some(total) => enzui_core::localized!(
            "enzui-download-list-of",
            "received" => received,
            "total" => crate::services::format_bytes(total)
        ),
        None => received,
    };
    let pair = |first: String, second: String| {
        crate::i18n::format(
            "files-download-status",
            &[("first", first), ("second", second)],
        )
    };
    let word = download.phase.word();
    let (line, tone) = match download.phase {
        Phase::Active => match download.percent() {
            Some(percent) => (
                pair(
                    bytes,
                    enzui_core::localized!("enzui-download-percent", "percentage" => percent),
                ),
                TextTone::Muted,
            ),
            None => (pair(bytes, word), TextTone::Muted),
        },
        Phase::Queued if download.received == 0 => (word, TextTone::Muted),
        Phase::Queued | Phase::Paused => (pair(bytes, word), TextTone::Muted),
        Phase::Failed => match state.error_of(download.id) {
            Some(error) => (pair(word, error.to_owned()), TextTone::Danger),
            None => (word, TextTone::Danger),
        },
        Phase::Complete => (
            pair(
                crate::services::format_bytes(download.total().unwrap_or(download.received)),
                word,
            ),
            TextTone::Muted,
        ),
        Phase::Cancelled => (word, TextTone::Muted),
    };
    // Live torrent telemetry (peers, rate) rides on the same line.
    match state.torrent_detail(download) {
        Some(detail) => (pair(line, detail.to_owned()), tone),
        None => (line, tone),
    }
}

// The progress column: a bar for work that is not finished, a spinner for live
// work of unknown length, and nothing once the transfer is over.
fn progress_cell<'a>(download: &Download, theme: &EnzTheme) -> Option<Element<'a, Msg>> {
    match download.phase {
        Phase::Complete | Phase::Failed | Phase::Cancelled => None,
        Phase::Active if download.fraction().is_none() => Some(
            enz_spinner()
                .size(theme.sizing.icon_sm)
                .active(true)
                .into_element(),
        ),
        phase => {
            let bar = enz_progress_bar(download.fraction().unwrap_or(0.0))
                .bar_height(BAR_HEIGHT)
                .busy(phase == Phase::Active);
            // Paused and queued work is not moving: a quiet fill, not the accent.
            let bar = if phase == Phase::Active {
                bar
            } else {
                bar.fill_color(theme.palette.text_subtle)
            };
            Some(bar.into_element())
        }
    }
}

fn action_icon(action: DownloadAction) -> &'static str {
    match action {
        DownloadAction::Pause => "pause",
        DownloadAction::Resume => "play",
        DownloadAction::Retry => "rotate-cw",
        DownloadAction::Cancel => "x",
    }
}

fn action_button<'a>(
    id: DownloadId,
    icon: &'static str,
    key: &str,
    label: String,
    message: Msg,
) -> Element<'a, Msg> {
    enz_toolbar_icon(icon, message)
        .id(iced::advanced::widget::Id::from(format!(
            "files-app-download-{}-{key}",
            id.0
        )))
        .aria_label(label.clone())
        .tooltip(label)
        .into_element()
}

fn download_row<'a>(
    state: &'a crate::downloads::DownloadsState,
    download: &Download,
    theme: &'a EnzTheme,
    columns: Columns,
) -> Element<'a, Msg> {
    let id = download.id;
    let name = state.name_of(id);
    let (status, tone) = status_line(state, download);

    let icon: Element<'a, Msg> = crate::icon::row_file_icon(name, false, FILE_ICON_SIZE);
    let text = column![
        enz_text(name.to_owned())
            .size(TextSize::Body)
            .single_line_elided()
            .full_width(),
        enz_text(status)
            .size(TextSize::Caption)
            .tone(tone)
            .single_line_elided()
            .full_width(),
    ]
    .spacing(theme.spacing.space1)
    .width(Length::Fill);

    // A finished row has no bar, so its name and status may run on into the
    // progress column; the bars and actions of other rows keep their places.
    let progress: Option<Element<'a, Msg>> = progress_cell(download, theme).map(|cell| {
        container(cell)
            .width(Length::Fixed(columns.progress))
            .align_y(Alignment::Center)
            .into()
    });

    // Trailing-aligned, so the last control of every row shares one edge.
    let mut actions = row![]
        .spacing(theme.spacing.xs())
        .align_y(Alignment::Center);
    for action in download.actions() {
        let key = match action {
            DownloadAction::Pause => "pause",
            DownloadAction::Resume => "resume",
            DownloadAction::Retry => "retry",
            DownloadAction::Cancel => "cancel",
        };
        actions = actions.push(action_button(
            id,
            action_icon(action),
            key,
            action.label(),
            Msg::DownloadAction(id, action),
        ));
    }
    if download.phase == Phase::Complete
        && let Some(folder) = state.completed_folder(id)
    {
        actions = actions.push(action_button(
            id,
            "folder-open",
            "reveal",
            crate::i18n::tr(SHOW_IN_FOLDER_LABEL).to_owned(),
            Msg::NavigateTo(folder.to_path_buf()),
        ));
    }
    let actions: Element<'a, Msg> = container(actions.mirrored(crate::i18n::rtl()))
        .width(Length::Fixed(columns.actions))
        .align_x(if crate::i18n::rtl() {
            Alignment::Start
        } else {
            Alignment::End
        })
        .into();

    let cells: Vec<Element<'a, Msg>> = [Some(icon), Some(text.into()), progress, Some(actions)]
        .into_iter()
        .flatten()
        .collect();
    let body = iced::widget::Row::with_children(cells)
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.md())
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // The row's node states the name, phase, percentage, bytes and actions;
    // the icon buttons publish their own nodes beneath it.
    let semantics = match state.torrent_detail(download) {
        Some(detail) => DownloadRow::new(name, download).secondary(detail),
        None => DownloadRow::new(name, download),
    }
    .semantics();
    SemanticsContainer::new(
        container(body)
            .height(Length::Fixed(ROW_HEIGHT))
            .padding(Padding::from([0.0, columns.inset]))
            .align_y(Alignment::Center)
            .width(Length::Fill),
        semantics,
    )
    .into()
}
