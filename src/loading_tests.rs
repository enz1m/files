// Folder loading under slow or hung filesystems (review batch 3).

use super::*;

// Navigating clears the old folder's files: nothing from the previous folder
// can be selected or acted on under the new path while it loads (B3.3).
#[test]
fn navigation_never_shows_the_previous_folders_files() {
    let (mut app, _task) = App::new();
    app.model.screen = Screen::Directory;
    app.model.cwd = std::env::temp_dir();
    let _ = app.update(Msg::DirLoaded {
        path: app.model.cwd.clone(),
        generation: app.model.load_generation,
        result: Ok(vec![model::FileEntry {
            path: std::env::temp_dir().join("old.txt"),
            name: "old.txt".to_owned(),
            kind: model::EntryKind::File,
            size: 1,
            modified: None,
            is_hidden: false,
            mime: "text/plain".to_owned(),
        }]),
    });
    app.model
        .navigate_to(std::env::temp_dir().join("elsewhere"));
    assert!(app.model.entries.is_empty());
    assert!(app.model.selection.is_empty());
    assert!(app.model.listing_pending);
}

// A named pipe is refused at once instead of blocking a worker (B3.10).
#[test]
fn a_named_pipe_is_never_opened_for_preview() {
    let dir = std::env::temp_dir().join(format!("files-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("notes.txt");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let started = std::time::Instant::now();
    let opened = services::open_regular_file(&fifo);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(opened.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn an_async_restored_tab_waits_for_its_own_listing() {
    use iced::futures::{StreamExt, executor::block_on};

    let scratch = tempfile::tempdir().unwrap();
    let directory = scratch.path().join("late-share");
    let settings = scratch.path().join("view-settings");
    view_settings_store::store(Some(&settings))
        .save_blocking(&view_settings_store::ViewSettings {
            tabs: vec![directory.clone()],
            ..view_settings_store::ViewSettings::default()
        })
        .unwrap();

    let mut roots = PersistenceRoots::disarmed();
    roots.view_settings_path = Some(settings);
    roots.async_view_settings_boot = true;
    let (mut app, boot) = App::new_with_persistence(roots, false);
    assert!(app.model.listing_pending);
    let home = app.model.cwd.clone();
    let boot_generation = app.model.load_generation;
    let _ = app.update(Msg::DirLoaded {
        path: home.clone(),
        generation: boot_generation,
        result: Ok(Vec::new()),
    });
    assert!(!app.model.listing_pending);

    let mut boot = iced_runtime::task::into_stream(boot).unwrap();
    let settings_loaded = block_on(async {
        loop {
            if let iced_runtime::Action::Output(message @ Msg::ViewSettingsLoaded(_)) =
                boot.next().await.expect("boot settings completion")
            {
                break message;
            }
        }
    });
    let _ = app.update(settings_loaded);
    assert_eq!(app.model.cwd, directory);
    assert!(app.model.listing_pending);
    assert!(app.model.load_error.is_none());

    let _ = app.update(Msg::DirLoaded {
        path: home,
        generation: boot_generation,
        result: Ok(Vec::new()),
    });
    let generation = app.model.load_generation;
    let _ = app.update(Msg::ListingSlow { generation });
    assert!(app.model.listing_pending && app.model.listing_slow);

    let _ = app.update(Msg::DirLoaded {
        path: directory,
        generation,
        result: Ok(Vec::new()),
    });
    assert!(!app.model.listing_pending && !app.model.listing_slow);
    assert!(app.model.entries.is_empty());
    assert!(app.model.load_error.is_none());
}

#[test]
fn a_new_mount_supersedes_the_empty_listing_of_its_mountpoint() {
    let scratch = tempfile::tempdir().unwrap();
    let directory = scratch.path().to_path_buf();
    let (mut app, _boot) =
        App::new_with_start_directory(PersistenceRoots::disarmed(), false, Some(directory.clone()));
    let unmounted_generation = app.model.load_generation;
    let mut location = crate::storage::capture_fixture(&app.model.home).remove(0);
    location.mount = Some(directory.clone());
    location.kind = crate::storage::Kind::Network;
    let _ = app.update(Msg::StorageLoaded(Ok(crate::storage::Inventory {
        locations: vec![location],
        cloud: Vec::new(),
    })));
    let _ = app.update(Msg::DirLoaded {
        path: directory.clone(),
        generation: unmounted_generation,
        result: Ok(Vec::new()),
    });
    assert!(app.model.listing_pending);
    assert!(app.model.load_error.is_none());

    let entry = directory.join("Notes.txt");
    std::fs::write(&entry, "ready").unwrap();
    let _ = app.update(Msg::DirLoaded {
        path: directory.clone(),
        generation: app.model.load_generation,
        result: services::read_directory(&directory).map_err(|error| error.to_string()),
    });
    assert!(!app.model.listing_pending);
    assert_eq!(app.model.visible_provider.len(), 1);
    assert_eq!(app.model.entries[0].path, entry);
}
