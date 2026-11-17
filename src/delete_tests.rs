// Filesystem effects, failure paths and the confirmation gate for
// `Msg::TrashSelection`, `Msg::AskPermanentDelete` and their aftermath.
//
// The reducer's `trash_selection` task trashes into `$XDG_DATA_HOME`, so
// filesystem assertions call `trash_batch_with` / `move_to_trash_with` with a
// temp home, and the reducer routing to one `Msg::TrashFinished` is pinned
// separately.
use super::*;
use crate::trash::move_to_trash_with;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "files-app-1181-del-{}-{}-{}",
        tag,
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    base
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

fn select(app: &mut App, paths: &[PathBuf]) {
    for p in paths {
        let _ = app.update(Msg::ToggleSelect(p.clone()));
    }
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

fn dispatch(app: &mut App, first: Msg) {
    let mut pending = vec![first];
    let mut steps = 0;
    while let Some(message) = pending.pop() {
        steps += 1;
        assert!(steps < 32, "task chain did not settle");
        pending.extend(drive(app.update(message)));
    }
}

fn app_on_trash(home_data: &Path) -> App {
    let (mut app, _task) = App::new();
    app.home_data_dir = home_data.to_path_buf();
    app.model.screen = Screen::Trash;
    app.model.cwd = PathBuf::from(services::TRASH_VIEW_ROOT);
    app.model.trash_entries = trash::list_trash(home_data);
    app
}

#[test]
fn cancelling_the_permanent_delete_leaves_the_disk_untouched() {
    use enzui_core::modal::DismissCause;
    use enzui_widgets::confirm_dialog::ConfirmDialogEvent;
    let dir = tmp("perm-cancel");
    let src = dir.join("safe.txt");
    fs::write(&src, b"safe").unwrap();
    let mut app = app_at(&dir);
    select(&mut app, std::slice::from_ref(&src));
    let _ = app.update(Msg::AskPermanentDelete);

    let _ = app.update(Msg::DeleteConfirmEvent(ConfirmDialogEvent::Resolved(
        DismissCause::CancelAction,
    )));
    assert!(
        app.model.pending_permanent_delete.is_none(),
        "the confirmation must have closed"
    );
    assert!(src.exists(), "a cancel must NOT delete the file");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn occupied_restore_uses_the_existing_conflict_queue_and_replace_policy() {
    let root = tmp("restore-conflict");
    let home_data = root.join("data");
    fs::create_dir_all(&home_data).unwrap();
    let origin = root.join("report.txt");
    fs::write(&origin, b"trashed version").unwrap();
    let trashed = move_to_trash_with(&origin, &home_data).unwrap();
    fs::write(&origin, b"new occupant").unwrap();
    let mut app = app_on_trash(&home_data);

    dispatch(&mut app, Msg::RestoreFromTrash(trashed));
    assert!(
        app.model.conflicts.is_some(),
        "occupied restore must pause in ConflictQueue"
    );
    assert_eq!(fs::read(&origin).unwrap(), b"new occupant");

    dispatch(
        &mut app,
        Msg::ResolveConflict(conflict::ConflictChoice::Replace),
    );
    assert_eq!(fs::read(&origin).unwrap(), b"trashed version");
    assert!(app.model.conflicts.is_none());
    assert!(app.model.trash_entries.is_empty());
    assert!(trash::list_trash(&home_data).is_empty());
}

#[test]
fn missing_original_directory_is_refused_while_an_ordinary_restore_works() {
    let root = tmp("restore-missing-parent");
    let home_data = root.join("data");
    let gone = root.join("gone");
    let kept = root.join("kept");
    fs::create_dir_all(&home_data).unwrap();
    fs::create_dir_all(&gone).unwrap();
    fs::create_dir_all(&kept).unwrap();
    let missing_origin = gone.join("missing.txt");
    let normal_origin = kept.join("normal.txt");
    fs::write(&missing_origin, b"missing-parent payload").unwrap();
    fs::write(&normal_origin, b"ordinary payload").unwrap();
    let missing_trash = move_to_trash_with(&missing_origin, &home_data).unwrap();
    let normal_trash = move_to_trash_with(&normal_origin, &home_data).unwrap();
    fs::remove_dir(&gone).unwrap();
    let mut app = app_on_trash(&home_data);

    dispatch(&mut app, Msg::RestoreFromTrash(missing_trash.clone()));
    assert!(
        !gone.exists(),
        "restore must not recreate the missing folder"
    );
    assert!(missing_trash.exists(), "the refused payload stays in Trash");
    assert!(
        app.model
            .status_message
            .as_deref()
            .is_some_and(|line| line.contains("original folder no longer exists")),
        "the refusal must be visible: {:?}",
        app.model.status_message
    );

    dispatch(&mut app, Msg::RestoreFromTrash(normal_trash));
    assert_eq!(fs::read(&normal_origin).unwrap(), b"ordinary payload");
    assert_eq!(trash::list_trash(&home_data).len(), 1);
}
