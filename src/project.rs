// Repository inspection executes no project hooks or tools; trust is a Files preference only.
// Network work-tree comparisons require explicit user admission and cancel on navigation.

use enzui_services::vcs::git::{self, Repository, StatusBudget, StatusSummary};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Files that configure developer tools to run in a project. Files never
// reads their contents; it names them so the person knows they are there.
pub(crate) const TOOL_FILES: &[&str] = &[
    "mise.toml",
    ".mise.toml",
    ".tool-versions",
    ".envrc",
    "Taskfile.yml",
    "Taskfile.yaml",
    "justfile",
    "Justfile",
    "flake.nix",
    "devenv.nix",
    ".devcontainer",
    ".nvmrc",
    "rust-toolchain.toml",
];

// The Developer package's project opener. Files hands it the project
// folder as its only argument, on click; it opens the folder in the
// application chosen for `application/x-enzimos-project` and runs nothing
// from the project itself.
pub(crate) const OPEN_PROJECT_HELPER: &str = "/usr/bin/enzimos-open-project";

// The session the opener needs to reach the desktop and its application
// associations; everything else is cleared.
const OPEN_PROJECT_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
];

// The automatic look's budget: small, because it runs for every folder
// opened. The walk for untracked files stops after this many entries (the
// count then reads "N+"), skips ignored and nested repositories, and never
// enters a submodule; hashing settles at most a few same-size files.
const BADGE_BUDGET: StatusBudget = StatusBudget {
    max_untracked_visits: 8_000,
    max_hash_bytes: 8 * 1024 * 1024,
    max_file_hash: 4 * 1024 * 1024,
};

// What the strip shows about one folder.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProjectInfo {
    // The folder this describes.
    pub(crate) dir: PathBuf,
    // The repository's top: its work tree, or the repository itself.
    pub(crate) root: PathBuf,
    pub(crate) kind: git::RepositoryKind,
    // "main", "detached at 1a2b3c4"…
    pub(crate) head: Option<String>,
    // "Merging", "Rebasing"…
    pub(crate) operation: Option<&'static str>,
    // The repository this one is nested in, if any.
    pub(crate) enclosing: Option<PathBuf>,
    pub(crate) status: StatusView,
    // Tool configuration files present at the root, by name.
    pub(crate) tools: Vec<String>,
    // The person marked this project trusted in Files.
    pub(crate) trusted: bool,
    // The Developer package's opener is installed.
    pub(crate) open_project: bool,
    // Where the repository is published: its `origin` remote, else its
    // first one.
    pub(crate) remote: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StatusView {
    Checking,
    Ready(StatusSummary),
    Unavailable,
    // A network folder: counted only when the person asks.
    OnRequest,
}

// The project state Files keeps.
#[derive(Debug, Default)]
pub(crate) struct Project {
    pub(crate) current: Option<ProjectInfo>,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Clone)]
pub(crate) enum ProjectMsg {
    // The repository (or none) for the folder of `generation`.
    Found {
        generation: u64,
        found: Option<Box<(ProjectInfo, Repository)>>,
    },
    // The work-tree comparison for `generation`.
    StatusRead {
        generation: u64,
        status: StatusView,
    },
    // Copy the selection's (or the folder's) path within the repository.
    CopyRelativePath,
    // Copy the repository's remote URL.
    CopyRemote,
    // Open the remote's web page in the browser.
    OpenRemote,
    // Count the changes of a network folder's repository.
    CountChanges,
    // Open the repository in the person's chosen development application.
    OpenProject,
    // The opener finished: `Ok` once the application has the folder, or
    // why it could not.
    ProjectOpened(Result<(), String>),
    Trust,
    Untrust,
    TrustSaved(Result<(), String>),
}

impl Project {
    // Start looking at `dir`, cancelling any earlier look. Returns the
    // generation the answers will carry and the flag the work polls.
    pub(crate) fn begin(&mut self, dir: &Path) -> (u64, Arc<AtomicBool>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.current.as_ref().is_some_and(|info| info.dir != dir) {
            self.current = None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        (self.generation, cancel)
    }

    // The flag the current look's work polls.
    pub(crate) fn cancel_flag(&self) -> Option<Arc<AtomicBool>> {
        self.cancel.clone()
    }

    // Whether an answer stamped `generation` is still wanted.
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }

    // The repository-relative path of `path`, `/`-separated.
    pub(crate) fn relative_path(&self, path: &Path) -> Option<String> {
        let info = self.current.as_ref()?;
        let relative = path.strip_prefix(&info.root).ok()?;
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(if parts.is_empty() {
            ".".to_owned()
        } else {
            parts.join("/")
        })
    }
}

// Find the repository for `dir` and read its branch and tool files.
pub(crate) fn find(dir: PathBuf, trusted: Vec<PathBuf>) -> Option<(ProjectInfo, Repository)> {
    let repository = git::discover(&dir)?;
    let root = repository
        .work_tree
        .clone()
        .unwrap_or_else(|| repository.git_dir.clone());
    let head = repository.head().ok();
    let tools = TOOL_FILES
        .iter()
        .filter(|name| std::fs::symlink_metadata(root.join(name)).is_ok())
        .map(|name| (*name).to_owned())
        .collect();
    let info = ProjectInfo {
        trusted: trusted.iter().any(|path| path == &root),
        dir,
        root,
        kind: repository.kind,
        head: head.as_ref().map(git::Head::label),
        operation: head
            .and_then(|head| head.operation)
            .map(git::Operation::label),
        enclosing: repository.enclosing.clone(),
        open_project: is_executable(Path::new(OPEN_PROJECT_HELPER)),
        status: match &repository.work_tree {
            None => StatusView::Ready(StatusSummary::default()),
            Some(work_tree) if enzui_core::background_io::is_network_filesystem(work_tree) => {
                StatusView::OnRequest
            }
            Some(_) => StatusView::Checking,
        },
        tools,
        remote: remote_url(&repository.common_dir),
    };
    Some((info, repository))
}

// The URL of the repository's `origin` remote, else of its first remote,
// read from its config.
pub(crate) fn remote_url(common_dir: &Path) -> Option<String> {
    let config = std::fs::read_to_string(common_dir.join("config")).ok()?;
    let mut remotes: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            current = line
                .strip_prefix("[remote \"")
                .and_then(|rest| rest.strip_suffix("\"]"))
                .map(str::to_owned);
            continue;
        }
        if let Some(name) = &current
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            remotes.push((name.clone(), value.trim().to_owned()));
        }
    }
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(_, url)| url.clone())
}

// A remote URL as a person reads it: no scheme, no user, no `.git`, so
// `https://github.com/a/b.git` and `git@github.com:a/b.git` both read
// `github.com/a/b`.
pub(crate) fn remote_label(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((_, rest)) => (true, rest),
        None => (false, url),
    };
    let rest = rest.split_once('@').map_or(rest, |(_, host)| host);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    // Without a scheme, `host:path` is scp-like and reads as `host/path`;
    // after one, the colon starts a port and stays.
    match rest.split_once(':') {
        Some((host, path)) if !scheme => format!("{host}/{}", path.trim_start_matches('/')),
        _ => rest.to_owned(),
    }
}

// The web page of a remote address, for the forges that serve one at the
// same host and path: `https` and `http` as given, and `ssh://`, `git://` and
// scp-like `git@host:path` addresses as `https://host/path`. Credentials, a
// port and the `.git` suffix are dropped. `None` for a local path or a
// `file://` remote, which has no page.
pub(crate) fn remote_web_url(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, url),
    };
    let web_scheme = match scheme.as_deref() {
        Some("https") => "https",
        Some("http") => "http",
        Some("ssh" | "git" | "git+ssh" | "ssh+git") => "https",
        Some(_) => return None,
        // scp-like `user@host:path`; a bare path has no colon before a slash.
        None => {
            let (host, _) = url.split_once(':')?;
            if host.contains('/') || host.is_empty() {
                return None;
            }
            "https"
        }
    };
    let rest = rest.split_once('@').map_or(rest, |(_, host)| host);
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (host, path) = match scheme {
        None => rest.split_once(':')?,
        Some(_) => rest.split_once('/')?,
    };
    // A port belongs to the transport, not to the web page.
    let host = match scheme.as_deref() {
        Some("https" | "http") => host,
        _ => host.split(':').next().unwrap_or(host),
    };
    let path = path.trim_start_matches('/');
    (!host.is_empty() && !path.is_empty()).then(|| format!("{web_scheme}://{host}/{path}"))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

// Run `helper` with `dir` as its only argument and report how it ended:
// `Ok` once launched, else the sentence to show. The path is passed as one
// argv entry, never through a shell.
pub(crate) fn open_project(helper: &Path, dir: &Path) -> Result<(), String> {
    use enzui_core::cancellation::CancelToken;
    use enzui_services::process_transport::{self, ProcessRequest, StdinPolicy};
    let session = OPEN_PROJECT_ENV.iter().filter_map(|key| {
        std::env::var_os(key).map(|value| (std::ffi::OsString::from(*key), value))
    });
    let request = ProcessRequest::new(
        helper.as_os_str().to_os_string(),
        StdinPolicy::Null,
        4096,
        4096,
        std::time::Duration::from_secs(30),
    )
    .arg(dir.as_os_str().to_os_string())
    .envs(session);
    let output = process_transport::run(request, &CancelToken::new()).map_err(|error| {
        crate::i18n::format(
            "files-the-project-opener-did-not-run-a3c0e73d",
            &[("error", (error).to_string())],
        )
    })?;
    // The helper prints one JSON object: schema 1 and a typed `status`. Its
    // stderr is deliberately uninformative, so it is never shown.
    let status = opener_status(&output.stdout);
    match (output.status.code(), status.as_deref()) {
        (Some(0), _) => Ok(()),
        (_, Some(status)) => Err(opener_message(status)),
        _ => Err(crate::i18n::tr("The project opener failed.").to_owned()),
    }
}

// The `status` string of the opener's one-line JSON answer.
fn opener_status(stdout: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?;
    let after = text.get(text.find("\"status\"")? + "\"status\"".len()..)?;
    let value = after
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let status = value.get(..value.find('"')?)?;
    status
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        .then(|| status.to_owned())
}

// What the person reads for each typed opener failure.
fn opener_message(status: &str) -> String {
    match status {
        "no-project-handler" => {
            crate::i18n::tr("No application is set to open projects. Choose one in Settings → Development.")
        }
        "handler-does-not-accept-files" => {
            crate::i18n::tr("The application chosen for projects can't open folders. Choose another in Settings → Development.")
        }
        "invalid-directory" => crate::i18n::tr("This folder can't be opened as a project."),
        "launch-failed" => crate::i18n::tr("The application chosen for projects didn't start."),
        "ordinary-user-required" => crate::i18n::tr("Projects open only from an ordinary user account."),
        "desktop-launch-unavailable" => crate::i18n::tr("Projects open only from a desktop session."),
        _ => crate::i18n::tr("The project opener failed."),
    }
    .to_owned()
}

// Compare the work tree, stopping when `cancel` is raised.
pub(crate) fn status(repository: &Repository, cancel: &AtomicBool) -> StatusView {
    let cancelled = || cancel.load(Ordering::Acquire);
    match repository.status(BADGE_BUDGET, &cancelled) {
        Ok(summary) => StatusView::Ready(summary),
        Err(_) => StatusView::Unavailable,
    }
}

// "Work tree: 3 changed · 2 untracked", "Work tree clean", "Checking…".
// Staged changes are not counted, so the label never claims the whole
// repository is clean.
pub(crate) fn status_label(status: &StatusView) -> String {
    match status {
        StatusView::Checking => crate::i18n::tr("Checking…").to_owned(),
        StatusView::Unavailable => crate::i18n::tr("Changes unknown").to_owned(),
        StatusView::OnRequest => crate::i18n::tr("Changes not counted").to_owned(),
        StatusView::Ready(summary) if summary.is_clean() && !summary.untracked_truncated => {
            crate::i18n::tr("Work tree clean").to_owned()
        }
        StatusView::Ready(summary) => {
            let mut parts = Vec::new();
            if summary.conflicted > 0 {
                parts.push(crate::i18n::format(
                    "files-in-conflict-888717d8",
                    &[("arg0", (summary.conflicted).to_string())],
                ));
            }
            let changed = summary.modified + summary.deleted;
            if changed > 0 {
                parts.push(crate::i18n::format(
                    "files-changed-changed-c33dbd5b",
                    &[("changed", (changed).to_string())],
                ));
            }
            if summary.untracked > 0 || summary.untracked_truncated {
                let plus = if summary.untracked_truncated { "+" } else { "" };
                parts.push(crate::i18n::format(
                    "files-plus-untracked-73c04f21",
                    &[
                        ("arg0", (summary.untracked).to_string()),
                        ("plus", (plus).to_owned()),
                    ],
                ));
            }
            let estimated = if summary.approximate {
                crate::i18n::tr(" (estimated)")
            } else {
                ""
            };
            crate::i18n::format(
                "files-work-tree-estimated-4a073c23",
                &[
                    ("arg0", (parts.join(" · "))),
                    ("estimated", (estimated).to_owned()),
                ],
            )
        }
    }
}

// Trusted projects: one absolute path per line, in the Files config folder.

const TRUST_FORMAT: &str = "files-app-trusted-projects";
const TRUST_VERSION: u32 = 1;

pub(crate) fn trust_path() -> Option<PathBuf> {
    dirs::config_dir().map(|config| config.join("files-app").join("trusted-projects"))
}

pub(crate) fn load_trusted(path: &Path) -> Vec<PathBuf> {
    use enzui_core::atomic_state::{StateLoad, load_with_reason};
    match load_with_reason(path, TRUST_FORMAT, TRUST_VERSION).0 {
        StateLoad::Loaded(body) => body
            .lines()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .collect(),
        StateLoad::UsedDefaults { .. } => Vec::new(),
    }
}

// Add or remove `root` from the trusted list.
pub(crate) fn set_trusted(path: &Path, root: &Path, trusted: bool) -> std::io::Result<()> {
    let mut list = load_trusted(path);
    list.retain(|known| known != root);
    if trusted {
        list.push(root.to_path_buf());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    enzui_core::atomic_state::save_atomic(path, TRUST_FORMAT, TRUST_VERSION, &body.join("\n"))
}

// Run `work` on the project worker: one job at a time, apart from the
// folder's other background work.
pub(crate) fn run<T, W>(work: W) -> impl std::future::Future<Output = Option<T>> + Send + 'static
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
{
    use enzui_core::blocking_task::BlockingTaskOwner;
    use std::num::NonZeroUsize;
    use std::sync::OnceLock;
    static OWNER: OnceLock<Option<BlockingTaskOwner>> = OnceLock::new();
    let owner = OWNER
        .get_or_init(|| {
            BlockingTaskOwner::new(
                "files-project",
                NonZeroUsize::MIN,
                NonZeroUsize::new(8).expect("non-zero"),
            )
            .ok()
        })
        .as_ref();
    let job = owner.map(|owner| {
        owner.submit_waiting(0, move |_| {
            enzui_core::background_io::lower_current_thread();
            work()
        })
    });
    async move {
        match job {
            Some(job) => job.await.into_result().ok(),
            None => None,
        }
    }
}
