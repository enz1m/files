// Default-application file opening through XDG Desktop Portal OpenURI.
//
// Primary for sandboxed installs; native hosts may fall back to the bounded GIO
// launcher when this owner reports an unavailable or unsupported outcome.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use enzui_core::portal::completion::{CompletionSignal, PortalWake};
use enzui_core::portal::open_uri::{
    FilePathUriError, OpenUri, OpenUriBackend, OpenUriRequest, PendingOpenUri,
};
use enzui_core::portal::parent::PortalParent;
use enzui_core::portal::{CancelReason, PortalOutcome};

// One terminal default-application request with the chosen path; the generation
// keeps concurrent opens independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCompletion {
    pub(crate) generation: u64,
    pub(crate) path: PathBuf,
    pub(crate) outcome: PortalOutcome<()>,
}

// A failure that occurs before an OpenURI request can be retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartError {
    Path(FilePathUriError),
    Portal(PortalOutcome<()>),
}

struct PendingFile {
    path: PathBuf,
    request: PendingOpenUri,
}

// App-side owner for concurrent portal OpenURI requests.
pub(crate) struct PortalFileOpener {
    backend: Option<Box<dyn OpenUriBackend>>,
    pending: BTreeMap<u64, PendingFile>,
    generation: u64,
}

impl std::fmt::Debug for PortalFileOpener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PortalFileOpener")
            .field(
                "backend",
                &self
                    .backend
                    .as_ref()
                    .map(|backend| backend.describe_open_uri()),
            )
            .field("pending", &self.pending.len())
            .field("generation", &self.generation)
            .finish()
    }
}

impl Default for PortalFileOpener {
    fn default() -> Self {
        Self::new()
    }
}

impl PortalFileOpener {
    pub(crate) fn new() -> Self {
        Self {
            backend: None,
            pending: BTreeMap::new(),
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_backend(backend: Box<dyn OpenUriBackend>) -> Self {
        Self {
            backend: Some(backend),
            pending: BTreeMap::new(),
            generation: 0,
        }
    }

    // Starts one independent OpenURI request; concurrent opens never supersede
    // each other.
    pub(crate) fn start(&mut self, path: PathBuf) -> Result<u64, StartError> {
        let uri = OpenUri::from_file_path(&path).map_err(StartError::Path)?;
        let backend = self
            .backend_mut()
            .ok_or(StartError::Portal(PortalOutcome::Unavailable))?;
        let request = OpenUriRequest::new(uri, PortalParent::None)
            .start(&mut **backend, |_| true)
            .map_err(StartError::Portal)?;

        self.generation = self
            .generation
            .checked_add(1)
            .expect("portal file-open generation exhausted");
        let generation = self.generation;
        self.pending
            .insert(generation, PendingFile { path, request });
        Ok(generation)
    }

    // Polls all live requests once after a portal completion edge.
    pub(crate) fn pump(&mut self) -> Vec<OpenCompletion> {
        let Some(backend) = self.backend.as_deref_mut() else {
            return Vec::new();
        };
        let mut completed = Vec::new();
        for (&generation, file) in &mut self.pending {
            let outcome = file.request.poll(backend);
            file.request.settle(backend);
            if let Some(outcome) = outcome {
                completed.push(OpenCompletion {
                    generation,
                    path: file.path.clone(),
                    outcome,
                });
            }
        }
        for completion in &completed {
            self.pending.remove(&completion.generation);
        }
        completed
    }

    // Event-driven wake source. A fresh request gets one priming wake to close
    // the start/subscription race, then waits for the portal signal.
    pub(crate) fn completion_subscription(&self) -> iced::Subscription<PortalWake> {
        if !self
            .pending
            .values()
            .any(|file| file.request.is_unsettled())
        {
            return iced::Subscription::none();
        }
        let Some(signal) = self
            .backend
            .as_deref()
            .and_then(OpenUriBackend::open_uri_completion_signal)
        else {
            return iced::Subscription::none();
        };

        iced::Subscription::run_with(
            OpenCompletionKey {
                generation: self.generation,
                signal,
            },
            |data| {
                let signal = data.signal.clone();
                iced::futures::stream::unfold(
                    (signal, None::<u64>),
                    async |(signal, seen): (CompletionSignal, Option<u64>)| match seen {
                        None => {
                            let generation = signal.generation();
                            Some((PortalWake { generation }, (signal, Some(generation))))
                        }
                        Some(seen) => {
                            let generation = signal.wait_after(seen).await;
                            Some((PortalWake { generation }, (signal, Some(generation))))
                        }
                    },
                )
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn backend_mut(&mut self) -> Option<&mut Box<dyn OpenUriBackend>> {
        if self.backend.is_none() {
            match enzui_core::portal::dbus_backend::DbusBackend::new() {
                Ok(backend) => self.backend = Some(Box::new(backend)),
                Err(_) => return None,
            }
        }
        self.backend.as_mut()
    }
}

impl Drop for PortalFileOpener {
    fn drop(&mut self) {
        let Some(backend) = self.backend.as_deref_mut() else {
            return;
        };
        for (_, mut file) in std::mem::take(&mut self.pending) {
            let _ = file.request.cancel(CancelReason::Application);
            file.request.settle(backend);
        }
    }
}

#[derive(Clone)]
struct OpenCompletionKey {
    generation: u64,
    signal: CompletionSignal,
}

impl std::hash::Hash for OpenCompletionKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.generation.hash(state);
    }
}

pub(crate) use enzui_core::portal::open_uri::desktop_launcher::OpenRoute;

// Whether a terminal portal outcome should be retried on the launcher.
//
// `Failed` retries: portals use it when declining the request entirely, and the
// launcher may still open the file. Refusals ([`PortalOutcome::Denied`],
// [`PortalOutcome::NotDeclared`]), a vanished window and user cancellation stay
// final.
pub(crate) fn outcome_should_fallback(outcome: &PortalOutcome<()>) -> bool {
    matches!(
        outcome,
        PortalOutcome::Unavailable | PortalOutcome::Unsupported | PortalOutcome::Failed
    )
}

pub(crate) fn should_fallback(error: &StartError) -> bool {
    matches!(
        error,
        StartError::Portal(
            PortalOutcome::Unavailable | PortalOutcome::Unsupported | PortalOutcome::Failed
        )
    )
}

// User-facing failure text for a request that did not open; cancellation is
// silent.
pub(crate) fn failure_text(
    path: &Path,
    failure: &Result<PortalOutcome<()>, StartError>,
) -> Option<String> {
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let detail = match failure {
        Err(StartError::Path(error)) => {
            return Some(crate::i18n::format(
                "files-could-not-open-name-error-22340e22",
                &[("name", (name)), ("error", (error).to_string())],
            ));
        }
        Err(StartError::Portal(outcome)) | Ok(outcome) => match outcome {
            PortalOutcome::Completed(()) | PortalOutcome::Cancelled(_) => return None,
            PortalOutcome::Unavailable => crate::i18n::tr("the desktop portal is unavailable"),
            PortalOutcome::Unsupported => {
                crate::i18n::tr("the desktop portal does not support opening files")
            }
            PortalOutcome::NotDeclared => {
                crate::i18n::tr("this build is not permitted to open files")
            }
            PortalOutcome::Denied => crate::i18n::tr("the desktop portal refused permission"),
            PortalOutcome::ParentUnavailable => {
                crate::i18n::tr("the open request could not be attached to this window")
            }
            PortalOutcome::Failed => crate::i18n::tr("the desktop portal could not open the file"),
        },
    };
    Some(crate::i18n::format(
        "files-could-not-open-name-detail-2e757e76",
        &[("name", (name)), ("detail", (detail).to_owned())],
    ))
}
