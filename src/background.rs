// Desktop and background activation share one instance; only an idle windowless worker exits automatically.

use std::sync::Arc;
use std::time::{Duration, Instant};

use enzui_agent::center::CenterProvision;
use enzui_agent::center::background::Disposition;
use enzui_agent::center::task_host::{TaskHost, TaskHostError};
use enzui_agent_app::{
    AgentProcess, ApplicationIntent, BackgroundApp, BackgroundExit, DEFAULT_IDLE_EXIT, LaunchMode,
};

use super::*;

pub(crate) use enzui_agent_app::DesktopClaim;

// How long a windowless Files stays resident with nothing to do.
pub(crate) const IDLE_EXIT: Duration = DEFAULT_IDLE_EXIT;

// The package's executable path, written into the generated user unit.

// What the OS lists for Files while it runs without a window. Files
// answers graceful stop ([`with_graceful_stop`]).

// Agent sandbox writes only app-owned data; the registry exposes navigation and listing.

pub(crate) fn launch_mode() -> Result<LaunchMode, String> {
    LaunchMode::from_args(std::env::args_os().skip(1)).map_err(|error| error.to_string())
}

pub(crate) fn export_manifest() -> std::process::ExitCode {
    enzui_agent_app::print_manifest(&crate::center::registry(), crate::center::metadata())
}

pub(crate) fn claim_for_gui() -> Result<DesktopClaim, enzui_agent_app::ApplicationServiceError> {
    let locations: Vec<String> = std::env::args_os()
        .skip(1)
        .filter_map(crate::startup::local_path)
        .filter(|path| path.is_dir() || path.is_file())
        .filter_map(|path| enzui_core::portal::open_uri::OpenUri::from_file_path(&path).ok())
        .map(|uri| uri.as_str().to_owned())
        .collect();
    let intent = if locations.is_empty() {
        ApplicationIntent::Activate
    } else {
        ApplicationIntent::Open(locations)
    };
    enzui_agent_app::claim_desktop(crate::center::CENTER_APP_ID, intent)
}

// Answers the OS's graceful stop on the single instance `agent` owns, so a
// window and a windowless worker behave the same.
pub(crate) fn with_graceful_stop(mut agent: AgentProcess<Msg>) -> AgentProcess<Msg> {
    if let Err(error) = agent.serve_lifecycle(enzui_agent_app::LifecycleCallers::installed()) {
        eprintln!("files-app: graceful stop is unavailable: {error}");
    }
    agent
}

// The windowless Files process the installed unit starts, for the broker
// or for a desktop launch the bus activated.
//
// Returns the state to present when a desktop launch asked for the window,
// or `None` when the worker finished. Without an agent credential the
// worker still serves desktop launches and refuses agent access.
pub(crate) fn run_background() -> Result<Option<App>, String> {
    let provision = CenterProvision::discover(crate::center::CENTER_APP_ID);
    let Some(service) = enzui_agent_app::claim_background(crate::center::CENTER_APP_ID)? else {
        // A Files window already serves the broker.
        return Ok(None);
    };
    let tasks = Arc::new(TaskHost::spawn().map_err(|error| {
        crate::i18n::format(
            "files-task-host-did-not-start-error-f2bf1d72",
            &[("error", (error).to_string())],
        )
    })?);
    let agent = with_graceful_stop(AgentProcess::windowless(Some(service), Arc::clone(&tasks)));
    let (app, boot) = App::boot_windowless(provision, agent)?;
    match enzui_agent_app::run_background(Background(app), boot, &tasks, IDLE_EXIT)
        .map_err(|error| error.to_string())?
    {
        BackgroundExit::Present(Background(app)) => Ok(Some(app)),
        BackgroundExit::Idle(Background(mut app)) | BackgroundExit::Exit(Background(mut app)) => {
            app.finish_windowless();
            Ok(None)
        }
    }
}

impl App {
    // Builds the domain state without a window, watch or transfers, but with
    // the initial listing load.
    //
    // Without a valid credential no broker connector exists: agents are
    // refused and desktop launches still open the window.
    fn boot_windowless(
        provision: Option<CenterProvision>,
        agent: AgentProcess<Msg>,
    ) -> Result<(Self, Task<Msg>), String> {
        let (mut app, load) =
            Self::new_with_start_directory(PersistenceRoots::production(), true, None);
        app.agent = agent;
        app.center = provision.and_then(|provision| {
            enzui_agent::center::CenterHost::spawn(
                provision,
                crate::center::metadata(),
                crate::center::registry(),
            )
            .map_err(|error| eprintln!("files-app: agent access is unavailable: {error}"))
            .ok()
        });
        Ok((app, load))
    }

    // Opens the main window for a process that started without one,
    // finishing deferred renderer work and showing any forwarded folder.
    pub(crate) fn present_task(&mut self) -> Task<Msg> {
        #[cfg(target_os = "linux")]
        self.install_real_dir_watch_backend();
        self.arm_dir_watch();
        let transfers = std::mem::take(&mut self.boot_transfers);
        let restored = self.placement.get_for(&WindowRole::Main);
        let mut settings = desktop_identity().window_settings(MAIN_LAYOUT);
        settings.exit_on_close_request = false;
        if let Some(placement) = restored {
            settings.size = placement.to_iced_size();
        }
        let (_id, open) = iced::window::open(settings);
        let (launch, deferred) = self.agent.begin_presenting();
        let reveal = match launch {
            Some(ApplicationIntent::Open(uris)) if self.pending_launches.is_empty() => {
                self.open_forwarded(&uris)
            }
            _ => Task::none(),
        };
        Task::batch(
            [open.map(Msg::MainWindowOpened), deferred, reveal]
                .into_iter()
                .chain(transfers),
        )
    }

    // The exit of a windowless worker: stop admission, cancel live calls and
    // write what a window's exit writes.
    fn finish_windowless(&mut self) {
        self.shutdown_center();
        self.flush_view_settings_blocking();
    }

    pub(crate) fn application_wake(&mut self) -> Task<Msg> {
        let mut tasks = Vec::new();
        if let Some(request) = self.agent.take_stop() {
            tasks.push(self.begin_stop(request));
        }
        let calls = self.agent.take_application_calls();
        tasks.push(self.route_desktop_calls(calls));
        Task::batch(tasks)
    }

    // The single-instance wake and pre-window task results, for both the
    // windowless runtime and a presented window.
    pub(crate) fn agent_subscription(&self) -> Subscription<Msg> {
        self.agent
            .subscription(|_| Msg::ApplicationWake, Msg::BackgroundTaskRefused)
    }
}

impl App {
    pub(crate) fn center_deadline(&mut self) -> Task<Msg> {
        if let Some(host) = self.center.as_mut() {
            let _ended = host.poll_ended();
        }
        Task::none()
    }
}

// [`App`] as the windowless runtime drives it.
struct Background(App);

impl std::fmt::Debug for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Background").field(&self.0.agent).finish()
    }
}

impl BackgroundApp for Background {
    type Message = Msg;

    fn update(&mut self, message: Msg) -> Task<Msg> {
        self.0.update(message)
    }

    fn subscription(&self) -> Subscription<Msg> {
        Subscription::batch([self.0.center_subscription(), self.0.agent_subscription()])
    }

    fn disposition(&self) -> Disposition {
        let app = &self.0;
        if app.stop_exit {
            return Disposition::Exit;
        }
        enzui_agent_app::disposition(app.center.as_ref(), &app.agent, app.dir_load_in_flight)
    }

    fn wake_at(&self) -> Option<Instant> {
        self.0
            .center
            .as_ref()
            .and_then(enzui_agent_app::deadline_instant)
    }

    fn wake(&mut self) -> Task<Msg> {
        self.0.center_deadline()
    }

    fn refused(&mut self, error: TaskHostError) -> Task<Msg> {
        self.0.update(Msg::BackgroundTaskRefused(error.to_string()))
    }
}
