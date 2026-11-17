use super::*;
use std::fs;
use std::io::Write;
use std::path::Path;

fn manifest(id: &str, version: &str) -> Manifest {
    let mut manifest =
        Manifest::parse(include_bytes!("../../extensions/bundled/checksums.json")).unwrap();
    manifest.id = id.into();
    manifest.version = version.into();
    manifest
}
fn package(path: &Path, manifest: &Manifest, extra: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(&serde_json::to_vec(manifest).unwrap())
        .unwrap();
    for (name, bytes) in extra {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn install_enable_update_remove_are_durable_and_updates_require_review() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("store");
    let source = temp.path().join("sample.enzfiles");
    let mut m = manifest("org.example.checksum", "1.0.0");
    m.settings.push(manifest::Setting {
        id: "note".into(),
        label: "Note".into(),
        value: SettingKind::Text {
            default: "hello".into(),
        },
    });
    package(&source, &m, &[]);
    let prepared = Prepared::open(&source).unwrap();
    let store = Store::open(root.clone()).unwrap();
    store.install(&prepared).unwrap();
    assert!(
        !store::load(root.clone())
            .entries
            .iter()
            .find(|e| e.id == m.id)
            .unwrap()
            .active()
    );
    store.enable(&m.id, true).unwrap();
    store
        .settings(
            &m.id,
            BTreeMap::from([("note".into(), Value::String("saved".into()))]),
        )
        .unwrap();
    assert!(
        store::load(root.clone())
            .entries
            .iter()
            .find(|e| e.id == m.id)
            .unwrap()
            .active()
    );
    assert!(store.install(&prepared).is_err());
    m.version = "1.1.0".into();
    package(&source, &m, &[]);
    store.install(&Prepared::open(&source).unwrap()).unwrap();
    let loaded = store::load(root.clone());
    let entry = loaded.entries.iter().find(|e| e.id == m.id).unwrap();
    assert!(!entry.active());
    assert_eq!(entry.settings()["note"], "saved");
    store.remove(&m.id).unwrap();
    assert!(!root.join(&m.id).exists());
}

#[test]
fn package_paths_duplicates_links_missing_workers_and_size_bombs_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("bad.enzfiles");
    let mut m = manifest("org.example.bad", "1.0.0");
    for name in [
        "../escape",
        "/absolute",
        "state.json",
        ".hidden",
        "bin/../../escape",
        "bin\\worker",
    ] {
        package(&source, &m, &[(name, b"x")]);
        assert!(Prepared::open(&source).is_err(), "{name}");
    }
    m.worker = Worker::Native {
        program: "bin/worker".into(),
        target: "linux-x86_64".into(),
    };
    package(&source, &m, &[]);
    assert!(Prepared::open(&source).is_err());
    let mut zip = zip::ZipWriter::new(fs::File::create(&source).unwrap());
    zip.add_symlink(
        "bin/worker",
        "/bin/sh",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    assert!(Prepared::open(&source).is_err());
}

#[test]
fn worker_responses_cannot_request_undeclared_host_effects() {
    let response = Response {
        api: 1,
        clipboard: Some("hello".into()),
        ..Response::default()
    };
    assert!(runtime::validate(&response, &[]).is_err());
    assert!(runtime::validate(&response, &[Permission::Clipboard]).is_ok());
    let response = Response {
        api: 1,
        navigate: Some(b"relative".to_vec()),
        ..Response::default()
    };
    assert!(runtime::validate(&response, &[Permission::Navigate]).is_err());
    let response = Response {
        api: 99,
        ..Response::default()
    };
    assert!(runtime::validate(&response, &[]).is_err());
}

fn native(temp: &Path, script: &[u8]) -> (Installed, PathBuf) {
    let mut m = manifest("org.example.worker", "1.0.0");
    m.permissions.clear();
    m.worker = Worker::Native {
        program: "bin/worker".into(),
        target: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
    };
    let package_path = temp.join("native.enzfiles");
    package(&package_path, &m, &[("bin/worker", script)]);
    let root = temp.join("store");
    let store = Store::open(root.clone()).unwrap();
    store
        .install(&Prepared::open(&package_path).unwrap())
        .unwrap();
    store.enable(&m.id, true).unwrap();
    let entry = load(root.clone())
        .entries
        .into_iter()
        .find(|e| e.id == m.id)
        .unwrap();
    (entry, root)
}

#[test]
fn cancellation_stops_and_reaps_an_extension_worker() {
    let temp = tempfile::tempdir().unwrap();
    let (entry, root) = native(temp.path(), b"#!/bin/sh\ncat >/dev/null\nexec sleep 60\n");
    let request = runtime::request(
        &entry,
        &entry.manifest.as_ref().unwrap().actions[0],
        temp.path(),
        Vec::new(),
    );
    let cancel = enzui_core::cancellation::CancelToken::new();
    let trigger = cancel.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        trigger.cancel(enzui_core::cancellation::CancelCause::Cancelled);
    });
    let start = std::time::Instant::now();
    let result = run(entry, root, request, &cancel);
    thread.join().unwrap();
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_cancelled_action_cannot_apply_a_success_queued_before_cancellation() {
    let (mut app, _) = crate::App::new();
    let token = enzui_core::cancellation::CancelToken::new();
    token.cancel(enzui_core::cancellation::CancelCause::Cancelled);
    app.extension_cancel = Some(token);
    let response = Response {
        clipboard: Some("must not copy".into()),
        ..Response::default()
    };
    let task = app.reduce_extensions(crate::Msg::ExtensionFinished(Ok(response.clone())));
    assert!(iced_runtime::task::into_stream(task).is_none());
    app.extension_cancel = Some(enzui_core::cancellation::CancelToken::new());
    let task = app.reduce_extensions(crate::Msg::ExtensionFinished(Ok(response)));
    assert!(iced_runtime::task::into_stream(task).is_some());
}
