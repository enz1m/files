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

#[test]
fn archive_activity_reports_bytes_without_exposing_file_names() {
    use crate::archive::{ArchiveFormat, ArchiveResult};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, ErrorKind, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::{Duration, Instant};

    fn exchange(stream: &mut BufReader<UnixStream>, reply: Value) -> Value {
        let mut line = String::new();
        stream.read_line(&mut line).expect("read publisher message");
        let value = serde_json::from_str(&line).expect("publisher sends JSON");
        writeln!(stream.get_mut(), "{reply}").expect("acknowledge publisher");
        value
    }

    let root = tempfile::tempdir().expect("isolated archive directory");
    let source = root.path().join("private-note.txt");
    let archive = root.path().join("private-bundle.7z");
    fs::write(&source, "content").expect("compression fixture");
    fs::write(
        &archive,
        include_bytes!("../tests/fixtures/archives/sample.7z"),
    )
    .expect("extraction fixture");
    let mut app = app_at(root.path());
    let socket = root.path().join("activity.sock");
    let listener = UnixListener::bind(&socket).expect("bind private broker");
    listener.set_nonblocking(true).expect("bound accept wait");
    let publisher =
        enzui_activity::Publisher::at_socket("io.github.enzui.Files", socket).expect("publisher");
    let token = "enzui-archive-test";
    app.shell_activity =
        crate::shell_activity::ShellActivity::with_test_publisher(publisher, token.to_owned());
    app.model.select_only(source);
    let english = crate::i18n::for_locale("en-US".parse().expect("English locale"));
    let operations = english.scope(|| {
        [
            app.update(Msg::CompressSelection(ArchiveFormat::Zip)),
            app.update(Msg::ExtractArchive {
                archive,
                dest_dir: None,
            }),
        ]
    });

    let deadline = Instant::now() + Duration::from_secs(4);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("waiting for Files activity publisher: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .expect("bound message wait");
    let mut stream = BufReader::new(stream);
    let hello = exchange(
        &mut stream,
        json!({
            "op": "ready", "version": 1, "lease_ms": 90000,
            "heartbeat_ms": 30000, "rate_per_second": 4, "burst": 8,
        }),
    );
    assert_eq!(hello["app_id"], "io.github.enzui.Files");
    let mut jobs = BTreeMap::new();
    while jobs.len() < 2 {
        let publish = exchange(&mut stream, json!({"op":"ack","revision":1,"changed":true}));
        assert_eq!(publish["op"], "batch");
        for work in publish["works"].as_array().expect("published work list") {
            assert_eq!(work["state"], "running");
            assert_eq!(work["window_token"], token);
            assert_eq!(work["detail"], "");
            assert!(work["progress"].is_null());
            let job = work["job"].as_str().expect("opaque job ID").to_owned();
            let label = work["label"].as_str().expect("generic label").to_owned();
            assert!(jobs.insert(job, label).is_none());
        }
    }
    assert_eq!(
        jobs.values()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["Compressing files", "Extracting archive"]
            .into_iter()
            .collect(),
    );

    let _ = app.update(Msg::ArchiveProgress);
    assert!(app.activity_center.iter().any(|row| {
        row.kind == ActivityKind::Other("Compress")
            && matches!(
                row.progress,
                enzui_core::operation_activity::ActivityProgress::Bytes {
                    done: 6,
                    total: Some(7),
                }
            )
    }));
    let mut compression_progress = false;
    while !compression_progress {
        let publish = exchange(&mut stream, json!({"op":"ack","revision":2,"changed":true}));
        assert_eq!(publish["op"], "batch");
        for work in publish["works"].as_array().expect("updated work list") {
            assert_eq!(work["detail"], "");
            if work["label"] == "Compressing files" {
                assert_eq!(work["progress"], json!({"done":6,"total":7,"unit":"bytes"}));
                compression_progress = true;
            }
        }
    }

    for operation in operations {
        let mut messages = drive(operation);
        assert_eq!(messages.len(), 1);
        let settled = messages.pop().expect("archive settled");
        assert!(matches!(
            settled,
            Msg::ArchiveSettled {
                result: ArchiveResult::Published { .. },
                ..
            }
        ));
        let _ = app.update(settled);
    }
    while !jobs.is_empty() {
        let finish = exchange(&mut stream, json!({"op":"ack","revision":2,"changed":true}));
        if finish["op"] == "batch" {
            continue;
        }
        assert_eq!(finish["op"], "finish");
        assert_eq!(finish["outcome"], "succeeded");
        assert!(
            jobs.remove(finish["job"].as_str().expect("finished job ID"))
                .is_some()
        );
    }
}
