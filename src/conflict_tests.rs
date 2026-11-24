// Conflict-resolution tests driven through `App::update` against real temp
// directories, asserting on the filesystem.
//
// `conflict`'s unit tests use a recording closure and would pass with
// `services::perform_conflict_choice` unwired, `Merge` routed to
// `move_replacing`, or Paste never dispatched.

use super::*;
use conflict::{ClipMode, ConflictChoice, ConflictKind};
use iced::futures::executor::block_on;
use std::fs;

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "files-app-conflict-test-{}-{}-{}",
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

// `src/` and `dst/` under one root.
struct Fixture {
    _tmp: TmpDir,
    src: PathBuf,
    dst: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let tmp = TmpDir::new(tag);
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        Fixture {
            _tmp: tmp,
            src,
            dst,
        }
    }
    fn write_src(&self, name: &str, body: &str) -> PathBuf {
        let p = self.src.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }
    fn write_dst(&self, name: &str, body: &str) -> PathBuf {
        let p = self.dst.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }
    fn app(&self) -> App {
        let (mut app, _t) = App::new();
        app.model.cwd = self.dst.clone();
        app.model.screen = Screen::Directory;
        app
    }
    fn paste(&self, app: &mut App, sources: &[PathBuf], copy: bool) {
        let mode = if copy { ClipMode::Copy } else { ClipMode::Cut };
        let plan = block_on(crate::plan_paste(sources.to_vec(), self.dst.clone(), copy));
        let empty = plan.is_empty();
        // An empty plan short-circuits in `PastePlanned` and never spawns
        // `open_paste_queue`.
        let activity = app.start_file_activity(
            if copy {
                enzui_core::operation_activity::ActivityKind::Copy
            } else {
                enzui_core::operation_activity::ActivityKind::Move
            },
            "Test paste",
            sources.len(),
        );
        let _ = app.update(Msg::PastePlanned {
            activity,
            dest_dir: self.dst.clone(),
            copy,
            plan: Box::new(plan.clone()),
        });
        if !empty {
            let queue = block_on(crate::open_paste_queue(
                self.dst.clone(),
                mode,
                plan,
                None,
                |_| {},
            ));
            let _ = app.update(Msg::PasteExecuted { activity, queue });
        }
    }
    fn resolve(&self, app: &mut App, choice: ConflictChoice) {
        let queue = app
            .model
            .conflicts
            .clone()
            .expect("a conflict must be on screen");
        assert!(!app.model.conflicts_busy);
        let q = block_on(crate::resolve_conflict(queue, choice, None));
        let activity = app
            .conflict_activity
            .expect("an open conflict carries its activity identity");
        let _ = app.update(Msg::ConflictBatchFinished { activity, queue: q });
    }

    fn current(&self, app: &App) -> conflict::Conflict {
        app.model
            .conflicts
            .as_ref()
            .and_then(|q| q.current.clone())
            .expect("a conflict must be on screen")
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn merge_unions_the_directory_and_replace_would_not() {
    let fx = Fixture::new("merge");
    fx.write_src("d/bring", "BRING");
    fx.write_src("d/shared", "SRC-SHARED");
    fx.write_dst("d/keep", "KEEP");
    fx.write_dst("d/shared", "DST-SHARED");
    let sources = vec![fx.src.join("d")];

    let mut app = fx.app();
    fx.paste(&mut app, &sources, false);

    let q = app.model.conflicts.as_ref().expect("folder vs folder asks");
    assert_eq!(
        q.current.as_ref().unwrap().kind,
        ConflictKind::FolderVsFolder
    );
    assert_eq!(
        q.primary_label(),
        "Merge",
        "two directories offer Merge, not Replace"
    );

    fx.resolve(&mut app, ConflictChoice::Merge);

    let nested = fx.current(&app);
    assert_eq!(nested.dst, fx.dst.join("d/shared"));
    assert_eq!(nested.kind, ConflictKind::FileVsFile);
    assert_eq!(
        fs::read_to_string(fx.dst.join("d/shared")).unwrap(),
        "DST-SHARED",
        "a nested collision waits for its own decision"
    );
    assert_eq!(fs::read_to_string(fx.dst.join("d/bring")).unwrap(), "BRING");

    fx.resolve(&mut app, ConflictChoice::Replace);

    assert!(app.model.conflicts.is_none());
    assert!(
        !fx.src.join("d").exists(),
        "the emptied Cut source folder is removed"
    );
    assert_eq!(
        names(&fx.dst.join("d")),
        vec!["bring".to_owned(), "keep".to_owned(), "shared".to_owned()],
        "a merge UNIONS: `keep` must survive"
    );
    assert_eq!(
        fs::read_to_string(fx.dst.join("d/keep")).unwrap(),
        "KEEP",
        "an untouched child keeps its bytes"
    );
    assert_eq!(fs::read_to_string(fx.dst.join("d/bring")).unwrap(), "BRING");
    assert_eq!(
        fs::read_to_string(fx.dst.join("d/shared")).unwrap(),
        "SRC-SHARED",
        "a colliding child is replaced by the incoming one once chosen"
    );
    assert_eq!(
        app.model
            .status_message
            .as_deref()
            .map(crate::i18n::unisolated)
            .as_deref(),
        Some("1 replaced, 1 merged")
    );
}

#[test]
fn merge_asks_before_swapping_nested_files_and_folders() {
    for copy in [false, true] {
        let fx = Fixture::new(if copy {
            "mismatch-copy"
        } else {
            "mismatch-cut"
        });
        fx.write_src("d/file_over_folder", "SRC-FILE");
        fx.write_dst("d/file_over_folder/kept", "DST-KEPT");
        fx.write_src("d/folder_over_file/inner", "SRC-INNER");
        fx.write_dst("d/folder_over_file", "DST-FILE");
        fx.write_src("d/one.txt", "SRC-ONE");
        fx.write_dst("d/one.txt", "DST-ONE");
        fx.write_src("d/sub/two.txt", "SRC-TWO");
        fx.write_dst("d/sub/two.txt", "DST-TWO");
        let mut app = fx.app();
        fx.paste(&mut app, &[fx.src.join("d")], copy);
        fx.resolve(&mut app, ConflictChoice::Merge);

        let mut asked = Vec::new();
        while app.model.conflicts.is_some() {
            let current = fx.current(&app);
            asked.push(current.kind);
            if current.kind == ConflictKind::FileVsFile {
                let _ = app.update(Msg::SetApplyToAllConflicts(true));
                fx.resolve(&mut app, ConflictChoice::Replace);
            } else {
                fx.resolve(&mut app, ConflictChoice::Skip);
            }
        }

        // Replacing all files is sticky for files only: each swap still asks.
        asked.sort_by_key(|kind| *kind == ConflictKind::FileVsFile);
        assert_eq!(
            asked,
            vec![
                ConflictKind::TypeMismatch,
                ConflictKind::TypeMismatch,
                ConflictKind::FileVsFile
            ]
        );
        assert_eq!(
            fs::read_to_string(fx.dst.join("d/file_over_folder/kept")).unwrap(),
            "DST-KEPT"
        );
        assert_eq!(
            fs::read_to_string(fx.dst.join("d/folder_over_file")).unwrap(),
            "DST-FILE"
        );
        assert_eq!(
            fs::read_to_string(fx.dst.join("d/one.txt")).unwrap(),
            "SRC-ONE"
        );
        assert_eq!(
            fs::read_to_string(fx.dst.join("d/sub/two.txt")).unwrap(),
            "SRC-TWO"
        );
        assert_eq!(
            fs::read_to_string(fx.src.join("d/file_over_folder")).unwrap(),
            "SRC-FILE"
        );
        assert_eq!(
            fs::read_to_string(fx.src.join("d/folder_over_file/inner")).unwrap(),
            "SRC-INNER"
        );
        assert_eq!(fx.src.join("d/sub").exists(), copy);
        assert_eq!(
            app.model
                .status_message
                .as_deref()
                .map(crate::i18n::unisolated)
                .as_deref(),
            Some("2 replaced, 1 merged, 2 skipped")
        );
    }
}

#[test]
fn skip_leaves_both_sides_exactly_as_they_were() {
    let fx = Fixture::new("skip");
    let sources = vec![fx.write_src("a", "SRC")];
    fx.write_dst("a", "DST");

    let mut app = fx.app();
    fx.paste(&mut app, &sources, false);
    assert!(app.model.conflicts.is_some(), "positive control: it asked");

    fx.resolve(&mut app, ConflictChoice::Skip);

    assert_eq!(
        fs::read_to_string(fx.dst.join("a")).unwrap(),
        "DST",
        "the destination must be untouched"
    );
    assert_eq!(
        fs::read_to_string(fx.src.join("a")).unwrap(),
        "SRC",
        "and the source must still be there"
    );
    assert_eq!(
        app.model
            .status_message
            .as_deref()
            .map(crate::i18n::unisolated)
            .as_deref(),
        Some("1 skipped")
    );
}

#[test]
fn cancelling_a_conflict_queue_leaves_the_remaining_items_alone() {
    let fx = Fixture::new("cancel");
    let sources: Vec<PathBuf> = ["a", "b"]
        .iter()
        .map(|n| fx.write_src(n, &format!("SRC-{n}")))
        .collect();
    for n in ["a", "b"] {
        fx.write_dst(n, &format!("DST-{n}"));
    }

    let mut app = fx.app();
    fx.paste(&mut app, &sources, false);
    assert!(app.model.conflicts.is_some());
    let _ = app.update(Msg::DialogEvent(
        enzui_widgets::modal_host::ModalHostEvent::Dismissed(
            enzui_core::modal::DismissCause::Escape,
        ),
    ));
    assert!(app.model.conflicts.is_some());

    let _ = app.update(Msg::CancelConflicts);
    assert!(app.model.conflicts.is_none(), "Cancel closes the dialog");
    assert_eq!(
        app.model
            .status_message
            .as_deref()
            .map(crate::i18n::unisolated)
            .as_deref(),
        Some("Paste cancelled")
    );
    assert_eq!(fs::read_to_string(fx.dst.join("a")).unwrap(), "DST-a");
    assert_eq!(fs::read_to_string(fx.dst.join("b")).unwrap(), "DST-b");
    assert!(fx.src.join("a").exists() && fx.src.join("b").exists());
}

#[test]
fn a_copy_paste_lands_bytes_and_leaves_the_source() {
    let fx = Fixture::new("copy");
    let sources = vec![fx.write_src("free", "F"), fx.write_src("clash", "SRC")];
    fx.write_dst("clash", "DST");

    let mut app = fx.app();
    fx.paste(&mut app, &sources, true);

    assert_eq!(
        fs::read_to_string(fx.dst.join("free")).unwrap(),
        "F",
        "a free copy lands the bytes"
    );
    assert!(fx.src.join("free").exists(), "a copy leaves the source");

    fx.resolve(&mut app, ConflictChoice::Replace);
    assert_eq!(
        fs::read_to_string(fx.dst.join("clash")).unwrap(),
        "SRC",
        "a Replace copy overwrites the destination with the source bytes"
    );
    assert!(
        fx.src.join("clash").exists(),
        "the conflicting source is a copy, so it survives too"
    );
    assert_eq!(names(&fx.src).len(), 2, "both sources are still there");
    assert_eq!(
        app.model
            .status_message
            .as_deref()
            .map(crate::i18n::unisolated)
            .as_deref(),
        Some("1 copied, 1 replaced")
    );
}

#[test]
fn copying_into_the_same_folder_creates_a_numbered_sibling() {
    let fx = Fixture::new("self-copy");
    let report = fx.write_dst("report.txt", "REPORT");
    fs::write(fx.dst.join("report (2).txt"), "OLDER COPY").unwrap();
    let folder = fx.dst.join("Photos");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("one.jpg"), "PHOTO").unwrap();

    let mut app = fx.app();
    fx.paste(&mut app, &[report.clone(), folder.clone()], true);

    assert!(
        app.model.conflicts.is_none(),
        "fresh sibling names never ask"
    );
    assert_eq!(fs::read_to_string(&report).unwrap(), "REPORT");
    assert_eq!(
        fs::read_to_string(fx.dst.join("report (3).txt")).unwrap(),
        "REPORT",
        "the suffix goes before the extension and skips occupied siblings"
    );
    assert_eq!(
        fs::read_to_string(fx.dst.join("Photos (2)/one.jpg")).unwrap(),
        "PHOTO",
        "a same-folder directory copy recursively duplicates the tree"
    );
    assert!(
        folder.join("one.jpg").exists(),
        "Copy keeps the source tree"
    );
    assert_eq!(
        app.model
            .status_message
            .as_deref()
            .map(crate::i18n::unisolated)
            .as_deref(),
        Some("2 copied")
    );
}

// ---- Free destinations are never overwritten (review B1.1, B1.6) ----

fn perform_plan(plan: conflict::PastePlan, dest: &Path, mode: ClipMode) -> conflict::ConflictQueue {
    conflict::ConflictQueue::from_plan(
        dest.to_path_buf(),
        mode,
        plan,
        &mut services::perform_conflict_choice,
    )
}

#[test]
fn two_same_name_items_in_one_cut_both_survive() {
    let tmp = TmpDir::new("same-name");
    let (a, b, dest) = (
        tmp.path().join("a"),
        tmp.path().join("b"),
        tmp.path().join("dest"),
    );
    for dir in [&a, &b, &dest] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(a.join("report.txt"), b"from a").unwrap();
    fs::write(b.join("report.txt"), b"from b").unwrap();

    let sources = vec![a.join("report.txt"), b.join("report.txt")];
    let plan = services::plan_paste(&sources, &dest, ClipMode::Cut);
    assert!(plan.conflicts.is_empty());
    let queue = perform_plan(plan, &dest, ClipMode::Cut);
    assert!(
        queue
            .resolved
            .iter()
            .all(|item| matches!(item.outcome, conflict::ItemOutcome::Moved))
    );

    let mut contents: Vec<Vec<u8>> = fs::read_dir(&dest)
        .unwrap()
        .map(|entry| fs::read(entry.unwrap().path()).unwrap())
        .collect();
    contents.sort();
    assert_eq!(contents, vec![b"from a".to_vec(), b"from b".to_vec()]);
    assert!(dest.join("report (2).txt").exists());
}

#[test]
fn a_name_that_appears_after_planning_is_not_overwritten() {
    for mode in [ClipMode::Cut, ClipMode::Copy] {
        let tmp = TmpDir::new("late-arrival");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        let src = tmp.path().join("notes.txt");
        fs::write(&src, b"new").unwrap();

        let plan = services::plan_paste(std::slice::from_ref(&src), &dest, mode);
        assert_eq!(plan.clear.len(), 1);
        // Something else writes the name between planning and the transfer.
        fs::write(dest.join("notes.txt"), b"precious").unwrap();
        let queue = perform_plan(plan, &dest, mode);

        assert!(matches!(
            queue.resolved[0].outcome,
            conflict::ItemOutcome::Failed(_)
        ));
        assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"precious");
        assert_eq!(fs::read(&src).unwrap(), b"new", "the source is kept");
    }
}

#[test]
fn merging_never_moves_the_contents_of_a_linked_folder() {
    let tmp = TmpDir::new("link-merge");
    let elsewhere = tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep.jpg"), b"photo").unwrap();

    // src/Photos holds a link to `elsewhere`; dst/Photos/inner is a real folder.
    let src = tmp.path().join("src/Photos");
    fs::create_dir_all(&src).unwrap();
    std::os::unix::fs::symlink(&elsewhere, src.join("inner")).unwrap();
    let dst = tmp.path().join("dst/Photos");
    fs::create_dir_all(dst.join("inner")).unwrap();

    let mut queue = conflict::ConflictQueue::new(
        tmp.path().join("dst"),
        ClipMode::Cut,
        vec![conflict::Conflict {
            kind: ConflictKind::FolderVsFolder,
            dst_detail: String::new(),
            src_detail: String::new(),
            src,
            dst: dst.clone(),
            nested: false,
        }],
    );
    queue.resolve(
        ConflictChoice::Merge,
        &mut services::perform_conflict_choice,
    );
    let nested = queue.current.clone().expect("the link over a folder asks");
    assert_eq!(nested.kind, ConflictKind::TypeMismatch);
    queue.resolve(
        ConflictChoice::Replace,
        &mut services::perform_conflict_choice,
    );

    assert!(
        queue
            .resolved
            .iter()
            .all(|item| !matches!(item.outcome, conflict::ItemOutcome::Failed(_))),
        "{:?}",
        queue.resolved
    );
    assert_eq!(fs::read(elsewhere.join("keep.jpg")).unwrap(), b"photo");
    assert!(
        fs::symlink_metadata(dst.join("inner"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link itself moves; its target's files stay where they are"
    );
}

// ---- A cross-filesystem move removes only what it copied ----

fn cross_device(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::EXDEV))
}

fn change_source_during_copy(src: &Path, _staged: &Path) -> std::io::Result<()> {
    fs::write(src.join("late.jpg"), b"late")?;
    fs::write(src.join("sub/b.jpg"), b"edited after the copy read it")
}

#[test]
fn a_cross_filesystem_move_keeps_source_entries_that_changed_during_the_copy() {
    for (tag, late_writer) in [
        ("quiet", None),
        (
            "busy",
            Some(change_source_during_copy as fn(&Path, &Path) -> _),
        ),
    ] {
        let tmp = TmpDir::new(tag);
        let src = tmp.path().join("album");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.jpg"), b"a").unwrap();
        fs::write(src.join("sub/b.jpg"), b"b").unwrap();
        let dst = tmp.path().join("dest/album");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let mut seams = services::MoveSeams {
            initial_rename: cross_device,
            ..services::MoveSeams::PLACE
        };
        if let Some(hook) = late_writer {
            seams.after_stage_flush = hook;
        }

        let result = services::move_replacing_with(&src, &dst, seams);

        assert_eq!(fs::read(dst.join("a.jpg")).unwrap(), b"a");
        assert_eq!(fs::read(dst.join("sub/b.jpg")).unwrap(), b"b");
        assert!(!src.join("a.jpg").exists(), "a copied entry is removed");
        if late_writer.is_none() {
            assert_eq!(result, Ok(()));
            assert!(!src.exists());
        } else {
            assert!(result.is_err(), "leftovers are reported, not hidden");
            assert_eq!(fs::read(src.join("late.jpg")).unwrap(), b"late");
            assert_eq!(
                fs::read(src.join("sub/b.jpg")).unwrap(),
                b"edited after the copy read it"
            );
        }
    }
}

// ---- A folder is never pasted into itself (review B1.8) ----

#[test]
fn pasting_a_folder_into_its_own_subfolder_is_refused() {
    let tmp = TmpDir::new("into-itself");
    let folder = tmp.path().join("a");
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::write(folder.join("file.txt"), b"x").unwrap();
    for mode in [ClipMode::Copy, ClipMode::Cut] {
        let plan = services::plan_paste(std::slice::from_ref(&folder), &folder.join("sub"), mode);
        assert!(plan.clear.is_empty() && plan.conflicts.is_empty());
        let queue = perform_plan(plan, &folder.join("sub"), mode);
        assert!(matches!(
            queue.resolved[0].outcome,
            conflict::ItemOutcome::Failed(_)
        ));
    }
    assert_eq!(fs::read_dir(folder.join("sub")).unwrap().count(), 0);
}
