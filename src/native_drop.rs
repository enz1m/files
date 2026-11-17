// Native completion follows generation-valid validation and committed file operations.

use crate::{App, Msg};
use enzui_widgets::file_drop::{DataDrop, DragAction, FileDrop};
use iced::Task;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct NativeDrop {
    next_id: u64,
    receiving: Option<u64>,
    pub(crate) lease: Option<FileDrop>,
    pub(crate) activity: Option<(crate::activity::FileActivity, usize)>,
    content_cancel: Option<enzui_core::cancellation::CancelToken>,
}

#[derive(Clone, Debug)]
pub(crate) struct Delivery {
    id: u64,
    target: (PathBuf, u64),
    sources: Result<Vec<PathBuf>, crate::drag_drop::DropRefusal>,
    lease: Arc<Mutex<Option<FileDrop>>>,
}

impl NativeDrop {
    pub(crate) fn busy(&self) -> bool {
        self.receiving.is_some() || self.lease.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.receiving = None;
        if let Some(cancel) = self.content_cancel.take() {
            cancel.cancel(enzui_core::cancellation::CancelCause::Cancelled);
        }
        self.lease = None;
        self.activity = None;
    }

    pub(crate) fn finish(
        &mut self,
        activity: crate::activity::FileActivity,
        completed: usize,
        success: bool,
    ) {
        if self.activity.is_some_and(|(owner, _)| owner == activity) {
            let (_, expected) = self.activity.take().expect("matched activity");
            if let Some(lease) = self.lease.take()
                && success
                && completed == expected
            {
                lease.finish();
            }
        }
    }
}

impl App {
    pub(crate) fn native_drop_allowed(&self) -> bool {
        self.main_window.is_some()
            && self.close_pending.is_none()
            && !self.modal_owns_keyboard()
            && self.model.screen == crate::model::Screen::Directory
            && self.model.conflicts.is_none()
            && self.model.inline_rename.is_none()
            && !self.native_drop.busy()
            && !self.activity_center.iter().any(|row| {
                !row.phase.is_terminal()
                    && matches!(
                        row.kind,
                        enzui_core::operation_activity::ActivityKind::Copy
                            | enzui_core::operation_activity::ActivityKind::Move
                            | enzui_core::operation_activity::ActivityKind::Rename
                            | enzui_core::operation_activity::ActivityKind::Delete
                            | enzui_core::operation_activity::ActivityKind::Trash
                    )
            })
            && !crate::services::is_virtual(&self.model.cwd)
    }

    fn native_target_is_current(&self, target: &(PathBuf, u64)) -> bool {
        if target.1 != self.model.load_generation || crate::services::is_virtual(&target.0) {
            return false;
        }
        target.0 == self.model.cwd
            || self.model.cwd.ancestors().any(|path| path == target.0)
            || self
                .model
                .entries
                .iter()
                .any(|entry| entry.path == target.0 && crate::drag_drop::accepts_drop(entry.kind))
            || self
                .model
                .places
                .iter()
                .chain(&self.model.bookmarks)
                .any(|place| {
                    place.path == target.0
                        && crate::model::Model::screen_for_place(place)
                            == crate::model::Screen::Directory
                })
    }

    pub(crate) fn native_files_dropped(
        &mut self,
        drop: FileDrop,
        target: (PathBuf, u64),
    ) -> Task<Msg> {
        if !self.native_drop_allowed() || !self.native_target_is_current(&target) {
            return Task::none();
        }
        let Some(id) = self.native_drop.next_id.checked_add(1) else {
            return Task::none();
        };
        self.native_drop.next_id = id;
        self.native_drop.receiving = Some(id);
        Task::perform(
            async move {
                let paths = drop.paths().to_vec();
                let destination = target.0.clone();
                let sources = crate::shell_work::run(move || {
                    crate::drag_drop::validate_native_paths(&paths, &destination).map(|()| paths)
                })
                .await;
                Delivery {
                    id,
                    target,
                    sources,
                    lease: Arc::new(Mutex::new(Some(drop))),
                }
            },
            Msg::NativeDropReceived,
        )
    }

    pub(crate) fn native_drop_received(&mut self, delivery: Delivery) -> Task<Msg> {
        if self.native_drop.receiving != Some(delivery.id) {
            return Task::none();
        }
        self.native_drop.receiving = None;
        let lease = delivery.lease.lock().ok().and_then(|mut slot| slot.take());
        let Some(lease) = lease else {
            return Task::none();
        };
        if !self.native_drop_allowed() || !self.native_target_is_current(&delivery.target) {
            return Task::none();
        }
        let sources = match delivery.sources {
            Ok(sources) => sources,
            Err(reason) => {
                self.post_status(reason.status(), enzui_widgets::toast::ToastTone::Warning);
                return Task::none();
            }
        };
        let copy = lease.action() != Some(DragAction::Move);
        self.native_drop.lease = Some(lease);
        self.update(Msg::BeginPaste {
            sources,
            dest_dir: delivery.target.0,
            copy,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ContentDelivery {
    id: u64,
    result: Result<PathBuf, String>,
    receipt: DataDrop,
}

const CONTENT_FORMATS: &[(&str, &str, &str)] = &[
    ("image/png", "Dropped image", "png"),
    ("image/jpeg", "Dropped image", "jpg"),
    ("image/webp", "Dropped image", "webp"),
    ("image/gif", "Dropped image", "gif"),
    ("image/tiff", "Dropped image", "tiff"),
    ("image/bmp", "Dropped image", "bmp"),
    ("image/heic", "Dropped image", "heic"),
    ("image/heif", "Dropped image", "heif"),
    ("image/avif", "Dropped image", "avif"),
    ("image/svg+xml", "Dropped image", "svg"),
    ("application/pdf", "Dropped document", "pdf"),
    ("text/plain;charset=utf-8", "Dropped text", "txt"),
    ("text/plain", "Dropped text", "txt"),
    ("UTF8_STRING", "Dropped text", "txt"),
    ("TEXT", "Dropped text", "txt"),
    ("STRING", "Dropped text", "txt"),
    ("text/html;charset=utf-8", "Dropped document", "html"),
    ("text/html", "Dropped document", "html"),
    ("text/markdown", "Dropped text", "md"),
    ("text/csv", "Dropped table", "csv"),
    ("text/calendar", "Dropped event", "ics"),
    ("application/json", "Dropped data", "json"),
    ("application/rtf", "Dropped document", "rtf"),
    ("audio/mpeg", "Dropped audio", "mp3"),
    ("audio/ogg", "Dropped audio", "ogg"),
    ("audio/wav", "Dropped audio", "wav"),
    ("video/mp4", "Dropped video", "mp4"),
    ("video/webm", "Dropped video", "webm"),
    ("application/octet-stream", "Dropped data", "bin"),
];

pub(crate) fn content_preferences() -> Vec<enzui_clipboard::MimeType> {
    CONTENT_FORMATS
        .iter()
        .map(|(mime, _, _)| enzui_clipboard::MimeType::new(*mime))
        .collect()
}

fn save_content(
    directory: &std::path::Path,
    mime: &enzui_clipboard::MimeType,
    bytes: &[u8],
    cancel: &enzui_core::cancellation::CancelToken,
) -> Result<PathBuf, String> {
    use enzui_core::user_file::{self, SavePolicy};
    let Some((_, stem, extension)) = CONTENT_FORMATS
        .iter()
        .find(|(name, _, _)| mime.matches(&enzui_clipboard::MimeType::new(*name)))
    else {
        return Err("unsupported dropped content".into());
    };
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("dropped content exceeds 32 MiB".into());
    }
    if *extension == "txt" {
        std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    }
    for suffix in 0..1000 {
        if cancel.is_terminal() {
            return Err("drop cancelled".into());
        }
        let name = if suffix == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem} ({suffix}).{extension}")
        };
        let path = directory.join(name);
        match user_file::save_with(&path, SavePolicy::CreateNew, |output| {
            for chunk in bytes.chunks(64 * 1024) {
                if cancel.is_terminal() {
                    return Err(std::io::Error::other("drop cancelled"));
                }
                output.write_all(chunk)?;
            }
            Ok(())
        }) {
            Ok(()) => return Ok(path),
            Err(error)
                if !error.was_published()
                    && error.io_error().kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("no unused drop filename is available".into())
}

impl App {
    pub(crate) fn native_content_dropped(
        &mut self,
        drop: DataDrop,
        target: (PathBuf, u64),
    ) -> Task<Msg> {
        if !self.native_drop_allowed() || !self.native_target_is_current(&target) {
            return Task::none();
        }
        let Some(id) = self.native_drop.next_id.checked_add(1) else {
            return Task::none();
        };
        self.native_drop.next_id = id;
        self.native_drop.receiving = Some(id);
        let cancel = enzui_core::cancellation::CancelToken::new();
        let worker_cancel = cancel.share();
        self.native_drop.content_cancel = Some(cancel);
        Task::perform(
            async move {
                let receipt = drop.clone();
                let result = crate::shell_work::run(move || {
                    save_content(
                        &target.0,
                        drop.data().mime(),
                        drop.data().bytes(),
                        &worker_cancel,
                    )
                })
                .await;
                ContentDelivery {
                    id,
                    result,
                    receipt,
                }
            },
            Msg::NativeContentSaved,
        )
    }

    pub(crate) fn native_content_saved(&mut self, delivery: ContentDelivery) -> Task<Msg> {
        if self.native_drop.receiving != Some(delivery.id) {
            return Task::none();
        }
        self.native_drop.receiving = None;
        self.native_drop.content_cancel = None;
        match delivery.result {
            Ok(path) => {
                delivery.receipt.finish();
                self.post_status(
                    crate::i18n::format(
                        "files-drop-content-saved",
                        &[(
                            "name",
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        )],
                    ),
                    enzui_widgets::toast::ToastTone::Success,
                );
                self.update(Msg::Refresh)
            }
            Err(error) => {
                self.post_status(
                    crate::i18n::format("files-drop-content-failed", &[("error", error)]),
                    enzui_widgets::toast::ToastTone::Warning,
                );
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_content_publishes_exact_bytes_without_overwriting_or_executing() {
        use enzui_clipboard::MimeType;
        use enzui_core::cancellation::{CancelCause, CancelToken};
        let root = tempfile::tempdir().expect("private drop destination");
        let text = "Dropped text.txt";
        std::fs::write(root.path().join(text), b"existing document").expect("existing document");
        let dropped = save_content(
            root.path(),
            &MimeType::new("text/plain;charset=utf-8"),
            b"Unicode photo \xe2\x98\x80\n",
            &CancelToken::new(),
        )
        .expect("text drop");
        assert_ne!(dropped, root.path().join(text));
        assert_eq!(
            std::fs::read(root.path().join(text)).expect("original"),
            b"existing document"
        );
        assert_eq!(
            std::fs::read(&dropped).expect("saved text"),
            b"Unicode photo \xe2\x98\x80\n"
        );
        let bytes = b"encoded content";
        let image = save_content(
            root.path(),
            &MimeType::new("image/png"),
            bytes,
            &CancelToken::new(),
        )
        .expect("encoded image drop");
        assert_eq!(image.extension().expect("encoding extension"), "png");
        assert_eq!(std::fs::read(image).expect("encoded bytes"), bytes);
        let before = std::fs::read_dir(root.path()).expect("destination").count();
        let cancel = CancelToken::new();
        cancel.cancel(CancelCause::Cancelled);
        assert!(
            save_content(
                root.path(),
                &MimeType::new("text/plain"),
                b"cancelled",
                &cancel
            )
            .is_err()
        );
        assert!(
            save_content(
                root.path(),
                &MimeType::new("text/plain"),
                &[255],
                &CancelToken::new()
            )
            .is_err()
        );
        assert!(
            save_content(
                root.path(),
                &MimeType::new("application/x-private-reorder"),
                bytes,
                &CancelToken::new()
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("unchanged destination")
                .count(),
            before
        );
    }

    #[test]
    fn native_paths_reject_unsafe_batches_without_writing_a_surviving_subset() {
        let root = tempfile::tempdir().expect("isolated native file workflow");
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        std::fs::create_dir(&source).expect("isolated native file workflow");
        std::fs::create_dir(&destination).expect("isolated native file workflow");
        let file = source.join("canary.txt");
        std::fs::write(&file, b"source canary").expect("isolated native file workflow");
        assert_eq!(
            crate::drag_drop::validate_native_paths(std::slice::from_ref(&file), &destination),
            Ok(())
        );
        for paths in [
            vec![],
            vec![file.clone(), source.join("missing")],
            vec![PathBuf::from("relative")],
            vec![destination.clone()],
        ] {
            assert!(crate::drag_drop::validate_native_paths(&paths, &destination).is_err());
            assert_eq!(
                std::fs::read_dir(&destination)
                    .expect("isolated native file workflow")
                    .count(),
                0
            );
            assert_eq!(
                std::fs::read(&file).expect("isolated native file workflow"),
                b"source canary"
            );
        }
        let nested = source.join("child");
        std::fs::create_dir(&nested).expect("isolated native file workflow");
        assert_eq!(
            crate::drag_drop::validate_native_paths(&[source], &nested),
            Err(crate::drag_drop::DropRefusal::IntoDescendant)
        );
    }
}
