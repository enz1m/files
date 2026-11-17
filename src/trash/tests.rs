// Trash writes to `<trash>/files/NAME` with a `<trash>/info/NAME.trashinfo`
// whose `Path=` names the origin, for files and directories; collisions get
// `NAME.1`, and a read-only mount fails instead of falling back into `$HOME`.
//
// Every test stages a private `home_data_dir`, so nothing reaches the user's
// real `~/.local/share/Trash`.
use super::*;
use std::fs;
use std::time::SystemTime;

fn tmp(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "files-app-1181-trash-{}-{}-{}",
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

fn workspace(tag: &str) -> (PathBuf, PathBuf) {
    let root = tmp(tag);
    let home_data = root.join("data");
    fs::create_dir_all(&home_data).unwrap();
    (root, home_data)
}

#[test]
fn unusual_names_round_trip_through_the_listing() {
    use std::os::unix::ffi::OsStrExt;
    let (root, home_data) = workspace("unusual-names");
    for name in [
        std::ffi::OsStr::new("a\nb.txt"),
        std::ffi::OsStr::from_bytes(b"latin1-\xe9.txt"),
    ] {
        let src = root.join(name);
        fs::write(&src, b"x").unwrap();
        let expected = root.canonicalize().unwrap().join(name);
        move_to_trash_with(&src, &home_data).unwrap();
        let entries = list_trash(&home_data);
        assert!(
            entries.iter().any(|entry| entry.original_path == expected),
            "{expected:?} must survive the Path= round trip"
        );
    }
}

use enzui_core::tempstage as core_tempstage;
use enzui_security::durable as security_durable;

fn info_workspace(tag: &str) -> (PathBuf, PathBuf) {
    let (_root, home_data) = workspace(tag);
    let info_dir = home_data.join("Trash").join("info");
    fs::create_dir_all(&info_dir).expect("info dir seeded");
    fs::create_dir_all(home_data.join("Trash").join("files")).expect("files dir seeded");
    let info_path = info_dir.join("doc.txt.trashinfo");
    (home_data, info_path)
}

fn recorded_info_bytes(origin: &Path) -> Vec<u8> {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate=2024-01-01T00:00:00\n",
        origin.display()
    )
    .into_bytes()
}

fn clear_all_faults() {
    core_tempstage::fault::clear();
    security_durable::fault::clear();
    publish_fault::clear();
    retire_fault::clear();
    scan_fault::clear();
}

struct FaultsGuard;
impl Drop for FaultsGuard {
    fn drop(&mut self) {
        clear_all_faults();
    }
}

#[test]
fn publish_metadata_twin_substitution_around_link_refuses_to_remove_foreign_bytes() {
    let _guard = FaultsGuard;
    let (_home_data, info_path) = info_workspace("publish-substitution");
    let bytes = recorded_info_bytes(Path::new("/tmp/origin/doc.txt"));
    let stolen_path = info_path.clone();

    // Substitute the linked name for a foreign inode after link,
    // before parent sync.
    publish_fault::arm_after_link(Box::new(move |_p: &Path| {
        let _ = fs::remove_file(&stolen_path);
        fs::write(&stolen_path, b"foreign-metadata").expect("plant substitute");
    }));

    let err =
        publish_metadata_twin(&info_path, &bytes).expect_err("substitution around link is refused");

    match err {
        PublishError::SubstitutedForeignTwin { .. } => {}
        other => panic!("expected SubstitutedForeignTwin, got {other:?}"),
    }
    assert_eq!(
        fs::read(&info_path).expect("foreign twin still readable"),
        b"foreign-metadata",
        "the substituted foreign bytes must never be removed"
    );
}

#[test]
fn retire_metadata_twin_refuses_substituted_bytes_without_touching_them() {
    let _guard = FaultsGuard;
    let (_home, info_path) = info_workspace("retire-substituted");
    let admitted_bytes = recorded_info_bytes(Path::new("/tmp/origin/doc.txt"));
    fs::write(&info_path, b"foreign-metadata").unwrap();

    let err =
        retire_metadata_twin(&info_path, &admitted_bytes).expect_err("substituted bytes refused");
    match err {
        RetireError::Substituted { .. } => {}
        other => panic!("expected Substituted, got {other:?}"),
    }
    assert_eq!(
        fs::read(&info_path).unwrap(),
        b"foreign-metadata",
        "foreign bytes must never be removed"
    );
}

fn plant_fifo(path: &Path) {
    use rustix::fs::{CWD, Mode};
    use std::os::unix::fs::FileTypeExt;
    rustix::fs::mkfifoat(CWD, path, Mode::RUSR | Mode::WUSR)
        .expect("mkfifoat must create a FIFO at the twin name");
    let md = fs::symlink_metadata(path).expect("fifo lstat");
    assert!(md.file_type().is_fifo(), "fixture must plant a FIFO");
}

// Completion beacon: a blocking open surfaces as a timeout panic, not an
// unbounded worker join.
fn assert_returned_within(
    label: &str,
    budget: std::time::Duration,
    work: impl FnOnce() + Send + 'static,
) {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        work();
        let _ = tx.send(());
    });
    match rx.recv_timeout(budget) {
        Ok(()) => {
            handle.join().expect("worker joined");
        }
        Err(_) => panic!(
            "{label} did not return within {budget:?}; a blocking open regressed the non-blocking flag"
        ),
    }
}

#[test]
fn capture_admitted_metadata_twin_refuses_fifo_without_blocking() {
    let _guard = FaultsGuard;
    let (_home, info_path) = info_workspace("capture-fifo");
    plant_fifo(&info_path);
    let probe = info_path.clone();
    assert_returned_within(
        "capture_admitted_metadata_twin",
        std::time::Duration::from_secs(3),
        move || {
            let err =
                capture_admitted_metadata_twin(&probe).expect_err("FIFO at twin name refused");
            // `open(FIFO, O_RDONLY|O_NONBLOCK)` returns without a
            // writer; the descriptor kind check then refuses the
            // non-regular file with `InvalidData`.
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidData,
                "raw errno: {:?}",
                err.raw_os_error()
            );
        },
    );
    use std::os::unix::fs::FileTypeExt;
    let md = fs::symlink_metadata(&info_path).expect("fifo preserved");
    assert!(md.file_type().is_fifo(), "FIFO must not be removed");
}
