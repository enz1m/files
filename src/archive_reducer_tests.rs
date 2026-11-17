// Archive verbs through the real context-menu rows, reducer arms,
// bounded owner and Activity Center.

use super::*;
use crate::model::EntryKind;
use std::fs;
use std::path::PathBuf;

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "files-app-2956-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn app_at(cwd: &Path) -> App {
    let (mut app, _t) = App::new();
    app.model.cwd = cwd.to_path_buf();
    app.model.screen = Screen::Directory;
    app.model
        .set_entries(services::read_directory(cwd).unwrap());
    app
}

fn drive(task: Task<Msg>) -> Vec<Msg> {
    match iced_runtime::task::into_stream(task) {
        None => Vec::new(),
        Some(stream) => iced::futures::executor::block_on(async move {
            use iced::futures::StreamExt;
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(m) => Some(m),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .await
        }),
    }
}

#[test]
fn archive_activation_honors_an_external_default_without_extracting() {
    let dir = tmp("external-default");
    let archive = dir.join("Bundle.7z");
    fs::write(
        &archive,
        include_bytes!("../tests/fixtures/archives/sample.7z"),
    )
    .unwrap();
    let mut app = app_at(&dir);
    app.open_route = crate::portal_open::OpenRoute::DesktopLauncher;
    app.mime_query = std::sync::Arc::new(crate::mime_query::RecordingMimeQuery::new(
        Ok(Some("org.gnome.FileRoller.desktop".into())),
        Ok(Vec::new()),
    ));
    let launcher = std::sync::Arc::new(crate::launch::RecordingLauncher::succeeding());
    app.launcher = std::sync::Arc::<crate::launch::RecordingLauncher>::clone(&launcher);
    let query = app.update(Msg::OpenEntry(archive.clone(), EntryKind::File));
    for message in drive(query) {
        let launch = app.update(message);
        for message in drive(launch) {
            let _ = app.update(message);
        }
    }
    assert_eq!(launcher.calls().len(), 1);
    assert_eq!(launcher.calls()[0].path, archive);
    assert!(!dir.join("Bundle").exists());
    assert!(
        app.activity_center
            .iter()
            .all(|row| row.kind != ActivityKind::Other("Extract"))
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sandbox_archive_activation_uses_the_host_portal_instead_of_container_defaults() {
    let root = tempfile::tempdir().unwrap();
    let archive = root.path().join("Bundle.7z");
    fs::write(
        &archive,
        include_bytes!("../tests/fixtures/archives/sample.7z"),
    )
    .unwrap();
    let mut app = app_at(root.path());
    app.open_route = crate::portal_open::OpenRoute::Portal;
    app.file_opener = crate::portal_open::PortalFileOpener::with_backend(Box::new(
        enzui_core::portal::open_uri::MockOpenUriBackend::new(),
    ));
    let task = app.update(Msg::OpenEntry(archive, EntryKind::File));
    assert!(drive(task).is_empty());
    assert_eq!(app.file_opener.pending_count(), 1);
    assert!(!root.path().join("Bundle").exists());
}
