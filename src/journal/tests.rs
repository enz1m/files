// Real-filesystem tests for the session-only operation journal.
//
// Every fixture is a `ScratchDir`; no test touches host paths.

use super::*;
use crate::services;
use crate::trash::TrashEntry;
use enzui_test_support::test_scratch::ScratchDir;
use std::fs;
use std::path::{Path, PathBuf};

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, bytes).expect("seed file");
}

fn perform_rename_and_admit(journal: &mut OpJournal, src: &Path, dst: &Path) -> JournalId {
    let outcome = services::apply_bulk_rename(&[(src.to_path_buf(), dst.to_path_buf())]);
    assert!(outcome.failed.is_empty(), "seed rename failed: {outcome:?}");
    let dst_inode = InodeIdent::of(dst).expect("dst inode");
    journal
        .record(
            OpRecord::Rename {
                src: src.to_path_buf(),
                dst: dst.to_path_buf(),
                dst_inode,
            },
            now_ms(),
        )
        .expect("seed rename admission fits the byte cap")
}

fn seed_trash_entry(scratch: &Path, origin: &Path, name: &str, bytes: &[u8]) -> TrashEntry {
    let files_dir = scratch.join("Trash").join("files");
    let info_dir = scratch.join("Trash").join("info");
    fs::create_dir_all(&files_dir).unwrap();
    fs::create_dir_all(&info_dir).unwrap();
    write_file(origin, bytes);
    let trashed_path = files_dir.join(name);
    fs::rename(origin, &trashed_path).expect("simulate trash move");
    let info_path = info_dir.join(format!("{name}.trashinfo"));
    let info_body = format!(
        "[Trash Info]\nPath={}\nDeletionDate=2024-01-01T00:00:00\n",
        origin.display()
    );
    fs::write(&info_path, info_body).unwrap();
    TrashEntry {
        trashed_path,
        info_path,
        original_path: origin.to_path_buf(),
        name: name.to_owned(),
        deletion_date: "2024-01-01T00:00:00".to_owned(),
        retained_root: None,
        mime: String::new(),
    }
}

fn same_fs_move_and_admit(journal: &mut OpJournal, src: &Path, dst: &Path) -> JournalId {
    services::move_replacing(src, dst).expect("same-fs move");
    let dst_inode = InodeIdent::of(dst).expect("dst inode");
    journal
        .record(
            OpRecord::Move {
                src: src.to_path_buf(),
                dst: dst.to_path_buf(),
                dst_inode,
            },
            now_ms(),
        )
        .expect("same-fs move admission fits the byte cap")
}

fn take_and_apply_undo(journal: &mut OpJournal) -> (JournalEntry, InverseResult) {
    let entry = journal.take_undo_target().expect("undo target present");
    let inverse = compute_inverse(&entry);
    let result = apply_inverse(&inverse);
    (entry, result)
}

fn take_and_apply_redo(journal: &mut OpJournal) -> (JournalEntry, InverseResult) {
    let entry = journal.take_redo_target().expect("redo target present");
    let forward = compute_redo(&entry);
    let result = apply_inverse(&forward);
    (entry, result)
}

// Seeds a trash entry and returns the info-file bytes, so tests can assert
// the `.trashinfo` twin survives byte-for-byte on every non-Applied outcome.
fn seed_trash_for_toctou(dir: &Path, bytes: &[u8]) -> (PathBuf, PathBuf, PathBuf, Vec<u8>) {
    let origin = dir.join("restored.txt");
    let trash = seed_trash_entry(dir, &origin, "restored.txt", bytes);
    let info_bytes = fs::read(&trash.info_path).expect("seeded info twin readable");
    (origin, trash.trashed_path, trash.info_path, info_bytes)
}

#[test]
fn rename_undo_and_redo_roundtrip_restores_source_and_settles_dest() {
    let scratch = ScratchDir::new("journal-rename-roundtrip");
    let dir = scratch.path();
    let src = dir.join("original.txt");
    let dst = dir.join("renamed.txt");
    write_file(&src, b"hello");

    let mut journal = OpJournal::new();
    let id = perform_rename_and_admit(&mut journal, &src, &dst);
    assert_eq!(journal.undo_len(), 1);

    let (entry, result) = take_and_apply_undo(&mut journal);
    assert_eq!(entry.id, id);
    assert!(matches!(result, InverseResult::Applied));
    assert!(src.exists(), "source restored");
    assert!(!dst.exists(), "dest removed");
    journal.undo_applied(entry);
    assert_eq!(journal.undo_len(), 0);
    assert_eq!(journal.redo_len(), 1);

    let (entry, result) = take_and_apply_redo(&mut journal);
    assert!(matches!(result, InverseResult::Applied));
    assert!(!src.exists());
    assert!(dst.exists(), "redo re-renamed");
    journal.redo_applied(entry);
    assert_eq!(journal.undo_len(), 1);
    assert_eq!(journal.redo_len(), 0);
}

#[test]
fn trash_undo_restores_and_redo_returns_to_trash() {
    let scratch = ScratchDir::new("journal-trash-roundtrip");
    let dir = scratch.path();
    let origin = dir.join("doc.txt");
    let entry = seed_trash_entry(dir, &origin, "doc.txt", b"payload");
    let seeded_info_bytes = fs::read(&entry.info_path).unwrap();

    let mut journal = OpJournal::new();
    let trashed_inode = InodeIdent::of(&entry.trashed_path).unwrap();
    let _ = journal
        .record(
            OpRecord::Trash {
                origin: origin.clone(),
                trashed_path: entry.trashed_path.clone(),
                info_path: entry.info_path.clone(),
                trashed_inode,
                info_inode: InodeIdent::of(&entry.info_path)
                    .unwrap_or_else(|_| InodeIdent::synthetic(0, 0)),
                info_bytes: seeded_info_bytes.clone(),
            },
            now_ms(),
        )
        .expect("seed trash admission fits the byte cap");

    let (j_entry, result) = take_and_apply_undo(&mut journal);
    assert!(
        matches!(result, InverseResult::Applied),
        "restore result: {result:?}"
    );
    assert!(origin.exists(), "origin restored");
    assert!(!entry.trashed_path.exists(), "payload gone");
    assert!(!entry.info_path.exists(), "info twin cleaned");
    journal.undo_applied(j_entry);

    let (j_entry, result) = take_and_apply_redo(&mut journal);
    assert!(
        matches!(result, InverseResult::Applied),
        "retrash result: {result:?}"
    );
    assert!(!origin.exists(), "origin returned to trash");
    assert!(entry.trashed_path.exists(), "payload back in trash files");
    // External trash tools need `[Trash Info]`, `Path=` and `DeletionDate=`
    // verbatim.
    assert!(entry.info_path.exists(), "twin republished");
    let republished_bytes = fs::read(&entry.info_path).unwrap();
    assert_eq!(
        republished_bytes, seeded_info_bytes,
        "Redo must publish the recorded metadata verbatim, not a synthesised approximation"
    );
    let republished = String::from_utf8(republished_bytes).expect("utf-8 info body");
    assert!(
        republished.starts_with("[Trash Info]\n"),
        "header intact: {republished:?}"
    );
    assert!(
        republished.contains("Path="),
        "Path field intact: {republished:?}"
    );
    assert!(
        republished.contains("DeletionDate="),
        "DeletionDate field intact: {republished:?}"
    );
    journal.redo_applied(j_entry);

    let (j_entry, result) = take_and_apply_undo(&mut journal);
    assert!(
        matches!(result, InverseResult::Applied),
        "second-Undo verdict: {result:?}"
    );
    assert!(
        origin.exists(),
        "payload restored to origin the second time"
    );
    assert_eq!(fs::read(&origin).unwrap(), b"payload");
    assert!(!entry.trashed_path.exists());
    assert!(!entry.info_path.exists());
    journal.undo_applied(j_entry);
    assert_eq!(journal.redo_len(), 1);
}

#[test]
fn same_fs_move_undo_returns_source_to_original_parent() {
    let scratch = ScratchDir::new("journal-move-samefs");
    let dir = scratch.path();
    let src = dir.join("a/file.bin");
    let dst = dir.join("b/file.bin");
    write_file(&src, b"same-fs");
    fs::create_dir_all(dir.join("b")).unwrap();

    let mut journal = OpJournal::new();
    same_fs_move_and_admit(&mut journal, &src, &dst);
    assert!(!src.exists() && dst.exists());

    let (_entry, result) = take_and_apply_undo(&mut journal);
    assert!(matches!(result, InverseResult::Applied));
    assert!(src.exists(), "source restored");
    assert!(!dst.exists(), "dest gone");
}

#[test]
fn external_conflict_preserves_the_entry_and_reports_identity_change() {
    let scratch = ScratchDir::new("journal-external-conflict");
    let dir = scratch.path();
    let src = dir.join("orig.txt");
    let dst = dir.join("renamed.txt");
    write_file(&src, b"one");
    let mut journal = OpJournal::new();
    let _id = perform_rename_and_admit(&mut journal, &src, &dst);

    fs::remove_file(&dst).unwrap();
    write_file(&dst, b"impostor");

    let entry = journal.take_undo_target().unwrap();
    let inverse = compute_inverse(&entry);
    let result = apply_inverse(&inverse);
    match &result {
        InverseResult::Conflict(InverseConflict::IdentityChanged { path, .. }) => {
            assert_eq!(path, &dst);
        }
        other => panic!("expected IdentityChanged, got {other:?}"),
    }
    journal.undo_return(entry);
    assert_eq!(journal.undo_len(), 1, "entry retained on conflict");
    assert!(dst.exists(), "external file untouched");
}

// TOCTOU fault hooks: `swap_at` replaces the entry at a path with an
// impostor; `block_reverse_src` makes the rollback's no-replace rename hit
// `EEXIST`; `swap_at_reverse_dst` fires between the post-inverse check and the
// rollback, so revalidation sees a second substitution.
fn swap_at(path: &Path) {
    let backup = path.with_extension("orig-bak");
    let _ = fs::rename(path, &backup);
    let _ = fs::write(path, b"impostor");
}

fn block_reverse_src(reverse_src: &Path, _reverse_dst: &Path) {
    let _ = fs::write(reverse_src, b"blocker");
}

fn swap_reverse_dst(_reverse_src: &Path, reverse_dst: &Path) {
    let backup = reverse_dst.with_extension("middle-bak");
    let _ = fs::rename(reverse_dst, &backup);
    let _ = fs::write(reverse_dst, b"middle-impostor");
}

#[test]
fn rollback_middle_race_leaves_replacement_untouched_and_returns_uncertain() {
    let scratch = ScratchDir::new("journal-rollback-middle-race");
    let dir = scratch.path();
    let src = dir.join("original.txt");
    let dst = dir.join("renamed.txt");
    write_file(&src, b"recorded");

    let mut journal = OpJournal::new();
    let _id = perform_rename_and_admit(&mut journal, &src, &dst);

    let entry = journal.take_undo_target().unwrap();
    let inverse = compute_inverse(&entry);
    let hooks = InverseHooks {
        after_precheck: swap_at,
        between_postcheck_and_rollback: swap_reverse_dst,
        ..InverseHooks::PRODUCTION
    };
    let result = dispatch_inverse(&inverse, hooks);
    match &result {
        InverseResult::Conflict(InverseConflict::RollbackUncertain { at, detail }) => {
            assert_eq!(at, &src, "uncertain names current custody location");
            assert!(
                detail.contains("changed") || detail.contains("disappeared"),
                "detail must name the mid-race substitution: {detail}"
            );
        }
        other => panic!("expected RollbackUncertain, got {other:?}"),
    }
    journal.undo_return(entry);
    assert_eq!(journal.undo_len(), 1);
    let middle_bak = src.with_extension("middle-bak");
    assert!(
        middle_bak.exists(),
        "the mid-race first impostor is at its backup, untouched by rollback"
    );
    assert!(
        src.symlink_metadata().is_ok(),
        "middle-race replacement remains at src"
    );
    assert_eq!(fs::read(&src).unwrap(), b"middle-impostor");
    assert!(!dst.exists(), "rollback never fired, so dst stays empty");
    let precheck_bak = dst.with_extension("orig-bak");
    assert!(precheck_bak.exists());
    assert_eq!(fs::read(&precheck_bak).unwrap(), b"recorded");
}

#[path = "tests/symlink_tests.rs"]
mod symlink_tests;
#[path = "tests/toctou_tests.rs"]
mod toctou_tests;
#[path = "tests/trash_restore_tests.rs"]
mod trash_restore_tests;
#[path = "tests/trash_twin_tests.rs"]
mod trash_twin_tests;
