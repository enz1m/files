// Explicit Open With command launching.
//
// Ordinary activation uses the portal and falls back to `gio open`. Explicit
// Open With choices run `gio launch <desktop-file> <target>` or a typed command.
// Every launch is detached: the opened app outlives the request, is never
// killed by a deadline or a process-group teardown, and gets the session's
// whole environment, as an app started from the desktop does.

use enzui_core::cancellation::CancelToken;
use enzui_services::detached_launch::{self, DetachedLaunch, LaunchError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

// How long a launch is watched for an early failure. `gio` reports a missing
// handler or an unreadable file well within it; an app still running after
// it is left alone.
const LAUNCH_SETTLE: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchSpec {
    program: String,
    args: Vec<OsString>,
}

impl LaunchSpec {
    pub(crate) fn default_handler() -> Self {
        Self {
            program: "gio".to_owned(),
            args: vec![OsString::from("open")],
        }
    }

    // `gio launch <desktop-file>` for an installed handler, the desktop file as
    // one exact argument.
    pub(crate) fn installed_handler(desktop_file: &Path) -> Self {
        Self {
            program: "gio".to_owned(),
            args: vec![
                OsString::from("launch"),
                desktop_file.as_os_str().to_os_string(),
            ],
        }
    }

    // A user-typed command split on whitespace (`"code -w"` is `code` with `-w`);
    // `None` when the text has no token.
    pub(crate) fn parse_custom(text: &str) -> Option<Self> {
        let mut tokens = text.split_whitespace();
        let program = tokens.next()?.to_owned();
        Some(Self {
            program,
            args: tokens.map(OsString::from).collect(),
        })
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    // The fixed arguments, in order, that precede the target path.
    pub(crate) fn args(&self) -> &[OsString] {
        &self.args
    }

    #[cfg(test)]
    pub(crate) fn argv_for(&self, target: &Path) -> Vec<OsString> {
        let mut argv = Vec::with_capacity(self.args.len() + 2);
        argv.push(OsString::from(self.program.as_str()));
        argv.extend(self.args.iter().cloned());
        argv.push(target.as_os_str().to_os_string());
        argv
    }
}

// The outcome of one launch: `Ok` names the file handed off, `Err` carries the
// failure toast line.
pub(crate) type LaunchResult = Result<PathBuf, String>;

// The seam the reducer launches an explicit Open With choice through.
//
// `Send + Sync`, because it is cloned into `Task::perform` closures.
pub(crate) trait OpenLauncher: Send + Sync {
    // Launches `spec` against `path`, appended as one final argument.
    //
    // Nothing is re-split, so whitespace in a desktop file or target survives.
    fn run_command(&self, spec: &LaunchSpec, path: &Path) -> LaunchResult;

    // [`run_command`] under a caller-owned cancellation token.
    //
    // [`run_command`]: OpenLauncher::run_command
    fn run_command_cancellable(
        &self,
        spec: &LaunchSpec,
        path: &Path,
        cancel: &CancelToken,
    ) -> LaunchResult {
        if cancel.is_terminal() {
            Err(cancelled_line(path))
        } else {
            self.run_command(spec, path)
        }
    }
}

pub(crate) fn cancelled_line(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    crate::i18n::format(
        "files-could-not-open-name-launch-cancelled-835bff63",
        &[("name", (name))],
    )
}

// Production launcher: resolves the program on the search path captured at
// boot and starts it detached. Waiting for the settle window is synchronous
// inside a `Task::perform` worker, never on the reducer thread.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProcessLauncher {
    search: crate::exe_resolver::SearchPath,
}

impl ProcessLauncher {
    pub(crate) fn new(search: crate::exe_resolver::SearchPath) -> Self {
        Self { search }
    }

    // Translates a launch into a [`LaunchResult`], naming `program` and `path`
    // in any error line.
    fn finish(
        program: &str,
        path: &Path,
        result: Option<Result<detached_launch::Launched, LaunchError>>,
    ) -> LaunchResult {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let program = program.to_owned();
        match result {
            Some(Ok(_)) => Ok(path.to_path_buf()),
            None => Err(crate::i18n::format(
                "files-could-not-open-program-not-found-466148a9",
                &[("arg0", name), ("program", program)],
            )),
            Some(Err(LaunchError::ExitedEarly(code))) => {
                let detail = match code {
                    Some(code) => crate::i18n::format(
                        "files-program-exited-with-status-code-900e7c14",
                        &[("program", program), ("code", code.to_string())],
                    ),
                    None => crate::i18n::format(
                        "files-program-was-terminated-a6921ce3",
                        &[("program", program)],
                    ),
                };
                Err(crate::i18n::format(
                    "files-could-not-open-detail-fee436ad",
                    &[("arg0", name), ("detail", detail)],
                ))
            }
            Some(Err(LaunchError::Spawn(detail))) => Err(crate::i18n::format(
                "files-could-not-open-program-spawn-failed-f79b7b67",
                &[("arg0", name), ("program", program), ("detail", detail)],
            )),
            Some(Err(error)) => Err(crate::i18n::format(
                "files-could-not-open-program-error-ea29041f",
                &[
                    ("arg0", name),
                    ("program", program),
                    ("error", error.to_string()),
                ],
            )),
        }
    }
}

impl OpenLauncher for ProcessLauncher {
    fn run_command(&self, spec: &LaunchSpec, path: &Path) -> LaunchResult {
        self.run_command_cancellable(spec, path, &CancelToken::new())
    }

    fn run_command_cancellable(
        &self,
        spec: &LaunchSpec,
        path: &Path,
        cancel: &CancelToken,
    ) -> LaunchResult {
        if cancel.is_terminal() {
            return Err(cancelled_line(path));
        }
        let program = spec.program();
        // Args and the target stay separate `OsString`s; nothing is re-split.
        let result = self.search.resolve(program).map(|exe| {
            let folder = path
                .parent()
                .filter(|parent| parent.is_dir())
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
            let request = DetachedLaunch::new(exe, folder)
                .args(spec.args().iter().cloned())
                .arg(path.as_os_str().to_os_string())
                .envs(std::env::vars_os())
                .settle(LAUNCH_SETTLE);
            detached_launch::launch(&request)
        });
        Self::finish(program, path, result)
    }
}

// The launcher handle carried on the app: a trait object so tests can swap in a
// recorder, and `Arc` for `'static` task futures.
pub(crate) type OpenLauncherHandle = Arc<dyn OpenLauncher>;

pub(crate) fn default_launcher() -> OpenLauncherHandle {
    Arc::new(ProcessLauncher::new(
        crate::exe_resolver::SearchPath::from_env(),
    ))
}

// Test-only helpers; only `#[cfg(test)]` unit tests wire the recorder.

// A record of one call the reducer made through the launcher.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchCall {
    pub(crate) path: PathBuf,
    // The structured launch `run_command` was handed; always `Some` here.
    pub(crate) command: Option<LaunchSpec>,
}

#[cfg(test)]
impl LaunchCall {
    pub(crate) fn argv(&self) -> Option<Vec<OsString>> {
        self.command.as_ref().map(|spec| spec.argv_for(&self.path))
    }
}

// A test launcher that records calls in a mutex and returns a scripted result.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct RecordingLauncher {
    inner: std::sync::Mutex<RecordingInner>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct RecordingInner {
    calls: Vec<LaunchCall>,
    // `None` -> return `Ok(path)`; `Some(err)` -> return that error verbatim.
    scripted_error: Option<String>,
}

#[cfg(test)]
impl RecordingLauncher {
    pub(crate) fn succeeding() -> Self {
        RecordingLauncher {
            inner: std::sync::Mutex::new(RecordingInner::default()),
        }
    }

    pub(crate) fn calls(&self) -> Vec<LaunchCall> {
        self.inner.lock().unwrap().calls.clone()
    }
}

#[cfg(test)]
impl OpenLauncher for RecordingLauncher {
    fn run_command_cancellable(
        &self,
        spec: &LaunchSpec,
        path: &Path,
        cancel: &CancelToken,
    ) -> LaunchResult {
        if cancel.is_terminal() {
            return Err(cancelled_line(path));
        }
        self.run_command(spec, path)
    }

    fn run_command(&self, spec: &LaunchSpec, path: &Path) -> LaunchResult {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.calls.push(LaunchCall {
            path: path.to_path_buf(),
            command: Some(spec.clone()),
        });
        match &inner.scripted_error {
            None => Ok(path.to_path_buf()),
            Some(msg) => Err(msg.clone()),
        }
    }
}
