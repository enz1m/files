//! Native file manager built on enzUI and iced.

#![deny(unconditional_recursion)]

mod activity;
mod archive;
mod atomic_publish;
#[cfg(feature = "center")]
mod background;
mod bulk_rename;
#[cfg(feature = "center")]
mod center;
mod conflict;
mod context_menu;

mod desktop_launch;
mod desktop_storage;
mod devices;
mod dialog;
mod downloads;
mod drag_drop;
mod exe_resolver;
mod extensions;
mod features;
mod file_clipboard;
mod folder_colors;
mod folder_size;
mod i18n;
mod icon;
mod journal;
mod keybindings;
mod launch;
mod message;
mod mime_query;
mod model;
mod mount_service;
mod native_drop;

mod network;
mod open_with;
mod picker;
mod portal_open;

mod history;
mod preview;
mod preview_pdf;
mod private_data;
mod project;
mod service_demo;
mod services;
mod settings_presentation;
mod settings_surface;
mod shell_activity;

mod shell_work;
mod startup;
mod storage;
mod tabs;
mod templates;
mod terminal;

mod thumbnail;

mod trash;
mod view;
mod view_settings_load;
mod view_settings_store;

mod dir_watch_worker;

#[cfg(all(test, target_os = "linux"))]
mod file_tree_watch_linux_tests;

use std::path::{Path, PathBuf};

use enzui_core::cancellation::{CancelCause, CancelToken};
use enzui_core::focus::{Traversal, traversal_operation as traverse};
use enzui_core::operation_activity::{
    ActivityCenter, ActivityControl, ActivityKind, ActivitySchedule, LivePhase, TerminalOutcome,
};
use enzui_core::theme_file::integration::ThemeIntegration;
use enzui_core::virtual_sequence::ItemProvider;
use enzui_widgets::breadcrumb::{BreadcrumbEditEvent, edit_focus_operation};
use enzui_widgets::modal_host::ModalHostEvent;
use iced::{Element, Subscription, Task, advanced::widget::operate};

use enzui_widgets::app_shell::{DesktopApplicationIdentity, ShellLayout};
use enzui_widgets::toast::ToastTone;
use enzui_widgets::toast_host::{
    ToastEvent, ToastExpirySchedule, ToastHost, ToastOverlayPlacement, ToastSpec, enz_toast_layer,
};
use enzui_widgets::window_lifecycle::{PlacementTracker, WindowLedger, lifecycle_task};
use message::CreateVerb;
use message::Msg;
use model::{Model, Screen};
use preview::WindowRole;
use view::widgets::{path_bar_id, view_settings_invoker_id};

const MAIN_LAYOUT: ShellLayout = ShellLayout::TitlebarSidebarContent;

// The Wayland application id every files-app window advertises.
//
// The basename of `io.github.enzui.Files.desktop`, so the compositor associates
// the browser window and every detached preview with the installed entry.
pub(crate) const APP_ID: &str = "io.github.enzui.Files";

// The shared desktop application identity for this app's windows; [`APP_ID`] is
// a valid constant, so construction cannot fail.
pub(crate) fn desktop_identity() -> DesktopApplicationIdentity {
    DesktopApplicationIdentity::new(APP_ID)
        .expect("BUG: files-app APP_ID must be a valid desktop application basename")
}

// The app name for the placement state file under `$XDG_STATE_HOME`.
const PLACEMENT_APP: &str = "files-app";

// Opens the freedesktop `Trash` and `thumbnails` roots as descriptor-bound
// [`private_data::HomeTrashAuthority`] and
// [`private_data::ThumbnailCacheAuthority`] handles.
//
// The one adopter for production and test constructors; takes
// [`private_data::AppPrivateRoots`] instead of reading process env.
pub(crate) fn compute_private_authorities(
    roots: &private_data::AppPrivateRoots,
) -> (
    private_data::HomeTrashAuthority,
    private_data::ThumbnailCacheAuthority,
) {
    (roots.resolve_home_trash(), roots.resolve_thumbnail_cache())
}

// Completes the breadcrumb's mode swap after the reducer rebuilt it.
fn breadcrumb_focus_task(event: &BreadcrumbEditEvent) -> Task<Msg> {
    match edit_focus_operation::<Msg>(event, path_bar_id()) {
        Some(operation) => {
            let focus = iced::advanced::widget::operate(operation);
            if matches!(event, BreadcrumbEditEvent::Begin) {
                focus.chain(iced::advanced::widget::operate(
                    enzui_widgets::text_field::select_all(path_bar_id()),
                ))
            } else {
                focus
            }
        }
        None => Task::none(),
    }
}

// Runs the app as an `iced::daemon`, since `iced::application` owns one window
// and cannot express detached previews.
//
// A daemon opens no window at boot ([`App::boot`] opens the main window by
// `Task`) and never exits on its own, so the subscription carries
// `close_events()`.
fn main() -> std::process::ExitCode {
    let _ = enzui_core::logging::init_default();
    if let Some(status) = extensions::helper_mode() {
        return status;
    }
    if let Some(status) = storage::helper_mode() {
        return status;
    }
    enzui_core::crash_report::CrashReporter::for_app("files-app", env!("CARGO_PKG_VERSION"));
    #[cfg(feature = "center")]
    {
        use enzui_agent::center::background::LaunchMode;
        match background::launch_mode() {
            Ok(LaunchMode::ExportManifest) => return background::export_manifest(),
            Ok(LaunchMode::Background) => {
                // No display, renderer or window until a desktop launch asks for one.
                return match background::run_background() {
                    Ok(Some(app)) => run_gui(Some(app)),
                    Ok(None) => std::process::ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("files-app: background mode: {error}");
                        std::process::ExitCode::FAILURE
                    }
                };
            }
            Ok(LaunchMode::Gui) => {}
            Err(error) => {
                eprintln!("files-app: {error}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    run_gui(None)
}

fn run_gui(presented: Option<App>) -> std::process::ExitCode {
    #[cfg(feature = "center")]
    let mut service = None;
    #[cfg(feature = "center")]
    if presented.is_none() {
        match background::claim_for_gui() {
            Ok(background::DesktopClaim::Owner(owned)) => service = Some(owned),
            Ok(background::DesktopClaim::Forwarded) => return std::process::ExitCode::SUCCESS,
            Ok(background::DesktopClaim::Independent) => {}
            Err(error) => {
                // Never start beside an instance that refused, nor an unverified
                // one beside the installed service.
                eprintln!("files-app: Files could not be opened: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    #[cfg(not(feature = "center"))]
    let desktop_service = match desktop_launch::claim() {
        Ok(desktop_launch::Claim::Owner(owner)) => Some(owner),
        Ok(desktop_launch::Claim::Forwarded) => return std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("files-app: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Demo stand-ins for the enzimOS services (`FILES_APP_DEMO_SERVICES=1`).
    service_demo::start();
    #[cfg(not(feature = "center"))]
    let desktop_service = std::cell::RefCell::new(desktop_service);
    // A windowless worker hands over its state once; other boots are fresh.
    #[cfg(feature = "center")]
    let pending = std::cell::RefCell::new(presented);
    #[cfg(not(feature = "center"))]
    let _ = presented;
    #[cfg(feature = "center")]
    let service = std::cell::RefCell::new(service);
    let boot = move || {
        #[cfg(feature = "center")]
        if let Some(mut app) = pending.borrow_mut().take() {
            let task = app.present_task();
            return (app, task);
        }
        #[cfg_attr(
            not(feature = "center"),
            allow(unused_mut, reason = "center feature restores pending window state")
        )]
        let (mut app, task) = App::boot();
        #[cfg(feature = "center")]
        {
            app.agent = background::with_graceful_stop(enzui_agent_app::AgentProcess::desktop(
                service.borrow_mut().take(),
            ));
        }
        #[cfg(not(feature = "center"))]
        {
            app.desktop_launch = desktop_service.borrow_mut().take();
        }
        (app, task)
    };
    let services = enzui_widgets::app_setup::AppServices::default()
        .localizer(i18n::app_localizer())
        .traversal(enzui_widgets::app_setup::TraversalBinding::Disabled)
        .accessibility_actions(false);
    let program = enzui_widgets::app_setup::with_services(
        iced::daemon(boot, App::update, App::view_window)
            .title(App::title)
            .subscription(App::subscription)
            .theme(App::theme),
        services,
    );
    let result = enzui_widgets::app_setup::run_program(program);
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("files-app: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum WatchInstallOutcome {
    Armed(enzui_core::file_tree_watch::WatchId),
    Refused(enzui_core::file_tree_watch::WatchRefusal),
    NotOnDirectoryScreen,
}

// Whether bookmark persistence is armed for real writes; only `#[cfg(test)]`
// constructors may use `TestDisarmed`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PinPersistenceMode {
    // Real boot; an unavailable path or authority sets `pins_refused` and blocks
    // mutations.
    Production,
    // Test fixture without a persistence path; mutations stay in memory. Only
    // constructed under `#[cfg(test)]`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only test fixtures bypass bookmark persistence")
    )]
    TestDisarmed,
}

// Custody-aware outcome of a bookmark publication attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveOutcome {
    // Custody rotated; the mutation is durable.
    Published,
    // Test-disarmed, in memory only; constructed under `#[cfg(test)]`.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only test fixtures publish without persistence")
    )]
    Disarmed,
    // A refused state is active; the mutation was blocked.
    Blocked,
    // Pre-publication or unchanged failure; the caller must roll back.
    Unchanged,
    // Bytes are at the destination but durability is uncertain: keep the
    // mutation, retain authority and record durability debt.
    Rotated,
    // Custody lost: the refused state is entered and mutations are blocked.
    Lost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveGenericDialog {
    Conflict,
    BulkRename,
    Properties,
    OpenWith,
    Attach,
    AddDownload,
    ViewSettings,
    SelectMatching,
    Shortcuts,
    AllActivity,
    DeviceConfirm,
    FolderColor,
    MountQuestion,
    Unlock,
    Location,
    Format,
    Cloud,
    History,
}

struct App {
    model: Model,
    file_clipboard: file_clipboard::Runtime,
    themes: ThemeIntegration,
    // Native size feeds dialog limits and asynchronous thumbnail scheduling.
    window_width: f32,
    // The live window height from `Resized`, for the settings dialog geometry.
    // Starts at 0.0.
    window_height: f32,
    // Settings retains its invoker through the keyed exit animation.
    settings_invoker: Option<iced::advanced::widget::Id>,
    dialogs: enzui_widgets::modal_host::DialogStack<ActiveGenericDialog>,
    delete_confirm_focus: Option<enzui_widgets::modal_host::ModalFocusLifecycle>,
    // Retention for the adaptive View Settings surface.
    //
    // Dismissal starts the exit animation and [`Msg::ViewSettingsExitSettled`]
    // retires only the matching generation, so a late settlement cannot destroy a
    // rapid reopen.
    settings_presentation: enzui_widgets::modal_host::ModalPresentation<()>,
    // The XDG portal directory chooser behind Move/Copy to…, built lazily.
    picker: picker::Picker,
    desktop_launch: Option<desktop_launch::DesktopLaunch>,
    pending_launches: Vec<enzui_core::activation::dbus::ApplicationCall>,
    native_drop: native_drop::NativeDrop,
    // The process-wide enzui-center session, when opted in and enrolled.
    #[cfg(feature = "center")]
    center: Option<enzui_agent::center::CenterHost<crate::center::CenterCall>>,
    // Single-instance ownership and the windowless runtime's state.
    #[cfg(feature = "center")]
    agent: enzui_agent_app::AgentProcess<Msg>,
    // An accepted OS stop waiting for its work to finish.
    #[cfg(feature = "center")]
    stopping: Option<graceful_stop::Stopping>,
    // A windowless worker finished an accepted stop and exits.
    #[cfg(feature = "center")]
    stop_exit: bool,
    // Listing calls admitted while a folder was still loading.
    #[cfg(feature = "center")]
    waiting_center_calls: Vec<(enzui_agent::center::CenterCall, crate::center::CenterCall)>,

    // Which windows are open, and therefore when the process exits.
    //
    // A daemon runs with zero windows; see the `Msg::WindowClosed` arm.
    ledger: WindowLedger,

    // The main window's id, or `None` before it opens.
    //
    // Window gestures name it explicitly: `iced::window::latest()` may be a preview.
    main_window: Option<iced::window::Id>,
    // The closing window whose background retirement still owns the runtime.
    close_pending: Option<iced::window::Id>,
    close_deadline: Option<std::time::Instant>,
    shutdown_helper: Option<std::thread::JoinHandle<downloads::ShutdownOutcome>>,

    // The open previews, keyed by window. Ids last only for the process, so
    // placement uses `PlacementKey` instead.
    previews: std::collections::BTreeMap<iced::window::Id, preview::Preview>,
    // Decoded video frames of media previews, keyed by preview window. Each
    // presenter allocates in its own window's renderer and shows the last
    // uploaded frame until the next one is resident there.
    preview_frames:
        std::collections::BTreeMap<iced::window::Id, enzui_widgets::video::LatestFramePresenter>,

    // Persistent window geometry, keyed by [`WindowRole`].
    //
    // Coalesces resizes into one atomic write and refuses stale replies.
    placement: PlacementTracker,

    // Where [`Self::placement`] is flushed; `None` keeps placement in memory, so a
    // missing `$HOME` does not stop windows opening.
    placement_path: Option<std::path::PathBuf>,

    // The subprocess launcher for an explicit Open With choice; default opening
    // uses `file_opener`.
    launcher: launch::OpenLauncherHandle,

    // Default-application launches through the XDG Desktop Portal OpenURI.
    file_opener: portal_open::PortalFileOpener,

    // Whether ordinary opening uses the portal (sandboxed) or the desktop launcher,
    // read once at startup from the shared sandbox policy.
    open_route: portal_open::OpenRoute,

    // The freedesktop MIME database query. The test default rejects both methods,
    // so no test depends on the host database.
    mime_query: mime_query::MimeQueryHandle,

    // The SMB discovery scanner; tests inject `network::RecordingScanner`.
    scanner: network::ShareScannerHandle,

    // The mounter; tests inject `network::RecordingMounter` to see the composed
    // URI without running `gio`.
    mounter: network::MounterHandle,

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "tests read which persistence mode booted")
    )]
    pin_mode: PinPersistenceMode,
    pin_authority: Option<enzui_core::state_authority::StateAuthority>,
    pin_path: Option<std::path::PathBuf>,

    // The parent of the freedesktop Trash leaf, for path-shaped APIs.
    //
    // Gate on [`Self::home_trash`] first: when it is `Unavailable` this is empty
    // and nothing may be scheduled against it.
    home_data_dir: std::path::PathBuf,

    // Descriptor-bound home-Trash authority.
    //
    // Every trash entry point checks `is_available()` and revalidates the
    // [`enzui_security::secure_dir::SecureRoot`]; a failed revalidation makes
    // it permanently `Unavailable`, so a pathname cannot reacquire it.
    home_trash: private_data::HomeTrashAuthority,

    // Descriptor-bound durable-thumbnail authority.
    thumbnail_cache: private_data::ThumbnailCacheAuthority,
    // Feature-state persistence; inert when test-disarmed.
    features_persistence: features::FeaturesPersistence,
    // Download settings persistence; inert when test-disarmed.
    downloads_settings: downloads::SettingsPersistence,
    // Abort handle for the current Network-tab operation; cancelling suppresses a
    // stale completion.
    network_operation: Option<iced::task::Handle>,
    network_cancel: Option<network::OperationCancel>,
    // In-flight explicit handler launches, keyed by generation.
    //
    // Each launch settles on its own completion and is never superseded; only app
    // drop cancels them, reaching each child's process group.
    launch_operations: std::collections::BTreeMap<u64, HelperOwner>,
    next_launch_generation: u64,
    // Attach-dialog session counter, threaded into every
    // [`network::MountCredentials`] so a backend refuses a callback from a closed
    // or reopened dialog.
    next_attach_generation: u64,
    // The single MIME chooser query; a new query or closing the dialog
    // cancels it, and its generation rejects stale completions.
    mime_query_operation: Option<(u64, iced::task::Handle)>,
    mime_query_cancel: Option<(u64, CancelToken, network::OperationCancel)>,
    // Durable MIME association writes, keyed by generation.
    //
    // Independent of the chooser and cancelled only by app drop, so a later dialog
    // cannot revoke a committed default. Shares `next_mime_generation` with queries.
    mime_write_operations: std::collections::BTreeMap<u64, HelperOwner>,
    next_mime_generation: u64,
    // Unmount helpers; app shutdown tears down each helper's process group.
    unmount_service: services::UnmountService,
    unmount_operations: std::collections::BTreeMap<u64, HelperOwner>,
    next_unmount_generation: u64,
    // Abort handles for the currently realised thumbnail window.
    thumbnail_operations: std::collections::HashMap<std::path::PathBuf, iced::task::Handle>,
    // Realised requests waiting for a decode slot; a scroll can discard them
    // before they retain pixels.
    thumbnail_queue: std::collections::BTreeMap<std::path::PathBuf, thumbnail::Request>,
    // Decoded previews by path and mtime, bounded to
    // [`thumbnail::PREVIEW_CACHE_BYTES`].
    preview_cache: enzui_resources::HandleCache<(std::path::PathBuf, u64)>,
    // Running folder-size measurements: their generations and cancel flags.
    folder_measurements: folder_size::Measurements,

    // One union of file and Downloads work, rendered by the shell.
    activity_center: ActivityCenter,
    // At most one one-shot terminal-linger deadline.
    activity_schedule: ActivitySchedule,
    activity_clock: std::time::Instant,
    next_file_activity_id: u64,
    shell_activity: shell_activity::ShellActivity,
    startup_directory_requested: bool,
    // The file operation currently blocked in the conflict dialog.
    conflict_activity: Option<activity::FileActivity>,
    // The bounded owner for archive work and each operation's cancel handle.
    archive_work: archive::ArchiveWork,
    extension_cancel: Option<enzui_core::cancellation::CancelToken>,
    extension_activity: Option<activity::FileActivity>,
    // The bounded owner for removable-media enumeration and operations.
    device_work: devices::DeviceWork,
    // The archive waiting for an "Extract To…" folder choice.
    pending_extract: Option<std::path::PathBuf>,
    // The in-flight network mount, whose activity row closes on `AttachMounted`.
    mount_activity: Option<activity::FileActivity>,

    // The visible and accessible surface for transient outcomes.
    //
    // Failures and meaningful results go through [`App::post_status`]. Each toast
    // is a live region: `Role::Alert` for failures, `Role::Status` otherwise.
    toasts: ToastHost,
    // The one-shot expiry schedule for `toasts`, reconciled after every reducer
    // turn via [`ToastHost::sync_expiry`]; an idle host runs no timer.
    toast_expiry: ToastExpirySchedule,
    // The download failure currently raised as a sticky error toast, so a new
    // failure posts once and a resolved one withdraws its toast.
    transfer_failure_toast: Option<(String, enzui_widgets::toast_host::ToastId)>,

    // The coalesced save gate for view settings; an unchanged token never
    // serialises.
    view_settings_gate: enzui_core::persist::SaveGate<
        view_settings_store::ViewSettings,
        view_settings_store::ViewSettingsToken,
    >,
    // The most recent view-settings save error. The gate retries on the next
    // change.
    view_settings_save_error: Option<String>,
    // Default and user shortcut chords; its resolved table is installed for
    // dispatch after every change.
    shortcuts: keybindings::FilesShortcuts,
    // Where shortcut overrides persist, saved off-thread after each change.
    shortcuts_store: enzui_core::persist::Store<enzui_core::editable_shortcuts::ShortcutOverrides>,
    // Off-thread view-settings load controller; `None` on the eager test path.
    view_settings_controller: Option<enzui_core::persist::LoadController>,
    // The view-settings token before the off-thread load started.
    //
    // The loaded record applies only if the token is unchanged, so edits made
    // meanwhile win. `None` when no async load was armed.
    view_settings_load_baseline: Option<view_settings_store::ViewSettingsToken>,
    // The record the model held when the load began, so edits made meanwhile
    // can be told apart field by field and kept on top of the loaded record.
    view_settings_boot_snapshot: Option<view_settings_store::ViewSettings>,
    // Torrent transfers armed during construction by the capture fixture.
    //
    // Held separately because the visual-QA boot branch replaces the load task,
    // which would otherwise drop the live jobs.
    boot_transfers: Vec<Task<Msg>>,

    // One worker serializes blocking install/retire calls, keeping only the latest
    // request.
    dir_watch: dir_watch_worker::DirWatchWorker,
    current_dir_watch: Option<enzui_core::file_tree_watch::WatchId>,
    dir_watch_generation: u64,
    // The watch generation a live-updates warning was last shown for, so a
    // failing watch says so once rather than on every event batch.
    dir_watch_warned: u64,
    // The current folder's modification time when its listing read it, and
    // whether an armed watch asked for that to be checked once it lands.
    listed_mtime: Option<(PathBuf, Option<std::time::SystemTime>)>,
    verify_after_listing: bool,
    // A batched rescan for folder changes is scheduled.
    dir_watch_rescan_due: bool,
    dir_watch_pending_rescan: bool,
    dir_load_in_flight: bool,
    // Listings running on a worker, by folder, with the newest load generation
    // that adopted each. A reload of a folder already being listed waits for
    // that listing instead of starting another, so Refresh on a hung mount
    // cannot pile up stuck workers.
    listings_in_flight: std::collections::HashMap<PathBuf, u64>,
    // The running Everywhere search walk, aborted when a newer query starts.
    search_walk: Option<iced::task::Handle>,
    dir_watch_vanished: bool,
    // Bumped by every event that starts or invalidates a scan, so older replies
    // are dropped.
    trash_load_generation: u64,
}

// The resolved plan for one Attach-dialog Confirm: anonymous, or with a password
// in a wipe-on-drop [`enzui_core::form::SecretText`]. Kept local to the
// reducer so the password cannot escape the dispatch.
struct AttachDispatch {
    uri: String,
    credentials: Option<network::MountCredentials>,
}

#[path = "app_parts/assist.rs"]
mod assist;
#[path = "app_parts/boot.rs"]
mod boot;
#[path = "app_parts/fixtures.rs"]
mod fixtures;
#[cfg(feature = "center")]
#[path = "app_parts/graceful_stop.rs"]
mod graceful_stop;
#[path = "app_parts/persistence.rs"]
mod persistence;
pub(crate) use fixtures::{HelperOwner, PersistenceRoots};
#[path = "app_parts/classify.rs"]
mod classify;
#[path = "app_parts/journal.rs"]
mod journal_ops;
#[path = "app_parts/reduce.rs"]
mod reduce;
pub(crate) use classify::{
    JournalAdmissionTally, apply_inverse_async, apply_renames, classify_event_with_status,
    classify_trash_incomplete, delete_trash_selection, describe_conflict,
    finalize_conflict_restore, load_drives, load_prepared_dir, load_trash, new_file, new_folder,
    open_paste_queue, permanent_delete_selection, plan_paste, plan_trash_restore, prepare_capture,
    query_mime_applications, recursive_search, recursive_size, resolve_conflict,
    restore_trash_entry, trash_listing_warning, trash_selection, trashinfo_path_of, unmount_drive,
};
#[path = "app_parts/reduce_archive.rs"]
mod reduce_archive;
#[path = "app_parts/reduce_aux.rs"]
mod reduce_aux;
#[path = "app_parts/reduce_clipboard.rs"]
mod reduce_clipboard;
#[path = "app_parts/reduce_devices.rs"]
mod reduce_devices;
#[path = "app_parts/reduce_downloads.rs"]
mod reduce_downloads;
#[path = "app_parts/reduce_extensions.rs"]
mod reduce_extensions;
#[path = "app_parts/reduce_folder_size.rs"]
mod reduce_folder_size;
#[path = "app_parts/reduce_history.rs"]
mod reduce_history;
#[path = "app_parts/reduce_mount_service.rs"]
mod reduce_mount_service;
#[path = "app_parts/reduce_nav.rs"]
mod reduce_nav;
#[path = "app_parts/reduce_project.rs"]
mod reduce_project;
#[path = "app_parts/reduce_remote.rs"]
mod reduce_remote;
#[path = "app_parts/reduce_search.rs"]
mod reduce_search;
#[path = "app_parts/reduce_shell.rs"]
mod reduce_shell;
#[path = "app_parts/reduce_shortcuts.rs"]
mod reduce_shortcuts;
#[path = "app_parts/reduce_storage_dialogs.rs"]
mod reduce_storage_dialogs;
#[path = "app_parts/reduce_tail.rs"]
mod reduce_tail;
#[path = "app_parts/reduce_trash.rs"]
mod reduce_trash;
#[path = "app_parts/reduce_view.rs"]
mod reduce_view;
#[path = "app_parts/shell_helpers.rs"]
mod shell_helpers;
#[path = "app_parts/tasks.rs"]
mod tasks;
#[path = "app_parts/thumbnails.rs"]
mod thumbnails;
#[path = "app_parts/view.rs"]
mod view_mod;

#[cfg(test)]
mod stale_recursive_search_tests;

#[cfg(test)]
mod bulk_rename_tests;

#[cfg(test)]
mod conflict_tests;

#[cfg(test)]
mod loading_tests;

#[cfg(test)]
mod drag_drop_tests;

#[cfg(test)]
mod open_with_tests;

#[cfg(test)]
mod delete_tests;

#[cfg(test)]
mod file_clipboard_tests;

#[cfg(test)]
mod archive_reducer_tests;
#[cfg(test)]
mod create_tests;
