// Filesystem-effect and failure-path tests for `Msg::NewFolder` and
// `Msg::NewFile`.
//
// A reducer arm that only sets `status_message` would pass a message-oriented
// test, so each test reads the real temp directory.
use super::*;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "files-app-1181-create-{}-{}-{}",
        tag,
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn app_at(cwd: &Path) -> App {
    let (app, _t) = App::new();
    let mut app = app;
    app.model.cwd = cwd.to_path_buf();
    app.model.screen = Screen::Directory;
    app.model
        .set_entries(services::read_directory(cwd).unwrap());
    app
}

#[test]
fn links_land_in_the_chosen_folder_without_changing_the_source() {
    let dir = tmp("link-destination");
    let source = dir.join("source");
    let destination = dir.join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    let file = source.join("sample.bin");
    // Binary content: a NUL keeps it from sniffing as text (and previewable).
    fs::write(&file, b"\0original").unwrap();
    let mut app = app_at(&source);
    app.model.select_only(file.clone());
    let menu = crate::context_menu::menu_model(&app.model, &file, false);
    assert!(!menu.iter().any(|item| matches!(
        item,
        enzui_widgets::context_menu_host::ContextMenuItem::Command {
            command: crate::context_menu::Command::Row(crate::context_menu::RowKind::OpenPreview),
            ..
        }
    )));
    crate::file_clipboard_tests::dispatch(
        &mut app,
        Msg::LinkSources {
            sources: vec![file.clone()],
            dest_dir: destination.clone(),
        },
    );
    let link = fs::read_dir(&destination)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read_link(link).unwrap(), file);
    assert_eq!(fs::read(&file).unwrap(), b"\0original");
    assert_eq!(fs::read_dir(&source).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}
