// Injectable MIME associations backed by the shared desktop application service.

use crate::network::OperationCancel;
use enzui_core::cancellation::CancelToken;
#[cfg(not(test))]
use enzui_services::mime_apps::{MimeApps, MimeContext};
use std::path::PathBuf;
use std::sync::Arc;

// One installed desktop application that advertises support for a MIME type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DesktopApplication {
    // Freedesktop desktop-file ID, e.g. `org.gnome.TextEditor.desktop`.
    pub(crate) desktop_id: String,
    // User-facing `Name=` from the desktop entry.
    pub(crate) name: String,
    // Absolute `.desktop` path, handed to `gio launch` when selected.
    pub(crate) desktop_file: PathBuf,
    // The application's icon file, resolved from its `Icon=` key.
    pub(crate) icon: Option<PathBuf>,
    // Whether an explicitly injected candidate is offered as text.
    pub(crate) as_text: bool,
}

impl DesktopApplication {
    pub(crate) fn new(
        desktop_id: impl Into<String>,
        name: impl Into<String>,
        desktop_file: impl Into<PathBuf>,
    ) -> Self {
        Self {
            desktop_id: desktop_id.into(),
            name: name.into(),
            desktop_file: desktop_file.into(),
            icon: None,
            as_text: false,
        }
    }

    // The structured launch consumed by `OpenLauncher::run_command`.
    //
    // `gio`, `launch`, and the complete desktop-file path remain separate
    // arguments. The target being opened is appended by the launcher.
    pub(crate) fn launch_spec(&self) -> crate::launch::LaunchSpec {
        crate::launch::LaunchSpec::installed_handler(&self.desktop_file)
    }
}

// Independent results of the two MIME queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MimeQueryOutcome {
    pub(crate) default: Result<Option<String>, String>,
    pub(crate) applications: Result<Vec<DesktopApplication>, String>,
}

// Injected access to the host MIME application database.
pub(crate) trait MimeQuery: Send + Sync {
    // Desktop-file ID registered as the default for `mime`.
    fn default_application(&self, mime: &str) -> Result<Option<String>, String>;

    // Installed handlers in desktop preference order, including MIME parents.
    fn applications_for(&self, mime: &str) -> Result<Vec<DesktopApplication>, String>;

    // [`applications_for`] under the reducer-owned cancellation token, so a
    // chooser closed mid-scan stops the filesystem walk itself. The default
    // body, for implementations without a scan, refuses a cancelled token and
    // otherwise runs the plain call.
    //
    // [`applications_for`]: MimeQuery::applications_for
    fn applications_for_cancellable(
        &self,
        mime: &str,
        cancel: &OperationCancel,
    ) -> Result<Vec<DesktopApplication>, String> {
        if cancel.is_cancelled() {
            Err(cancelled_line(&crate::i18n::format(
                "files-applications-mime-2cf0414f",
                &[("mime", (mime).to_owned())],
            )))
        } else {
            self.applications_for(mime)
        }
    }

    // Register `desktop_id` as the durable default application for `mime`.
    //
    // Updates the captured profile's `mimeapps.list` atomically.
    fn set_default_application(&self, mime: &str, desktop_id: &str) -> Result<(), String>;

    // [`default_application`] under the reducer-owned token for this route.
    // The default body refuses a cancelled token and otherwise runs the plain
    // call.
    //
    // [`default_application`]: MimeQuery::default_application
    fn default_application_cancellable(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Option<String>, String> {
        if cancel.is_terminal() {
            Err(cancelled_line(&crate::i18n::format(
                "files-query-default-mime-0235008c",
                &[("mime", (mime).to_owned())],
            )))
        } else {
            self.default_application(mime)
        }
    }

    // [`set_default_application`] under a reducer-owned token, like
    // [`default_application_cancellable`].
    //
    // [`set_default_application`]: MimeQuery::set_default_application
    // [`default_application_cancellable`]: MimeQuery::default_application_cancellable
    fn set_default_application_cancellable(
        &self,
        mime: &str,
        desktop_id: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        if cancel.is_terminal() {
            Err(cancelled_line(&crate::i18n::format(
                "files-default-desktop-id-mime-b6f92643",
                &[
                    ("desktop_id", (desktop_id).to_owned()),
                    ("mime", (mime).to_owned()),
                ],
            )))
        } else {
            self.set_default_application(mime, desktop_id)
        }
    }
}

// The one status line a cancelled MIME operation produces.
pub(crate) fn cancelled_line(subject: &str) -> String {
    crate::i18n::format(
        "files-mime-operation-cancelled-subject-420c8708",
        &[("subject", (subject).to_owned())],
    )
}

pub(crate) type MimeQueryHandle = Arc<dyn MimeQuery>;

// Run both independent operations and preserve both outcomes under the two
// cancellation owners for this route: the framework token owns the short
// helper and the app token checkpoints the recursive filesystem scan.
pub(crate) fn query(
    query: &dyn MimeQuery,
    mime: &str,
    process_cancel: &CancelToken,
    scan_cancel: &OperationCancel,
) -> MimeQueryOutcome {
    MimeQueryOutcome {
        default: query.default_application_cancellable(mime, process_cancel),
        applications: query.applications_for_cancellable(mime, scan_cancel),
    }
}

// Captured desktop application context for default and candidate queries.
#[cfg(not(test))]
#[derive(Debug, Clone)]
pub(crate) struct NativeMimeQuery {
    context: MimeContext,
}

#[cfg(not(test))]
impl NativeMimeQuery {
    pub(crate) fn new(context: MimeContext) -> Self {
        Self { context }
    }

    fn applications(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<DesktopApplication>, String> {
        let roots: Vec<_> = std::iter::once(self.context.data_home.clone())
            .chain(self.context.data_dirs.iter().cloned())
            .collect();
        MimeApps::new(self.context.clone())
            .candidates(mime, cancel)
            .map_err(|error| error.to_string())
            .map(|apps| {
                apps.into_iter()
                    .map(|app| {
                        let icon = app.icon.and_then(|name| {
                            enzui_icons::app_icon::find_app_icon(&name, 48, &roots)
                        });
                        let mut application = DesktopApplication::new(app.id, app.name, app.path);
                        application.icon = icon;
                        application
                    })
                    .collect()
            })
    }
}

#[cfg(not(test))]
impl MimeQuery for NativeMimeQuery {
    fn default_application(&self, mime: &str) -> Result<Option<String>, String> {
        self.default_application_cancellable(mime, &CancelToken::new())
    }
    fn default_application_cancellable(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Option<String>, String> {
        MimeApps::new(self.context.clone())
            .default_for(mime, cancel)
            .map(|default| default.map(|default| default.app.id))
            .map_err(|error| error.to_string())
    }
    fn applications_for(&self, mime: &str) -> Result<Vec<DesktopApplication>, String> {
        self.applications(mime, &CancelToken::new())
    }
    fn applications_for_cancellable(
        &self,
        mime: &str,
        cancel: &OperationCancel,
    ) -> Result<Vec<DesktopApplication>, String> {
        self.applications(mime, cancel.token())
    }
    fn set_default_application(&self, mime: &str, desktop_id: &str) -> Result<(), String> {
        self.set_default_application_cancellable(mime, desktop_id, &CancelToken::new())
    }
    fn set_default_application_cancellable(
        &self,
        mime: &str,
        desktop_id: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        MimeApps::new(self.context.clone())
            .set_default(mime, desktop_id, cancel)
            .map_err(|error| error.to_string())
    }
}

#[cfg(not(test))]
pub(crate) fn default_query() -> MimeQueryHandle {
    match MimeContext::from_env() {
        Some(context) => Arc::new(NativeMimeQuery::new(context)),
        None => Arc::new(UnavailableMimeQuery),
    }
}

#[cfg(not(test))]
struct UnavailableMimeQuery;
#[cfg(not(test))]
impl MimeQuery for UnavailableMimeQuery {
    fn default_application(&self, _: &str) -> Result<Option<String>, String> {
        Err(crate::i18n::tr("The desktop application profile is unavailable").into())
    }
    fn applications_for(&self, _: &str) -> Result<Vec<DesktopApplication>, String> {
        Err(crate::i18n::tr("The desktop application profile is unavailable").into())
    }
    fn set_default_application(&self, _: &str, _: &str) -> Result<(), String> {
        Err(crate::i18n::tr("The desktop application profile is unavailable").into())
    }
}

// Test defaults must not query the host MIME database; inject RecordingMimeQuery for Open
// With.
#[cfg(test)]
pub(crate) fn default_query() -> MimeQueryHandle {
    Arc::new(RejectHostMimeQuery)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct RejectHostMimeQuery;

#[cfg(test)]
impl MimeQuery for RejectHostMimeQuery {
    fn default_application(&self, _mime: &str) -> Result<Option<String>, String> {
        Err("host MIME query disabled in tests; inject RecordingMimeQuery".to_owned())
    }

    fn applications_for(&self, _mime: &str) -> Result<Vec<DesktopApplication>, String> {
        Err("host MIME query disabled in tests; inject RecordingMimeQuery".to_owned())
    }

    fn set_default_application(&self, _mime: &str, _desktop_id: &str) -> Result<(), String> {
        Err("host MIME query disabled in tests; inject RecordingMimeQuery".to_owned())
    }
}

// Scripted test double with independent default/application outcomes and a
// call log proving which query surface was exercised.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RecordingMimeQuery {
    default: Result<Option<String>, String>,
    applications: Result<Vec<DesktopApplication>, String>,
    calls: std::sync::Mutex<Vec<(String, String)>>,
    // Cancellation tokens handed to the subprocess-facing `default`/`write`
    // query surface.
    tokens: std::sync::Mutex<Vec<CancelToken>>,
    // Tokens handed to the desktop-file scan
    // ([`applications_for_cancellable`]), kept apart from `tokens`.
    //
    // [`applications_for_cancellable`]: MimeQuery::applications_for_cancellable
    application_tokens: std::sync::Mutex<Vec<OperationCancel>>,
    // Defaults written via [`set_default_application`], keyed by mime; a
    // stored write shadows the fixed `default` on the next read.
    //
    // [`set_default_application`]: MimeQuery::set_default_application
    written_defaults: std::sync::Mutex<std::collections::HashMap<String, String>>,
    // If set, `set_default_application` returns this error instead of storing.
    write_error: Option<String>,
}

#[cfg(test)]
impl RecordingMimeQuery {
    pub(crate) fn new(
        default: Result<Option<String>, String>,
        applications: Result<Vec<DesktopApplication>, String>,
    ) -> Self {
        Self {
            default,
            applications,
            calls: std::sync::Mutex::new(Vec::new()),
            tokens: std::sync::Mutex::new(Vec::new()),
            application_tokens: std::sync::Mutex::new(Vec::new()),
            written_defaults: std::sync::Mutex::new(std::collections::HashMap::new()),
            write_error: None,
        }
    }

    pub(crate) fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }

    pub(crate) fn written_default(&self, mime: &str) -> Option<String> {
        self.written_defaults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mime)
            .cloned()
    }
}

#[cfg(test)]
impl MimeQuery for RecordingMimeQuery {
    fn default_application_cancellable(
        &self,
        mime: &str,
        cancel: &CancelToken,
    ) -> Result<Option<String>, String> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(cancel.share());
        if cancel.is_terminal() {
            Err(cancelled_line(&format!("query default {mime}")))
        } else {
            self.default_application(mime)
        }
    }

    fn set_default_application_cancellable(
        &self,
        mime: &str,
        desktop_id: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(cancel.share());
        if cancel.is_terminal() {
            Err(cancelled_line(&format!("default {desktop_id} {mime}")))
        } else {
            self.set_default_application(mime, desktop_id)
        }
    }

    fn default_application(&self, mime: &str) -> Result<Option<String>, String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(("default".to_owned(), mime.to_owned()));
        if let Some(written) = self
            .written_defaults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mime)
        {
            return Ok(Some(written.clone()));
        }
        self.default.clone()
    }

    fn applications_for(&self, mime: &str) -> Result<Vec<DesktopApplication>, String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(("applications".to_owned(), mime.to_owned()));
        self.applications.clone()
    }

    fn applications_for_cancellable(
        &self,
        mime: &str,
        cancel: &OperationCancel,
    ) -> Result<Vec<DesktopApplication>, String> {
        self.application_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(cancel.clone());
        if cancel.is_cancelled() {
            Err(cancelled_line(&format!("applications {mime}")))
        } else {
            self.applications_for(mime)
        }
    }

    fn set_default_application(&self, mime: &str, desktop_id: &str) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(("set_default".to_owned(), mime.to_owned()));
        if let Some(error) = &self.write_error {
            return Err(error.clone());
        }
        self.written_defaults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(mime.to_owned(), desktop_id.to_owned());
        Ok(())
    }
}
