// TOCTOU rollback: source swaps after precheck roll back with
// identity-changed conflicts.

use super::*;

#[test]
fn toctou_source_swap_after_precheck_rolls_back_and_reports_identity_changed() {
    let scratch = ScratchDir::new("journal-toctou-rollback-success");
    let dir = scratch.path();
    let src = dir.join("original.txt");
    let dst = dir.join("renamed.txt");
    write_file(&src, b"real bytes");

    let mut journal = OpJournal::new();
    let _id = perform_rename_and_admit(&mut journal, &src, &dst);
    let recorded_ident = match &journal.peek_undo().unwrap().op {
        OpRecord::Rename { dst_inode, .. } => *dst_inode,
        _ => unreachable!(),
    };

    let entry = journal.take_undo_target().unwrap();
    let inverse = compute_inverse(&entry);
    let hooks = InverseHooks {
        after_precheck: swap_at,
        ..InverseHooks::PRODUCTION
    };
    let result = dispatch_inverse(&inverse, hooks);
    match &result {
        InverseResult::Conflict(InverseConflict::IdentityChanged {
            path,
            expected,
            actual,
        }) => {
            assert_eq!(path, &dst);
            assert_eq!(*expected, recorded_ident);
            let a = actual.as_ref().expect("actual identity");
            assert_ne!(
                a.ino, expected.ino,
                "post-rename lstat must observe the impostor's inode"
            );
        }
        other => panic!("expected IdentityChanged, got {other:?}"),
    }
    journal.undo_return(entry);
    assert_eq!(journal.undo_len(), 1, "journal custody preserved");
    // Rollback preserves the recorded backup and returns the impostor to its destination.
    assert!(!src.exists(), "no wrong entry left at the reverse-target");
    assert!(
        dst.symlink_metadata().is_ok(),
        "impostor rolled back to dst"
    );
    let backup = dst.with_extension("orig-bak");
    assert!(backup.exists(), "original bytes safely aside");
    assert_eq!(fs::read(&backup).unwrap(), b"real bytes");
}

#[test]
fn toctou_rollback_failure_reports_rollback_uncertain_and_preserves_entry() {
    let scratch = ScratchDir::new("journal-toctou-rollback-fail");
    let dir = scratch.path();
    let src = dir.join("orig.txt");
    let dst = dir.join("renamed.txt");
    write_file(&src, b"real");

    let mut journal = OpJournal::new();
    let _id = perform_rename_and_admit(&mut journal, &src, &dst);

    let entry = journal.take_undo_target().unwrap();
    let inverse = compute_inverse(&entry);
    let hooks = InverseHooks {
        after_precheck: swap_at,
        after_inverse: block_reverse_src,
        ..InverseHooks::PRODUCTION
    };
    let result = dispatch_inverse(&inverse, hooks);
    match &result {
        InverseResult::Conflict(InverseConflict::RollbackUncertain { at, detail }) => {
            assert_eq!(at, &src);
            assert!(
                !detail.is_empty(),
                "detail must describe the rollback failure"
            );
        }
        other => panic!("expected RollbackUncertain, got {other:?}"),
    }
    journal.undo_return(entry);
    assert_eq!(
        journal.undo_len(),
        1,
        "journal custody preserved on uncertain rollback"
    );
    assert_eq!(
        journal.redo_len(),
        0,
        "redo must not advance on uncertain rollback"
    );
    assert!(src.exists());
    assert!(dst.exists());
    let backup = dst.with_extension("orig-bak");
    assert!(backup.exists());
    assert_eq!(fs::read(&backup).unwrap(), b"real");
}
