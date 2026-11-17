// Trash restore: missing parents, dangling links, source swaps, and
// rollback blocked at the trash.

use super::*;

#[test]
fn trash_restore_source_swap_after_precheck_rolls_back_with_identity_changed() {
    let scratch = ScratchDir::new("journal-trash-toctou-rollback");
    let dir = scratch.path();
    let (origin, trashed_path, info_path, seeded_info_bytes) = seed_trash_for_toctou(dir, b"real");

    let mut journal = OpJournal::new();
    let trashed_inode = InodeIdent::of(&trashed_path).unwrap();
    let _ = journal
        .record(
            OpRecord::Trash {
                origin: origin.clone(),
                trashed_path: trashed_path.clone(),
                info_path: info_path.clone(),
                trashed_inode,
                info_inode: InodeIdent::of(&info_path)
                    .unwrap_or_else(|_| InodeIdent::synthetic(0, 0)),
                info_bytes: seeded_info_bytes.clone(),
            },
            now_ms(),
        )
        .expect("admit trash");

    let entry = journal.take_undo_target().unwrap();
    let inverse = compute_inverse(&entry);
    // Substitute the trash payload between precheck and the actual
    // `restore_without_replacing` call.
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
            assert_eq!(path, &trashed_path, "rolled back to the trash location");
            assert_eq!(expected.kind, FileKind::Regular);
            let a = actual.as_ref().expect("actual identity");
            assert_ne!(a.ino, expected.ino, "impostor has a different inode");
        }
        other => panic!("expected IdentityChanged, got {other:?}"),
    }
    journal.undo_return(entry);
    assert_eq!(journal.undo_len(), 1);
    // Rollback keeps the recorded bytes and metadata twin while returning the impostor to trash.
    assert!(!origin.exists());
    assert!(trashed_path.exists(), "impostor rolled back to trash");
    let backup = trashed_path.with_extension("orig-bak");
    assert!(backup.exists());
    assert_eq!(fs::read(&backup).unwrap(), b"real");
    assert!(
        info_path.exists(),
        "metadata twin preserved through the rollback"
    );
    assert_eq!(
        fs::read(&info_path).unwrap(),
        seeded_info_bytes,
        "metadata twin byte-for-byte preserved (a delayed-unlink regression reds this)"
    );
}
