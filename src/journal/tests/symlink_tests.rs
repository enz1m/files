// Symlink identity: link capture through lstat and undo restoring
// the link itself.

use super::*;

// A new symlink at `link_path` to `target`; the returned inode is the link's
// own, not its target's.
fn seed_symlink(link_path: &Path, target: &Path) -> InodeIdent {
    std::os::unix::fs::symlink(target, link_path).expect("create symlink");
    InodeIdent::of(link_path).expect("lstat succeeds on the link")
}

#[test]
fn dangling_symlink_can_be_renamed_journaled_and_undone() {
    let scratch = ScratchDir::new("journal-symlink-dangling");
    let dir = scratch.path();
    let phantom = dir.join("does-not-exist");
    let link_src = dir.join("dangling_src");
    let link_dst = dir.join("dangling_dst");
    let seed_ident = seed_symlink(&link_src, &phantom);
    assert!(!phantom.exists(), "target must be absent");
    assert!(
        std::fs::metadata(&link_src).is_err(),
        "follow-metadata fails"
    );
    assert!(link_src.symlink_metadata().is_ok(), "lstat still succeeds");
    assert_eq!(seed_ident.kind, FileKind::Symlink);

    fs::rename(&link_src, &link_dst).unwrap();
    let dst_inode = InodeIdent::of(&link_dst).expect("lstat the dangling link");
    let mut journal = OpJournal::new();
    let _ = journal
        .record(
            OpRecord::Rename {
                src: link_src.clone(),
                dst: link_dst.clone(),
                dst_inode,
            },
            now_ms(),
        )
        .expect("dangling symlink must be journalable");

    let (_entry, result) = take_and_apply_undo(&mut journal);
    assert!(
        matches!(result, InverseResult::Applied),
        "dangling symlink undo verdict: {result:?}"
    );
    assert!(link_src.symlink_metadata().is_ok());
    assert!(!link_dst.exists());
}
