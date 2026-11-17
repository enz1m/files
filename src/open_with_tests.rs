use super::*;
use crate::mime_query::{DesktopApplication, RecordingMimeQuery};
use crate::open_with::OpenBehavior;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

fn file(name: &str, mime: &str) -> model::FileEntry {
    model::FileEntry {
        path: PathBuf::from(format!("/base/{name}")),
        name: name.to_owned(),
        kind: model::EntryKind::File,
        size: 1,
        modified: None,
        is_hidden: false,
        mime: mime.to_owned(),
    }
}

fn desktop(id: &str, name: &str) -> DesktopApplication {
    DesktopApplication::new(id, name, PathBuf::from("/xdg/applications").join(id))
}

fn drive(task: Task<Msg>) -> Vec<Msg> {
    match iced_runtime::task::into_stream(task) {
        None => Vec::new(),
        Some(stream) => iced::futures::executor::block_on(async move {
            use iced::futures::StreamExt;
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(message) => Some(message),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .await
        }),
    }
}

fn open_and_finish(app: &mut App) {
    let task = app.update(Msg::ShowOpenWith(PathBuf::from("/base/notes.txt")));
    let messages = drive(task);
    assert_eq!(messages.len(), 1, "the MIME query task emits one outcome");
    for message in messages {
        let _ = app.update(message);
    }
}

fn installed_handler_argv(desktop_file: PathBuf, target: PathBuf) -> Vec<OsString> {
    use crate::launch::RecordingLauncher;

    let query = Arc::new(RecordingMimeQuery::new(
        Ok(Some("editor.desktop".to_owned())),
        Ok(vec![DesktopApplication::new(
            "editor.desktop",
            "Editor",
            desktop_file,
        )]),
    ));
    let launcher = Arc::new(RecordingLauncher::succeeding());
    let (mut app, _startup) = App::new();
    app = app.with_mime_query(query);
    app = app.with_launcher(std::sync::Arc::<crate::launch::RecordingLauncher>::clone(
        &launcher,
    ));
    app.model.cwd = PathBuf::from("/base");
    app.model.screen = Screen::Directory;
    app.model.set_entries(vec![model::FileEntry {
        path: target.clone(),
        name: "target.txt".to_owned(),
        kind: model::EntryKind::File,
        size: 1,
        modified: None,
        is_hidden: false,
        mime: "text/plain".to_owned(),
    }]);

    let query_messages = drive(app.update(Msg::ShowOpenWith(target)));
    assert_eq!(query_messages.len(), 1, "the MIME query must complete");
    for message in query_messages {
        let _ = app.update(message);
    }
    let _ = app.update(Msg::SelectOpenCandidate(0));
    for message in drive(app.update(Msg::ConfirmOpenWith)) {
        let _ = app.update(message);
    }

    let calls = launcher.calls();
    assert_eq!(calls.len(), 1, "the selected handler launches once");
    calls[0]
        .argv()
        .expect("an installed handler uses the command-launch route")
}

#[cfg(unix)]
#[test]
fn installed_handler_route_preserves_non_utf8_path_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let desktop_file = PathBuf::from(OsString::from_vec(
        b"/fixture/User\xffData/applications/editor.desktop".to_vec(),
    ));
    let target = PathBuf::from(OsString::from_vec(b"/base/target-\xfe.txt".to_vec()));

    assert_eq!(
        installed_handler_argv(desktop_file.clone(), target.clone()),
        vec![
            OsString::from("gio"),
            OsString::from("launch"),
            desktop_file.into_os_string(),
            target.into_os_string(),
        ]
    );
}

#[test]
fn always_this_app_persists_and_the_next_open_uses_it() {
    use crate::launch::RecordingLauncher;
    let query = Arc::new(RecordingMimeQuery::new(
        Ok(Some("beta.desktop".to_owned())),
        Ok(vec![
            desktop("alpha.desktop", "Alpha"),
            desktop("beta.desktop", "Beta"),
        ]),
    ));
    let launcher = Arc::new(RecordingLauncher::succeeding());
    let (mut app, _startup) = App::new();
    app =
        app.with_mime_query(std::sync::Arc::<crate::mime_query::RecordingMimeQuery>::clone(&query));
    app = app.with_launcher(std::sync::Arc::<crate::launch::RecordingLauncher>::clone(
        &launcher,
    ));
    app.model.cwd = PathBuf::from("/base");
    app.model.screen = Screen::Directory;
    app.model.set_entries(vec![
        file("notes.txt", "text/plain"),
        file("memo.txt", "text/plain"),
    ]);

    open_and_finish(&mut app);
    assert_eq!(app.model.open_with.as_ref().unwrap().selected, Some(0));
    let expected_cmd = app
        .model
        .open_with
        .as_ref()
        .unwrap()
        .effective_command()
        .unwrap();
    assert_eq!(
        expected_cmd.args(),
        &[
            std::ffi::OsString::from("launch"),
            std::ffi::OsString::from("/xdg/applications/beta.desktop"),
        ],
        "the chosen launch is Beta's"
    );

    let _ = app.update(Msg::SetOpenBehavior(OpenBehavior::Always));
    for m in drive(app.update(Msg::ConfirmOpenWith)) {
        let _ = app.update(m);
    }

    // Cached for the session, so the reducer needs no subprocess.
    assert_eq!(
        app.model.mime_defaults.get("text/plain").cloned(),
        Some(expected_cmd.clone()),
        "Always must cache the chosen command per-MIME"
    );
    // The durable write reached the seam and round-trips on read.
    assert!(
        query
            .calls()
            .iter()
            .any(|(op, mime)| op == "set_default" && mime == "text/plain"),
        "the durable freedesktop write must reach the seam"
    );
    assert_eq!(
        query.written_default("text/plain").as_deref(),
        Some("beta.desktop"),
        "the seam must have stored the chosen desktop id"
    );

    let before = launcher.calls().len();
    for m in drive(app.update(Msg::OpenEntry(
        PathBuf::from("/base/memo.txt"),
        model::EntryKind::File,
    ))) {
        let _ = app.update(m);
    }
    let calls = launcher.calls();
    assert_eq!(
        calls.len(),
        before + 1,
        "exactly one launch for the second open"
    );
    let last = calls.last().unwrap();
    assert_eq!(last.path, PathBuf::from("/base/memo.txt"));
    assert_eq!(
        last.command.as_ref(),
        Some(&expected_cmd),
        "the second open must use the persisted app, not the generic \
         handler (which passes command = None)"
    );
}
