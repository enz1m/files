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
