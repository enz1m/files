// Bulk-rename tests driven through `App::update` against a real temp directory.
//
// `bulk_rename`'s unit tests would pass with the reducer unwired, so these
// read the filesystem after each dispatch.

use super::*;
use std::fs;

fn apply_and_capture_rename(app: &mut App) -> activity::FileActivity {
    let _ = app.update(Msg::ApplyBulkRename);
    app.activity_center
        .iter()
        .find(|row| row.key.provider == activity::FILES && row.kind == ActivityKind::Rename)
        .map(|row| activity::FileActivity {
            key: row.key,
            generation: row.generation,
        })
        .expect("ApplyBulkRename publishes a Rename activity row")
}

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "files-app-rename-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        TmpDir(base)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn app_with_files(cwd: &Path, names: &[&str]) -> App {
    for n in names {
        fs::write(cwd.join(n), n.as_bytes()).unwrap();
    }
    let (mut app, _t) = App::new();
    app.model.cwd = cwd.to_path_buf();
    app.model.screen = Screen::Directory;
    app.model.sort_folders_first = false;
    app.model
        .set_entries(services::read_directory(cwd).unwrap());
    app
}

fn on_disk(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn select(app: &mut App, names: &[&str]) {
    let cwd = app.model.cwd.clone();
    for n in names {
        let _ = app.update(Msg::ToggleSelect(cwd.join(n)));
    }
}

#[test]
fn preview_and_apply_agree_on_a_fixture() {
    let tmp = TmpDir::new("agree");
    let mut app = app_with_files(tmp.path(), &["IMG_1.jpg", "IMG_2.jpg", "notes.txt"]);
    select(&mut app, &["IMG_1.jpg", "IMG_2.jpg", "notes.txt"]);

    let _ = app.update(Msg::BeginBulkRename);
    let _ = app.update(Msg::BulkRenameFindChanged("IMG_".into()));
    let _ = app.update(Msg::BulkRenameReplaceChanged("Photo_".into()));

    let state = app.model.bulk_rename.as_ref().expect("the dialog is open");
    let plan = state.plan();
    let previewed_originals: Vec<String> =
        plan.rows.iter().map(|r| r.original_name.clone()).collect();
    let previewed_new: Vec<String> = plan.rows.iter().map(|r| r.new_name.clone()).collect();
    assert_eq!(
        previewed_originals,
        vec!["IMG_1.jpg", "IMG_2.jpg", "notes.txt"],
        "the Original: column is the current names"
    );
    assert_eq!(
        previewed_new,
        vec!["Photo_1.jpg", "Photo_2.jpg", "notes.txt"],
        "the New: column, including the unchanged row"
    );

    let ops = app
        .planned_rename_operations()
        .expect("a valid plan must be applyable");
    let activity = apply_and_capture_rename(&mut app);
    let _ = app.update(Msg::DialogEvent(
        enzui_widgets::modal_host::ModalHostEvent::Dismissed(
            enzui_core::modal::DismissCause::Escape,
        ),
    ));
    assert!(app.model.bulk_rename_busy && app.model.bulk_rename.is_some());
    let outcome = services::apply_bulk_rename(&ops);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(outcome.renamed.len(), 2, "only matching files are renamed");
    let _ = app.update(Msg::BulkRenameFinished { activity, outcome });

    let mut expected = previewed_new;
    expected.sort();
    assert_eq!(
        on_disk(tmp.path()),
        expected,
        "the bytes on disk must be exactly what the preview promised"
    );
    assert!(
        app.model.bulk_rename.is_none(),
        "a finished rename closes the dialog"
    );
    for row in plan.rows {
        assert_eq!(
            fs::read(tmp.path().join(&row.new_name)).unwrap(),
            row.original_name.as_bytes(),
            "renaming must preserve each source's bytes"
        );
    }
}

#[test]
fn a_colliding_batch_is_refused_and_the_disk_is_untouched() {
    let tmp = TmpDir::new("collide");
    let mut app = app_with_files(tmp.path(), &["a.txt", "b.txt"]);
    select(&mut app, &["a.txt", "b.txt"]);
    let before = on_disk(tmp.path());

    let _ = app.update(Msg::BeginBulkRename);
    let _ = app.update(Msg::SetBulkRenameMode(
        bulk_rename::BulkRenameMode::Numbering,
    ));
    // A template with no [N] gives both files one name.
    let _ = app.update(Msg::BulkRenameTemplateChanged("same".into()));

    let state = app.model.bulk_rename.as_ref().unwrap();
    assert!(state.plan().is_blocked(), "the fixture must be blocked");
    assert!(!state.can_apply());
    assert!(
        app.planned_rename_operations().is_none(),
        "a blocked plan must produce NO operations"
    );

    let _ = app.update(Msg::ApplyBulkRename);
    assert!(
        !app.model.bulk_rename_busy,
        "a refused batch must not start; busy is the observable tell"
    );
    assert_eq!(
        on_disk(tmp.path()),
        before,
        "a refused batch must leave the filesystem byte-for-byte unchanged"
    );
    assert!(
        app.model.bulk_rename.is_some(),
        "and the dialog stays open so the user can fix the rule"
    );
}

#[test]
fn a_rename_permutation_survives_the_two_phase_pass() {
    let tmp = TmpDir::new("swap");
    fs::write(tmp.path().join("a"), b"A").unwrap();
    fs::write(tmp.path().join("b"), b"B").unwrap();
    let ops = vec![
        (tmp.path().join("a"), tmp.path().join("b")),
        (tmp.path().join("b"), tmp.path().join("a")),
    ];
    let outcome = services::apply_bulk_rename(&ops);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(on_disk(tmp.path()), vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(
        fs::read(tmp.path().join("a")).unwrap(),
        b"B".to_vec(),
        "the contents really swapped"
    );
    assert_eq!(fs::read(tmp.path().join("b")).unwrap(), b"A".to_vec());
}
