// App construction and boot.

use super::*;

impl App {
    #[cfg(test)]
    pub(crate) fn new() -> (Self, Task<Msg>) {
        Self::new_with_persistence(PersistenceRoots::disarmed(), false)
    }

    #[cfg(test)]
    pub(crate) fn new_with_persistence(
        roots: PersistenceRoots,
        _persist_bookmarks: bool,
    ) -> (Self, Task<Msg>) {
        Self::new_with_start_directory(roots, _persist_bookmarks, None)
    }

    pub(crate) fn new_with_start_directory(
        roots: PersistenceRoots,
        _persist_bookmarks: bool,
        requested: Option<PathBuf>,
    ) -> (Self, Task<Msg>) {
        // Roots are snapshotted once, never re-read from `dirs::*` or `XDG_*`.
        let PersistenceRoots {
            pin_mode: pin_mode_root,
            pin_authority: pin_authority_root,
            pin_path: pin_path_root,
            pin_boot_refusal,
            features_persistence,
            downloads_settings,
            home_data_dir,
            placement_path,
            view_settings_path,
            shortcuts_path,
            home_trash,
            thumbnail_cache,
            async_view_settings_boot,
        } = roots;
        let home = services::home_dir();
        // Capture paths expand against the isolated HOME.
        let startup_directory_requested =
            requested.is_some() || std::env::var_os("FILES_APP_START_DIR").is_some();
        let start = requested
            .or_else(|| {
                std::env::var("FILES_APP_START_DIR")
                    .ok()
                    .map(|raw| services::expand_user_path(&raw, &home))
                    .filter(|p| p.is_dir())
            })
            .unwrap_or_else(|| home.clone());
        let mut model = Model::new(home);
        // Home and XDG folders are ordinary pins, so every Places row can be
        // reordered or unpinned.
        model.promote_default_places_to_pins();
        model.cwd.clone_from(&start);
        // View settings are restored before the first directory read, so frame one
        // uses them. A corrupt or future-version file yields defaults and a summary,
        // and the store stays write-protected so an edit cannot replace the refused
        // bytes. Must follow setting `cwd`.
        let vs_store = view_settings_store::store(view_settings_path.as_deref());
        // Shortcut overrides are a small bounded file, read before the first
        // key event so a rebind answers from frame one. A refused file opens
        // with defaults and a notice; the store stays write-protected.
        let shortcuts_store = keybindings::store(shortcuts_path.as_deref());
        let shortcuts_load = shortcuts_store.load();
        model.shortcuts_notice = shortcuts_load.user_summary();
        let shortcuts = keybindings::catalog(shortcuts_load.into_value());
        keybindings::install(&shortcuts);
        // Production publishes defaults and loads off-thread via
        // [`enzui_core::persist::LoadController::begin`], settled by
        // `Msg::ViewSettingsLoaded`. The eager branch is reachable only from test
        // roots, which set `async_view_settings_boot = false`.
        let (
            view_settings_load_notice,
            view_settings_controller,
            view_settings_load_baseline,
            view_settings_boot_task,
        ): (
            Option<String>,
            Option<enzui_core::persist::LoadController>,
            Option<view_settings_store::ViewSettingsToken>,
            Task<Msg>,
        ) = if async_view_settings_boot {
            let mut controller = enzui_core::persist::LoadController::new();
            match controller.begin(&vs_store) {
                Ok(load_task) => {
                    // The settlement applies the load only while the model token
                    // still equals this baseline, so edits made meanwhile win.
                    let baseline = view_settings_store::ViewSettings::baseline_token(&model);
                    (
                        None,
                        Some(controller),
                        Some(baseline),
                        load_task.map(|completion| {
                            Msg::ViewSettingsLoaded(std::sync::Arc::new(
                                view_settings_load::ViewSettingsLoadHandoff::new(completion),
                            ))
                        }),
                    )
                }
                Err(err) => {
                    // No worker slot: report it and continue with defaults.
                    (
                        Some(crate::i18n::format(
                            "files-view-settings-could-not-be-loaded-815567aa",
                            &[("err", (err).to_string())],
                        )),
                        None,
                        None,
                        Task::none(),
                    )
                }
            }
        } else {
            let persisted = vs_store.load();
            let notice = persisted.user_summary();
            model.apply_view_settings(persisted.value());
            // The saved tabs, unless this launch names its own folder.
            if !startup_directory_requested {
                model.restore_session(&persisted.value().tabs, persisted.value().active_tab);
            }
            (notice, None, None, Task::none())
        };
        {
            let (loaded_features, outcome) = features::load_bounded(&features_persistence);
            model.features = loaded_features;
            match outcome {
                features::FeaturesLoadOutcome::Applied | features::FeaturesLoadOutcome::Absent => {}
                features::FeaturesLoadOutcome::Lost => {
                    model.features_refusal = Some(features::FeaturesRefusal::CustodyLost);
                }
                features::FeaturesLoadOutcome::Refused(refusal) => {
                    model.features_refusal = Some(refusal);
                }
            }
            if features_persistence.is_unavailable() {
                model.features_refusal = Some(features::FeaturesRefusal::AuthorityUnavailable);
            }
        }
        {
            let outcome = model.downloads.load_settings_bounded(&downloads_settings);
            if downloads_settings.is_unavailable() {
                model.downloads.settings_refusal =
                    Some(downloads::SettingsRefusal::AuthorityUnavailable);
            }
            let _ = outcome;
        }
        // A first run starts with the default pins; a v2 file is authoritative even
        // with every default unpinned. Legacy files hold only custom bookmarks,
        // which are appended after the defaults.
        if let Some(ref authority) = pin_authority_root {
            let defaults = model.bookmarks.clone();
            match services::load_sidebar_pins_from_authority(authority, &defaults) {
                services::PinLoadOutcome::Admitted {
                    pins,
                    retained_rows,
                    retained_bytes,
                } => {
                    model.bookmarks = pins;
                    model.pin_retained_rows = retained_rows;
                    model.pin_retained_bytes = retained_bytes;
                }
                services::PinLoadOutcome::Refused(refusal) => {
                    model.pins_refused = Some(refusal);
                }
            }
        } else if let Some(refusal) = pin_boot_refusal {
            model.pins_refused = Some(refusal);
        }
        // Capture-only startup dialog, unset in production. `FILES_APP_OPEN=properties`
        // opens Properties on the start dir; `=view-settings` opens View Settings.
        let open = std::env::var("FILES_APP_OPEN").unwrap_or_default();
        // The Open With seed still uses the real injected query; this holds the MIME
        // until the app exists.
        let mut startup_open_with_mime: Option<String> = None;
        let mut startup_torrent_fixture = false;
        let mut startup_activity_fixture = false;
        // Capture-only: one outcome toast in either tone.
        let mut startup_toast_fixture: Option<(String, ToastTone)> = None;
        // `properties` inspects `start`; `properties:<path>` inspects that path.
        if let Some(rest) = open.strip_prefix("properties") {
            let target = rest
                .strip_prefix(':')
                .map_or_else(|| start.clone(), PathBuf::from);
            if let Ok(info) = services::build_properties(&target) {
                model.open_properties(info);
            }
        } else if open == "toast-success" {
            startup_toast_fixture = Some((
                crate::i18n::tr("Created Untitled Folder").to_owned(),
                ToastTone::Success,
            ));
        } else if open == "toast-failure" {
            startup_toast_fixture = Some((
                crate::i18n::tr("Could not unmount: device is busy").to_owned(),
                ToastTone::Error,
            ));
        } else if open == "previous-versions" || open == "browse-history" || open == "history-menu"
        {
            // Capture-only: the history dialogs and Backup submenu with sample
            // versions, without the enzimOS history service.
            model.history_available = true;
            model.history_fixture = true;
            let now = crate::history::now();
            if open == "previous-versions" {
                let version = |number, ago: i64, size, current| crate::history::Version {
                    number,
                    taken: now - ago,
                    size,
                    modified: now - ago - 600,
                    current,
                };
                model.history = Some(crate::history::Dialog::Versions(crate::history::VersionsDialog {
                    path: start.join("Quarterly report.txt"),
                    versions: Some(Ok(vec![
                        version(41, 1800, 18_204, true),
                        version(38, 3 * 3600, 17_950, false),
                        version(30, 26 * 3600, 12_480, false),
                        version(12, 9 * 86_400, 9_312, false),
                    ])),
                    selected: Some(1),
                    preview: Some(crate::history::Preview::Text(
                        "Quarterly report\n================\n\nRevenue grew 12% over the quarter, led by the new\nsubscription plans. Costs were flat.\n\nNext steps:\n- hire two support engineers\n- ship the annual plan".to_owned(),
                    )),
                    busy: false,
                    error: None,
                }));
            } else if open == "browse-history" {
                let item = |name: &str, dir, size, exists| crate::history::Item {
                    name: name.to_owned(),
                    dir,
                    size,
                    modified: now - 86_400,
                    exists,
                };
                model.history = Some(crate::history::Dialog::Browse(
                    crate::history::BrowseDialog {
                        folder: start.join("Documents"),
                        snapshots: Some(Ok(vec![
                            (41, now - 1800),
                            (38, now - 3 * 3600),
                            (30, now - 26 * 3600),
                            (12, now - 9 * 86_400),
                        ])),
                        at: 2,
                        items: Some(Ok(vec![
                            item("Invoices", true, 0, true),
                            item("Quarterly report.txt", false, 12_480, true),
                            item("Tax 2025.pdf", false, 284_311, false),
                            item("Trip plan.odt", false, 33_002, false),
                        ])),
                        chosen: vec!["Tax 2025.pdf".to_owned()],
                        busy: false,
                        error: None,
                    },
                ));
            }
        } else if open == "view-settings" || open == "extensions" {
            model.view_settings_open = true;
            if open == "extensions" {
                model.settings_page = model::SettingsSection::Extensions;
            }
        } else if open == "select-matching" {
            model.open_select_matching();
            "*.png".clone_into(&mut model.select_matching_draft);
        } else if open == "open-with" {
            // Loading state over the real MIME database; no candidate fixture.
            let mime = "text/plain".to_owned();
            model.open_with = Some(open_with::OpenWithState::for_target(
                "notes.txt",
                mime.clone(),
            ));
            startup_open_with_mime = Some(mime);
        } else if open == "attach" {
            // Capture-only seed with a sample discovery result; the live Discover
            // button runs the injected scanner.
            let a = network::AttachState {
                discovered: network::parse_discovery(services::DISCOVERY_FIXTURE),
                discovery: network::DiscoveryState::Found,
                ..network::AttachState::default()
            };
            model.attach = Some(a);
        } else if matches!(
            open.as_str(),
            "network-untrusted" | "network-trusted-empty" | "network-unknown-empty"
        ) {
            model.navigate_screen(
                model::Screen::Network,
                PathBuf::from(services::NETWORK_VIEW_ROOT),
            );
            model.network.finish_scan(
                Ok(Vec::new()),
                match open.as_str() {
                    "network-untrusted" => network::DiscoveryContext {
                        trust: network::NetworkTrustStatus::Untrusted {
                            connection: Some("Studio Wi-Fi".to_owned()),
                        },
                        can_open_network_settings: true,
                    },
                    "network-trusted-empty" => network::DiscoveryContext {
                        trust: network::NetworkTrustStatus::Trusted,
                        can_open_network_settings: true,
                    },
                    _ => network::DiscoveryContext::default(),
                },
            );
        } else if open == "grid" || open == "grid-details" {
            // Capture-only: Grid view. `grid-details` also opens the inspector on
            // the first regular file, to show a video before and after its poster.
            model.view_mode = model::ViewMode::Grid;
            if open == "grid-details"
                && let Ok(entries) = services::read_directory(&start)
            {
                let first_file = entries
                    .iter()
                    .find(|entry| !matches!(entry.kind, model::EntryKind::Dir))
                    .map(|entry| entry.path.clone());
                model.set_entries(entries);
                if let Some(path) = first_file {
                    model.select_only(path);
                }
                model.details_open = true;
            }
        } else if matches!(
            open.as_str(),
            "downloads"
                | "downloads-absent"
                | "downloads-states"
                | "downloads-many"
                | "downloads-torrent-fixture"
                | "activity"
                | "add-download"
        ) {
            model.navigate_screen(
                model::Screen::Downloads,
                PathBuf::from(services::DOWNLOADS_VIEW_ROOT),
            );
            if open == "downloads-absent" {
                model.downloads.backend = downloads::Aria2Status::Absent;
            } else if open == "downloads-states" || open == "downloads-many" {
                let extra = if open == "downloads-many" { 14 } else { 0 };
                let downloads_dir = model.home.join("Downloads");
                model.downloads.seed_visual_states(&downloads_dir, extra);
            } else if open == "downloads-torrent-fixture" {
                model.downloads.seed_torrent_fixture(start.clone());
                startup_torrent_fixture = true;
            } else if open == "activity" {
                model.downloads.seed_activity_fixture();
                startup_activity_fixture = true;
            } else if open == "add-download" {
                model.downloads.backend = downloads::Aria2Status::Absent;
                model.downloads.add = Some(downloads::AddDownloadState::new(
                    model.home.join("Downloads"),
                    model.downloads.default_download_limit_kib(),
                ));
            }
        } else if open == "trash" {
            model.navigate_screen(
                model::Screen::Trash,
                PathBuf::from(services::TRASH_VIEW_ROOT),
            );
            model.trash_entries = trash::list_trash(&home_data_dir);
        } else if open == "network" {
            model.navigate_screen(
                model::Screen::Network,
                PathBuf::from(services::NETWORK_VIEW_ROOT),
            );
            model.network.finish_scan(
                Ok(network::parse_discovery(services::DISCOVERY_FIXTURE)),
                network::DiscoveryContext::default(),
            );
        } else if open == "computer-storage"
            && std::env::var("FILES_APP_CAPTURE_SETTLED").as_deref() == Ok("1")
        {
            model.navigate_screen(
                model::Screen::Computer,
                PathBuf::from(services::COMPUTER_VIEW_ROOT),
            );
            model.storage_capture = true;
            model.storage_ready = true;
            model.storage_locations = crate::storage::capture_fixture(&model.home);
        } else if open == "computer" || open == "computer-phones" {
            // Capture-only: the Computer screen with `model.drives` filled
            // synchronously, since capture does not settle the async drive load.
            model.navigate_screen(
                model::Screen::Computer,
                PathBuf::from(services::COMPUTER_VIEW_ROOT),
            );
            model.drives = services::list_drives();
            // `computer-phones` adds one closed and one open phone.
            if open == "computer-phones" {
                model.storage_capture =
                    std::env::var("FILES_APP_CAPTURE_SETTLED").as_deref() == Ok("1");
                model.phones = crate::devices::phones::capture_fixture();
            }
        } else if open == "details" {
            // Capture-only: the inspector with the first regular file selected.
            if let Ok(entries) = services::read_directory(&start) {
                let first_file = entries
                    .iter()
                    .find(|e| !matches!(e.kind, model::EntryKind::Dir))
                    .map(|e| e.path.clone());
                model.set_entries(entries);
                if let Some(p) = first_file {
                    model.select_only(p);
                }
                model.details_open = true;
            }
        } else if open == "filters" {
            model.search_active = true;
            model.filters_open = true;
        } else if open == "search-empty" {
            // A search query that matches nothing, to shoot the empty state.
            model.search_active = true;
            "zzzznomatch".clone_into(&mut model.search_query);
        } else if let Some(rest) = open.strip_prefix("bulk-rename") {
            // Capture-only bulk-rename dialog over a real directory read, through
            // `BulkRename::new`. `bulk-rename:find` pre-fills a Find & Replace rule.
            if let Ok(entries) = services::read_directory(&start) {
                let targets: Vec<PathBuf> = entries
                    .iter()
                    .filter(|e| !matches!(e.kind, model::EntryKind::Dir))
                    .map(|e| e.path.clone())
                    .collect();
                model.set_entries(entries);
                for t in &targets {
                    model.selection.insert(t.clone());
                }
                if !targets.is_empty() {
                    let occupied = services::occupied_siblings(&targets);
                    let mut b = bulk_rename::BulkRename::new(targets, occupied);
                    if rest == ":find" {
                        "IMG_".clone_into(&mut b.find);
                        "Photo_".clone_into(&mut b.replace);
                    }
                    model.bulk_rename = Some(b);
                }
            }
        } else if let Some(rest) = open.strip_prefix("context") {
            // Capture-only context menu on the first file, or the first directory
            // with `context:dir`. The directory is read synchronously because the
            // surface renders only against an entry in `visible_entries()`. The
            // anchor (40, 120) stays on-screen at every capture rung.
            let want_dir = rest == ":dir";
            if let Ok(entries) = services::read_directory(&start) {
                let pick = entries
                    .iter()
                    .find(|e| {
                        if want_dir {
                            matches!(e.kind, model::EntryKind::Dir)
                        } else {
                            !matches!(e.kind, model::EntryKind::Dir)
                        }
                    })
                    .map(|e| e.path.clone());
                model.set_entries(entries);
                if let Some(path) = pick {
                    model.select_only(path.clone());
                    model.context_menu = model
                        .projected_entries()
                        .into_iter()
                        .find(|e| e.path == path)
                        .map(|e| (e.path.clone(), matches!(e.kind, model::EntryKind::Dir)))
                        .and_then(|(p, is_dir)| {
                            let rows = context_menu::menu_model(&model, &p, is_dir);
                            let invoker = (model.view_mode == model::ViewMode::List)
                                .then(|| context_menu::list_row_id(&p));
                            context_menu::OpenContextMenu::new_with_restore(
                                p,
                                iced::Point::new(40.0, 120.0),
                                rows,
                                invoker,
                            )
                        });
                }
            }
        } else if let Some(rest) = open.strip_prefix("pin-hover") {
            // Capture-only drop-to-pin state: a nested compositor cannot synthesise
            // an OS file drag. Uses the same `pin_hover_begin` as the event arm.
            let target = rest
                .strip_prefix(':')
                .map_or_else(|| start.clone(), PathBuf::from);
            model.pin_hover_begin(target);
        }
        // Loaded before the first window opens, so windows restore without jumping.
        // Tests have no `placement_path`, so they never rewrite real geometry.
        let placement = match placement_path.as_deref() {
            // A malformed or absent file opens at defaults rather than refusing to
            // start.
            Some(path) => {
                PlacementTracker::with_loaded(enzui_core::window_placement::load(path).into_map())
            }
            None => PlacementTracker::new(),
        };

        let (themes, theme_boot) = ThemeIntegration::boot();
        let downloads_enabled = model.features.enabled(features::FeatureId::Downloads);
        // Baseline of the booting state, so only a real edit writes.
        let view_settings_gate = enzui_core::persist::SaveGate::with_baseline(
            vs_store,
            view_settings_store::ViewSettings::token(&model),
        );
        let view_settings_boot_snapshot = view_settings_load_baseline
            .as_ref()
            .map(|_| view_settings_store::ViewSettings::snapshot(&model));
        let mut app = Self {
            model,
            file_clipboard: file_clipboard::Runtime::default(),
            #[cfg(feature = "center")]
            center: None,
            #[cfg(feature = "center")]
            agent: enzui_agent_app::AgentProcess::default(),
            #[cfg(feature = "center")]
            stopping: None,
            #[cfg(feature = "center")]
            stop_exit: false,
            #[cfg(feature = "center")]
            waiting_center_calls: Vec::new(),
            themes,
            window_width: 0.0,
            window_height: 0.0,
            dialogs: enzui_widgets::modal_host::DialogStack::new(),
            delete_confirm_focus: None,
            settings_invoker: None,
            settings_presentation: enzui_widgets::modal_host::ModalPresentation::default(),
            picker: picker::Picker::new(),
            desktop_launch: None,
            pending_launches: Vec::new(),
            native_drop: native_drop::NativeDrop::default(),
            ledger: WindowLedger::new(),
            main_window: None,
            close_pending: None,
            close_deadline: None,
            shutdown_helper: None,
            previews: std::collections::BTreeMap::new(),
            preview_frames: std::collections::BTreeMap::new(),
            placement,
            placement_path,
            launcher: launch::default_launcher(),
            file_opener: portal_open::PortalFileOpener::new(),
            open_route: portal_open::OpenRoute::for_evidence(
                &enzui_core::portal::sandbox::SandboxEvidence::from_host(),
            ),
            mime_query: mime_query::default_query(),
            scanner: network::default_scanner(),
            network_context_probe: network::default_context_probe(),
            settings_opener: network::default_settings_opener(),
            mounter: network::default_mounter(),
            pin_mode: pin_mode_root,
            pin_authority: pin_authority_root,
            pin_path: pin_path_root,
            home_data_dir,
            home_trash,
            thumbnail_cache,
            features_persistence,
            downloads_settings,
            network_operation: None,
            network_cancel: None,
            launch_operations: std::collections::BTreeMap::new(),
            next_launch_generation: 0,
            next_attach_generation: 0,
            mime_query_operation: None,
            mime_query_cancel: None,
            mime_write_operations: std::collections::BTreeMap::new(),
            next_mime_generation: 0,
            unmount_service: services::UnmountService::production(),
            unmount_operations: std::collections::BTreeMap::new(),
            next_unmount_generation: 0,
            thumbnail_operations: std::collections::HashMap::new(),
            thumbnail_queue: std::collections::BTreeMap::new(),
            preview_cache: enzui_resources::HandleCache::new(thumbnail::PREVIEW_CACHE_BYTES),
            folder_measurements: folder_size::Measurements::default(),
            activity_center: {
                let mut center = ActivityCenter::new();
                center.register_provider(activity::FILES, true);
                center.register_provider(
                    enzui_core::operation_activity::downloads::DOWNLOADS,
                    downloads_enabled,
                );
                center
            },
            activity_schedule: ActivitySchedule::default(),
            activity_clock: std::time::Instant::now(),
            next_file_activity_id: 0,
            shell_activity: shell_activity::ShellActivity::default(),
            startup_directory_requested,
            conflict_activity: None,
            archive_work: archive::ArchiveWork::new(),
            extension_cancel: None,
            extension_activity: None,
            device_work: devices::DeviceWork::new(),
            pending_extract: None,
            mount_activity: None,
            toasts: ToastHost::new().id_prefix("files-app-toast"),
            toast_expiry: ToastExpirySchedule::default(),
            transfer_failure_toast: None,
            view_settings_gate,
            view_settings_save_error: None,
            shortcuts,
            shortcuts_store,
            view_settings_boot_snapshot,
            view_settings_controller,
            view_settings_load_baseline,
            boot_transfers: Vec::new(),
            // Keep constructors syscall-free; production swaps this worker in boot.
            dir_watch: dir_watch_worker::DirWatchWorker::spawn(std::sync::Arc::new(
                enzui_core::file_tree_watch::DirectoryWatchService::new(
                    enzui_core::file_tree_watch::NoopBackend,
                ),
            )),
            current_dir_watch: None,
            dir_watch_generation: 0,
            trash_load_generation: 0,
            dir_watch_pending_rescan: false,
            dir_load_in_flight: false,
            dir_watch_warned: 0,
            dir_watch_rescan_due: false,
            listed_mtime: None,
            verify_after_listing: false,
            listings_in_flight: std::collections::HashMap::new(),
            search_walk: None,
            dir_watch_vanished: false,
        };
        app.publish_downloads();
        if let Some(summary) = view_settings_load_notice {
            app.post_status(summary, ToastTone::Error);
        }
        if startup_activity_fixture {
            let _ = app.start_file_activity(
                ActivityKind::Copy,
                crate::i18n::tr("Copying 2 items to Documents"),
                2,
            );
        }
        if let Some((text, tone)) = startup_toast_fixture {
            // Capture-only, with a long dwell for the screenshot. Leaves
            // `status_message` alone: `post_status` is its only writer.
            app.toasts.push(
                ToastSpec::new(text)
                    .tone(tone)
                    .duration(std::time::Duration::from_secs(3600)),
            );
            app.toasts
                .sync_expiry(&mut app.toast_expiry, std::time::Instant::now());
        }
        let boot_generation = app.model.begin_load();
        let policy = model::ProjectionPolicy::from_model(&app.model);
        let load = Task::perform(
            load_prepared_dir(start.clone(), policy.clone()),
            move |(mtime, result)| Msg::DirPrepared {
                path: start.clone(),
                generation: boot_generation,
                policy,
                mtime,
                result,
            },
        );
        let mut tasks = vec![load, theme_boot.map(|event| Msg::Theme(Box::new(event)))];
        if let Some(mime) = startup_open_with_mime {
            let query = std::sync::Arc::clone(&app.mime_query);
            let (generation, process_cancel, scan_cancel) = app.begin_mime_query();
            let task =
                query_mime_applications(query, mime, generation, process_cancel, scan_cancel);
            tasks.push(app.own_mime_query(generation, task));
        }
        if startup_torrent_fixture {
            // Held on the app: the visual-QA boot discards the load task but must
            // still run these.
            app.boot_transfers = app
                .model
                .downloads
                .promote()
                .into_iter()
                .map(downloads::transfer_task)
                .collect();
        }
        // `Task::none()` in test fixtures, whose state is already loaded.
        tasks.push(view_settings_boot_task);
        #[cfg(not(test))]
        if let Some(root) = crate::extensions::Catalog::root_path() {
            app.model.extensions.root = Some(root.clone());
            tasks.push(Task::perform(
                crate::shell_work::run(move || crate::extensions::load(root)),
                Msg::ExtensionsLoaded,
            ));
        }
        (app, Task::batch(tasks))
    }

    #[cfg(test)]
    pub(crate) fn with_launcher(mut self, launcher: launch::OpenLauncherHandle) -> Self {
        self.launcher = launcher;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_mime_query(mut self, query: mime_query::MimeQueryHandle) -> Self {
        self.mime_query = query;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_scanner(mut self, scanner: network::ShareScannerHandle) -> Self {
        self.scanner = scanner;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_network_context_probe(
        mut self,
        probe: network::NetworkContextProbeHandle,
    ) -> Self {
        self.network_context_probe = probe;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_settings_opener(mut self, opener: network::SettingsOpenerHandle) -> Self {
        self.settings_opener = opener;
        self
    }

    // The daemon's boot function: builds the state and opens the main window.
    //
    // The window id arrives later as a message, so [`App::main_window`] is an
    // `Option` and the size is requested in the `MainWindowOpened` arm.
    pub(crate) fn boot() -> (Self, Task<Msg>) {
        #[cfg_attr(
            test,
            allow(
                unused_mut,
                reason = "production attaches the activity publisher after boot"
            )
        )]
        let (mut app, task) = Self::boot_with_arguments(
            PersistenceRoots::production(),
            true,
            std::env::args_os().skip(1),
        );
        #[cfg(not(test))]
        {
            app.shell_activity = shell_activity::ShellActivity::production();
        }
        // The shipping boot alone may expose the app.
        #[cfg(all(feature = "center", not(test)))]
        app.start_center();
        (app, task)
    }

    pub(crate) fn boot_with_arguments(
        roots: PersistenceRoots,
        persist_bookmarks: bool,
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> (Self, Task<Msg>) {
        let arguments: Vec<_> = arguments.into_iter().collect();
        let package = startup::first_extension_package(&arguments);
        let archive = startup::first_archive(&arguments);
        let requested = startup::first_local_directory(arguments);
        let (mut app, load) = Self::new_with_start_directory(roots, persist_bookmarks, requested);
        let load = if let Some(package) = package {
            Task::batch([load, Task::done(Msg::ReviewExtension(package))])
        } else if let Some(archive) = archive {
            Task::batch([load, Task::done(Msg::OpenArchiveExternally(archive))])
        } else {
            load
        };

        #[cfg(target_os = "linux")]
        app.install_real_dir_watch_backend();

        app.arm_dir_watch();
        // Both boot paths run these; the settled branch must not drop them with
        // `load`.
        let transfers = std::mem::take(&mut app.boot_transfers);

        // Visual QA opens no window until the fixture and its previews are ready,
        // prepared off the UI thread, so first paint is deterministic.
        if std::env::var("FILES_APP_CAPTURE_SETTLED").as_deref() == Ok("1") {
            let root = app.model.cwd.clone();
            // The same descriptor-relative cache path as the real scheduler; `None`
            // means durable caching is off.
            let cache = app.durable_cache_root_for_dispatch();
            let capture = Task::perform(prepare_capture(root, cache), |(entries, thumbnails)| {
                Msg::CapturePrepared {
                    entries,
                    thumbnails,
                }
            });
            return (
                app,
                Task::batch(Self::assemble_settled_boot(capture, transfers)),
            );
        }

        // Sanitised against the output bounds, so a size from a larger monitor
        // still fits.
        let restored = app.placement.get_for(&WindowRole::Main);

        let mut settings = desktop_identity().window_settings(MAIN_LAYOUT);
        settings.exit_on_close_request = false;
        if let Some(p) = restored {
            settings.size = p.to_iced_size();
        }

        let (_id, open) = iced::window::open(settings);
        (
            app,
            Task::batch([
                Task::batch(Self::assemble_windowed_boot(
                    load,
                    open.map(Msg::MainWindowOpened),
                    transfers,
                )),
                Task::perform(crate::history::refresh(), |r| {
                    Msg::History(crate::history::HistoryMsg::Refreshed(r))
                }),
            ]),
        )
    }

    // Capture and transfer tasks must both survive settled-mode startup.
    pub(crate) fn assemble_settled_boot(
        capture: Task<Msg>,
        transfers: Vec<Task<Msg>>,
    ) -> Vec<Task<Msg>> {
        std::iter::once(capture).chain(transfers).collect()
    }

    // Window startup retains constructor-armed transfers.
    pub(crate) fn assemble_windowed_boot(
        load: Task<Msg>,
        open: Task<Msg>,
        transfers: Vec<Task<Msg>>,
    ) -> Vec<Task<Msg>> {
        [load, open].into_iter().chain(transfers).collect()
    }
}
