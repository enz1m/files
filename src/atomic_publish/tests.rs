// One interrupted-write gate per file this app owns.

use super::*;
use crate::model::{Place, PlaceKind};
use crate::services::{load_bookmarks_from, save_bookmarks_to, serialize_bookmarks};
use enzui_test_support::test_scratch::ScratchDir;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

fn inode(path: &Path) -> u64 {
    std::fs::metadata(path).expect("publication metadata").ino()
}

// Stage a differing prefix so an in-place overwrite cannot pass by leaving identical
// bytes.
fn arm_interrupted_prefix(old_bytes: &[u8], new_bytes: &[u8]) {
    let index = old_bytes
        .iter()
        .zip(new_bytes)
        .position(|(a, b)| a != b)
        .expect("the two fixtures must differ inside their common prefix");
    let old_byte = old_bytes[index];
    let new_byte = new_bytes[index];
    assert_ne!(old_byte, new_byte);
    inject_raw_writer(move |staged, bytes| {
        assert_eq!(bytes[index], new_byte);
        assert_ne!(bytes[index], old_byte);
        staged.write_all(&bytes[..=index])?;
        Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "injected after the staged prefix",
        ))
    });
}

fn entries_besides(dir: &Path, keep: &[&Path]) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read publication directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| !keep.contains(&path.as_path()))
        .collect()
}

fn pins(places: &[Place]) -> Vec<(String, PathBuf)> {
    places
        .iter()
        .map(|place| (place.label.clone(), place.path.clone()))
        .collect()
}

fn place(label: &str, path: &str) -> Place {
    Place {
        id: label.to_ascii_lowercase(),
        kind: PlaceKind::Bookmark,
        label: label.to_owned(),
        icon: "folder".into(),
        path: PathBuf::from(path),
    }
}

#[test]
fn bookmark_publication_survives_an_interrupted_staged_write() {
    let scratch = ScratchDir::new("bookmarks");
    let dir = scratch.path();
    let path = dir.join("bookmarks");

    let old = vec![place("Archive", "/home/u/Archive")];
    save_bookmarks_to(&path, &old).expect("seed the previous file");
    let old_bytes = std::fs::read(&path).expect("seeded bytes");
    let seeded_inode = inode(&path);

    let replacement = vec![place("Blueprints", "/home/u/Blueprints")];
    let replacement_bytes = serialize_bookmarks(&replacement)
        .expect("serialize")
        .into_bytes();
    arm_interrupted_prefix(&old_bytes, &replacement_bytes);

    let error =
        save_bookmarks_to(&path, &replacement).expect_err("the interrupted write must be reported");
    assert_eq!(error.kind(), std::io::ErrorKind::WriteZero);
    assert_eq!(std::fs::read(&path).expect("old bytes"), old_bytes);
    assert_eq!(
        inode(&path),
        seeded_inode,
        "the old file was replaced anyway"
    );
    assert_eq!(pins(&load_bookmarks_from(&path)), pins(&old));
    assert!(
        entries_besides(dir, &[&path]).is_empty(),
        "the interrupted write left staging residue"
    );

    save_bookmarks_to(&path, &replacement).expect("the retry publishes");
    let published = std::fs::read(&path).expect("published bytes");
    assert_eq!(published, replacement_bytes);
    assert_ne!(
        inode(&path),
        seeded_inode,
        "publication must install a new inode rather than rewrite the old one"
    );
    assert_eq!(pins(&load_bookmarks_from(&path)), pins(&replacement));
    assert!(!published.starts_with(b"enzui-iced "));
}

#[cfg(unix)]
#[test]
fn planted_legacy_staging_names_cannot_capture_or_lose_a_publication() {
    use std::os::unix::fs::symlink;

    let scratch = ScratchDir::new("planted");
    let dir = scratch.path();
    let path = dir.join("bookmarks");
    let victim = dir.join("victim");
    std::fs::write(&victim, b"precious").expect("seed the victim");
    let fixed = path.with_extension("tmp");
    let per_process = dir.join(format!("bookmarks.tmp-{}", std::process::id()));
    symlink(&victim, &fixed).expect("plant the fixed staging name");
    symlink(&victim, &per_process).expect("plant the per-process staging name");

    let bookmarks = vec![place("Reports", "/home/u/Reports")];
    save_bookmarks_to(&path, &bookmarks).expect("publish over planted staging names");

    assert_eq!(std::fs::read(&victim).expect("victim bytes"), b"precious");
    assert!(
        std::fs::symlink_metadata(&fixed)
            .expect("the planted name is not ours to delete")
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::symlink_metadata(&per_process)
            .expect("the planted name is not ours to delete")
            .file_type()
            .is_symlink()
    );
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("destination metadata")
            .file_type()
            .is_file()
    );
    assert_eq!(pins(&load_bookmarks_from(&path)), pins(&bookmarks));
}
