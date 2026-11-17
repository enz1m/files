// Preview-window roles, content, and remembered geometry.

use std::path::PathBuf;

use enzui_widgets::window_lifecycle::{PlacementKey, PlacementKeyed};

// The placement default for a preview with no stored geometry.
//
// The tracker sanitises it against the real output; a tiling compositor may
// ignore it.
pub(crate) const DEFAULT_PREVIEW_SIZE: (f32, f32) = (720.0, 560.0);

// Identifies the browser or a detached file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WindowRole {
    // The one browser window: the shell, the sidebar, the file list.
    Main,
    // A detached preview of one file.
    Preview(PathBuf),
}

impl PlacementKeyed for WindowRole {
    // The slot this role persists to, stable across launches.
    //
    // Keyed by path, since a `window::Id` changes every launch. `sanitized`,
    // because filenames may contain `=`, the state file's field separator.
    fn placement_key(&self) -> PlacementKey {
        match self {
            WindowRole::Main => PlacementKey::sanitized("main"),
            WindowRole::Preview(path) => {
                PlacementKey::sanitized(format!("preview:{}", path.display()))
            }
        }
    }
}

#[must_use]
pub(crate) fn preview_window_settings(size: (f32, f32)) -> iced::window::Settings {
    iced::window::Settings {
        size: iced::Size::new(size.0, size.1),
        ..crate::desktop_identity()
            .window_settings(enzui_widgets::app_shell::ShellLayout::TitlebarContent)
    }
}

// What a preview window paints, decided once at open by
// `services::read_preview_content`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PreviewContent {
    // An image decoded off the UI thread into a bounded renderer handle.
    Image(iced::widget::image::Handle),
    Pdf {
        page: crate::preview_pdf::Page,
        pending: Option<u32>,
        error: Option<String>,
    },
    Media {
        player: std::sync::Arc<MediaPreview>,
        status: Box<enzui_services::media::VideoStatus>,
        frame: Option<iced::widget::image::Handle>,
    },
    // A text file, holding the first N bytes decoded lossily to UTF-8.
    Text(String),
    // A type for which no preview backend exists.
    Unsupported,
    // A supported preview whose source could not be read; distinct from
    // `Unsupported` because the user action differs.
    Error(String),
}

// One open preview window's state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Preview {
    // The file being previewed; also its placement slot identity.
    pub(crate) path: PathBuf,
    // The display name shown as the window title and heading.
    pub(crate) name: String,
    // Human-readable type label, from the entry's mime.
    pub(crate) type_label: String,
    // Size in bytes, formatted by the caller.
    pub(crate) size_label: String,
    // What this preview actually paints.
    pub(crate) content: PreviewContent,
    // The preview's own width, separate from the main window's.
    pub(crate) width: f32,
}

impl Preview {
    // The window title, leading with the file name so previews are
    // distinguishable in a window switcher.
    #[must_use]
    pub(crate) fn title(&self) -> String {
        crate::i18n::format("files-preview-47b5d363", &[("arg0", (self.name).clone())])
    }

    // This preview's role, for the placement tracker.
    #[must_use]
    pub(crate) fn role(&self) -> WindowRole {
        WindowRole::Preview(self.path.clone())
    }
}

// One native worker per preview, shared only with its in-flight open message.
// Dropping the last owner closes the bounded worker without joining the UI thread.
pub(crate) struct MediaPreview {
    pub(crate) service: enzui_services::media::VideoService,
    pub(crate) audio: bool,
}
impl std::fmt::Debug for MediaPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPreview")
            .field("audio", &self.audio)
            .finish_non_exhaustive()
    }
}
impl PartialEq for MediaPreview {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl MediaPreview {
    pub(crate) fn open(path: &std::path::Path, audio: bool) -> PreviewContent {
        use enzui_services::media::{LoadOptions, MediaSource, VideoService, VideoServiceConfig};
        let mut service = match VideoService::new(VideoServiceConfig {
            position_interval: std::time::Duration::from_millis(50),
            ..Default::default()
        }) {
            Ok(service) => service,
            Err(error) => return PreviewContent::Error(error.to_string()),
        };
        service.set_frame_limit(Some((1280, 720)));
        service.load(
            MediaSource::local(path),
            LoadOptions {
                autoplay: false,
                ..Default::default()
            },
        );
        let status = Box::new(service.status());
        PreviewContent::Media {
            player: std::sync::Arc::new(Self { service, audio }),
            status,
            frame: None,
        }
    }
}
