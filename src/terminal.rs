// Open in Terminal.
//
// The folder is passed as the working directory, never as an argument, so no
// path can be parsed as an option or expanded by a shell. The terminal is the
// configured command, else the first installed of [`CANDIDATES`]. Launches go
// through [`enzui_services::detached_launch`], which reports an immediate failure.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use enzui_services::detached_launch::{self, DetachedLaunch};

use crate::exe_resolver::SearchPath;

// Known terminals, in preference order, with the fixed arguments that make
// them open one window in the working directory.
pub(crate) const CANDIDATES: &[(&str, &[&str])] = &[
    ("xdg-terminal-exec", &[]),
    ("kgx", &[]),
    ("gnome-terminal", &[]),
    ("konsole", &[]),
    ("xfce4-terminal", &[]),
    ("foot", &[]),
    ("alacritty", &[]),
    ("kitty", &[]),
    ("wezterm", &["start"]),
    ("xterm", &[]),
];

// A resolved terminal launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPlan {
    pub(crate) name: String,
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<OsString>,
}

// The search path captured once, like the rest of the app's helpers.
fn search_path() -> &'static SearchPath {
    static SEARCH: OnceLock<SearchPath> = OnceLock::new();
    SEARCH.get_or_init(SearchPath::from_env)
}

// Resolve the terminal to launch: the configured command, else the first
// installed candidate.
pub(crate) fn plan(configured: &str) -> Result<TerminalPlan, String> {
    plan_with(search_path(), configured)
}

pub(crate) fn plan_with(search: &SearchPath, configured: &str) -> Result<TerminalPlan, String> {
    let mut tokens = configured.split_whitespace();
    if let Some(program) = tokens.next() {
        let executable = resolve(search, program).ok_or_else(|| {
            crate::i18n::format(
                "files-the-terminal-command-program-was-not-1d4e3d9d",
                &[("program", (program).to_owned())],
            )
        })?;
        return Ok(TerminalPlan {
            name: program.to_owned(),
            executable,
            args: tokens.map(OsString::from).collect(),
        });
    }
    CANDIDATES
        .iter()
        .find_map(|(program, args)| {
            search.resolve(program).map(|executable| TerminalPlan {
                name: (*program).to_owned(),
                executable,
                args: args.iter().map(OsString::from).collect(),
            })
        })
        .ok_or_else(|| {
            crate::i18n::tr(
                "No terminal was found. Install one, or set a terminal command in Settings.",
            )
            .to_owned()
        })
}

// A bare program name on the search path, or an absolute path; a relative
// path with a separator is refused rather than resolved against each entry.
fn resolve(search: &SearchPath, program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return is_executable(path).then(|| path.to_path_buf());
    }
    if program.contains('/') {
        return None;
    }
    search.resolve(program)
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

// Launches `plan` in `folder`, blocking for the settle window; call off the UI
// thread. The terminal inherits the session environment.
pub(crate) fn launch(plan: &TerminalPlan, folder: &Path) -> Result<(), String> {
    let request = DetachedLaunch::new(&plan.executable, folder)
        .args(plan.args.iter().cloned())
        .envs(std::env::vars_os());
    detached_launch::launch(&request)
        .map(|_| ())
        .map_err(|error| {
            crate::i18n::format(
                "files-could-not-open-here-error-4c123900",
                &[
                    ("arg0", (plan.name).clone()),
                    ("error", (error).to_string()),
                ],
            )
        })
}
