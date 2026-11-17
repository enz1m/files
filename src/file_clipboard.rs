use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use enzui_clipboard::{FileIntent, FileTransfer};
use iced::Task;
use iced::futures::future::BoxFuture;

type Reply<T> = Option<Result<T, enzui_clipboard::ClipboardError>>;
const WAIT_BUDGET: Duration = Duration::from_secs(3);

pub(crate) trait Provider: Send + Sync {
    fn read(&self) -> BoxFuture<'static, Reply<FileTransfer>>;
    fn offer(&self, transfer: FileTransfer) -> BoxFuture<'static, Reply<()>>;
}

struct Desktop;
impl Provider for Desktop {
    fn read(&self) -> BoxFuture<'static, Reply<FileTransfer>> {
        Box::pin(enzui_clipboard_runtime::read_file_transfer_async(
            WAIT_BUDGET,
        ))
    }
    fn offer(&self, transfer: FileTransfer) -> BoxFuture<'static, Reply<()>> {
        Box::pin(enzui_clipboard_runtime::write_file_transfer_async(
            transfer,
            WAIT_BUDGET,
        ))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Response {
    Offered(Reply<()>),
    Read(Reply<FileTransfer>),
}

#[derive(Debug, Clone)]
pub(crate) struct Receipt {
    pub(crate) generation: u64,
    pub(crate) response: Response,
}

pub(crate) enum Context {
    Offer,
    Paste {
        folder: PathBuf,
        load_generation: u64,
        // Link the clipboard's files here instead of pasting them.
        as_link: bool,
    },
}

struct Pending {
    generation: u64,
    context: Context,
    _abort: iced::task::Handle,
}

pub(crate) struct Runtime {
    provider: Arc<dyn Provider>,
    generation: u64,
    pending: Option<Pending>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            provider: Arc::new(Desktop),
            generation: 0,
            pending: None,
        }
    }
}

impl Runtime {
    pub(crate) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn admit(&mut self) -> Result<u64, &'static str> {
        if self.busy() {
            return Err(crate::i18n::tr(
                "A clipboard request is still pending. Try again when it finishes.",
            ));
        }
        let next = self.generation.checked_add(1).ok_or_else(|| {
            crate::i18n::tr("Clipboard request identities are exhausted. Restart Files.")
        })?;
        self.generation = next;
        Ok(next)
    }

    pub(crate) fn offer(
        &mut self,
        paths: Vec<PathBuf>,
        intent: FileIntent,
    ) -> Result<Task<Receipt>, &'static str> {
        let generation = self.admit()?;
        let transfer = FileTransfer::new(intent, paths);
        let future = self.provider.offer(transfer);
        let (task, abort) = Task::perform(future, move |result| Receipt {
            generation,
            response: Response::Offered(result),
        })
        .abortable();
        self.pending = Some(Pending {
            generation,
            context: Context::Offer,
            _abort: abort.abort_on_drop(),
        });
        Ok(task)
    }

    pub(crate) fn paste(
        &mut self,
        folder: PathBuf,
        load_generation: u64,
        as_link: bool,
    ) -> Result<Task<Receipt>, &'static str> {
        let generation = self.admit()?;
        let (task, abort) = Task::perform(self.provider.read(), move |result| Receipt {
            generation,
            response: Response::Read(result),
        })
        .abortable();
        self.pending = Some(Pending {
            generation,
            context: Context::Paste {
                folder,
                load_generation,
                as_link,
            },
            _abort: abort.abort_on_drop(),
        });
        Ok(task)
    }

    pub(crate) fn settle(&mut self, receipt: Receipt) -> Option<(Context, Response)> {
        let pending = self.pending.as_ref()?;
        if pending.generation != receipt.generation {
            return None;
        }
        if !matches!(
            (&pending.context, &receipt.response),
            (Context::Offer, Response::Offered(_)) | (Context::Paste { .. }, Response::Read(_))
        ) {
            return None;
        }
        let pending = self.pending.take()?;
        Some((pending.context, receipt.response))
    }

    pub(crate) fn cancel(&mut self) {
        self.pending.take();
    }

    #[cfg(test)]
    pub(crate) fn with_provider(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            ..Self::default()
        }
    }
}

pub(crate) fn failure(
    result: Option<&Result<impl Sized, enzui_clipboard::ClipboardError>>,
) -> &'static str {
    use enzui_clipboard::ClipboardError;
    match result {
        None => crate::i18n::tr(
            "The desktop clipboard is unavailable or the request timed out. Its contents may have changed.",
        ),
        Some(Err(ClipboardError::NoSelection | ClipboardError::MimeUnavailable { .. })) => {
            crate::i18n::tr("The clipboard contains no supported local files.")
        }
        Some(Err(ClipboardError::NotFocused)) => {
            crate::i18n::tr("Focus this Files window before using the clipboard.")
        }
        Some(Err(ClipboardError::InvalidFileSelection)) => crate::i18n::tr(
            "The selected paths cannot all be represented safely on the desktop clipboard. Nothing was offered.",
        ),
        Some(Err(ClipboardError::Busy)) => {
            crate::i18n::tr("The desktop clipboard is busy. Try again.")
        }
        Some(Err(_)) => {
            crate::i18n::tr("The desktop clipboard request failed. No file operation was started.")
        }
        Some(Ok(_)) => crate::i18n::tr("Clipboard request completed."),
    }
}

impl crate::App {
    pub(super) fn begin_file_copy(&mut self, copy: bool) -> Task<crate::Msg> {
        if self.close_pending.is_some() || self.modal_owns_keyboard() {
            return Task::none();
        }
        let paths = self.model.clipboard_sources();
        if paths.is_empty() {
            return Task::none();
        }
        let intent = if copy {
            FileIntent::Copy
        } else {
            FileIntent::Cut
        };
        match self.file_clipboard.offer(paths, intent) {
            Ok(task) => {
                self.model.clipboard_pending = true;
                task.map(crate::Msg::FileClipboardFinished)
            }
            Err(error) => {
                self.post_status(error, crate::ToastTone::Warning);
                Task::none()
            }
        }
    }

    pub(super) fn begin_file_paste(&mut self, as_link: bool) -> Task<crate::Msg> {
        if !self.model.can_paste()
            || self.close_pending.is_some()
            || self.modal_owns_keyboard()
            || self.native_drop.busy()
        {
            return Task::none();
        }
        match self
            .file_clipboard
            .paste(self.model.cwd.clone(), self.model.load_generation, as_link)
        {
            Ok(task) => {
                self.model.clipboard_pending = true;
                task.map(crate::Msg::FileClipboardFinished)
            }
            Err(error) => {
                self.post_status(error, crate::ToastTone::Warning);
                Task::none()
            }
        }
    }

    pub(super) fn file_clipboard_finished(&mut self, receipt: Receipt) -> Task<crate::Msg> {
        let Some((context, response)) = self.file_clipboard.settle(receipt) else {
            return Task::none();
        };
        self.model.clipboard_pending = false;
        if self.close_pending.is_some() {
            return Task::none();
        }
        match (context, response) {
            (Context::Offer, Response::Offered(result)) => {
                if !matches!(result, Some(Ok(()))) {
                    self.post_status(failure(result.as_ref()), crate::ToastTone::Warning);
                }
            }
            (
                Context::Paste {
                    folder,
                    load_generation,
                    as_link,
                },
                Response::Read(result),
            ) => {
                if folder != self.model.cwd
                    || load_generation != self.model.load_generation
                    || self.model.screen != crate::Screen::Directory
                    || self.modal_owns_keyboard()
                {
                    self.post_status(crate::i18n::tr("The destination changed before paste completed. Paste again in the intended folder."), crate::ToastTone::Warning);
                } else if let Some(Ok(transfer)) = result {
                    if as_link {
                        return self.update(crate::Msg::LinkSources {
                            sources: transfer.into_paths(),
                            dest_dir: folder,
                        });
                    }
                    let copy = transfer.intent() == FileIntent::Copy;
                    return self.update(crate::Msg::BeginPaste {
                        sources: transfer.into_paths(),
                        dest_dir: folder,
                        copy,
                    });
                } else {
                    self.post_status(failure(result.as_ref()), crate::ToastTone::Warning);
                }
            }
            _ => unreachable!("receipt kind checked by settlement"),
        }
        Task::none()
    }
}
