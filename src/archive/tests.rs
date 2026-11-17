use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

// A fresh, empty directory removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "files-archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn never() -> bool {
    false
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn sample_tree(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let proj = root.join("proj");
    fs::create_dir_all(proj.join("sub")).unwrap();
    fs::write(proj.join("a.txt"), b"alpha\n").unwrap();
    fs::write(proj.join("sub/b.bin"), vec![7u8; 200_000]).unwrap();
    fs::write(proj.join("run.sh"), b"#!/bin/sh\n").unwrap();
    fs::set_permissions(proj.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("a.txt", proj.join("link")).unwrap();
    proj
}

fn assert_sample_extracted(out: &Path) {
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(fs::read(out.join("sub/b.bin")).unwrap(), vec![7u8; 200_000]);
    let mode = fs::metadata(out.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the executable bit survives");
    assert!(
        fs::symlink_metadata(out.join("link")).is_err(),
        "a symlink entry is skipped, never created"
    );
}

#[test]
fn zip_and_tar_gz_round_trip_a_tree_and_skip_its_symlink() {
    for (format, extension) in [
        (ArchiveFormat::Zip, "zip"),
        (ArchiveFormat::TarGz, "tar.gz"),
    ] {
        let scratch = Scratch::new();
        let proj = sample_tree(&scratch.dir("src"));
        let out_dir = scratch.dir("out");

        let packed = compress(&[proj], &out_dir, format, &Limits::default(), &never).unwrap();
        assert_eq!(packed.output, out_dir.join(format!("proj.{extension}")));
        assert_eq!(packed.skipped, 0);
        assert_eq!(names(&out_dir), vec![format!("proj.{extension}")]);

        let unpack_dir = scratch.dir("unpacked");
        let report = extract(&packed.output, &unpack_dir, &Limits::default(), &never).unwrap();
        assert_eq!(report.output, unpack_dir.join("proj"), "{format:?}");
        assert_eq!(report.files, 3, "{format:?}");
        assert_eq!(report.skipped, 1, "{format:?}: the symlink");
        assert_sample_extracted(&report.output.join("proj"));
        assert_eq!(
            names(&unpack_dir),
            vec!["proj".to_owned()],
            "no staging residue"
        );
    }
}

#[test]
fn outputs_never_replace_an_existing_entry() {
    let scratch = Scratch::new();
    let src = scratch.dir("src");
    fs::write(src.join("report.pdf"), b"pdf").unwrap();
    let dest = scratch.dir("dest");
    fs::write(dest.join("report.zip"), b"keep me").unwrap();

    let packed = compress(
        &[src.join("report.pdf")],
        &dest,
        ArchiveFormat::Zip,
        &Limits::default(),
        &never,
    )
    .unwrap();
    assert_eq!(packed.output, dest.join("report (2).zip"));
    assert_eq!(fs::read(dest.join("report.zip")).unwrap(), b"keep me");

    fs::create_dir(dest.join("report (2)")).unwrap();
    fs::write(dest.join("report (2)/existing"), b"x").unwrap();
    let report = extract(&packed.output, &dest, &Limits::default(), &never).unwrap();
    assert_eq!(report.output, dest.join("report (2) (2)"));
    assert_eq!(names(&dest.join("report (2)")), vec!["existing"]);
}

fn raw_tar_entry(
    builder: &mut tar::Builder<File>,
    name: &[u8],
    kind: tar::EntryType,
    link: &[u8],
    data: &[u8],
) {
    let mut header = tar::Header::new_old();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.as_old_mut().linkname[..link.len()].copy_from_slice(link);
    header.set_entry_type(kind);
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    header.set_cksum();
    builder.append(&header, data).unwrap();
}

#[test]
fn hostile_tar_entries_cannot_reach_outside_the_output() {
    let scratch = Scratch::new();
    let archive = scratch.path().join("evil.tar");
    let mut builder = tar::Builder::new(File::create(&archive).unwrap());
    raw_tar_entry(
        &mut builder,
        b"../escaped.txt",
        tar::EntryType::Regular,
        b"",
        b"no",
    );
    raw_tar_entry(
        &mut builder,
        b"/tmp/absolute.txt",
        tar::EntryType::Regular,
        b"",
        b"no",
    );
    raw_tar_entry(&mut builder, b"link", tar::EntryType::Symlink, b"/etc", b"");
    raw_tar_entry(
        &mut builder,
        b"hard",
        tar::EntryType::Link,
        b"/etc/passwd",
        b"",
    );
    raw_tar_entry(
        &mut builder,
        b"link/passwd",
        tar::EntryType::Regular,
        b"",
        b"inside",
    );
    raw_tar_entry(
        &mut builder,
        b"ok.txt",
        tar::EntryType::Regular,
        b"",
        b"fine",
    );
    builder.finish().unwrap();

    let dest = scratch.dir("dest");
    let report = extract(&archive, &dest, &Limits::default(), &never).unwrap();
    assert_eq!(
        report.skipped, 4,
        "traversal, absolute, symlink and hardlink"
    );
    assert_eq!(report.files, 2);
    assert_eq!(report.output, dest.join("evil"));
    assert_eq!(fs::read(report.output.join("ok.txt")).unwrap(), b"fine");
    let planted = report.output.join("link");
    assert!(
        fs::symlink_metadata(&planted).unwrap().is_dir(),
        "`link` is an ordinary folder, never the symlink an earlier entry declared"
    );
    assert_eq!(fs::read(planted.join("passwd")).unwrap(), b"inside");
    assert!(!scratch.path().join("escaped.txt").exists());
    assert_eq!(names(scratch.path()), vec!["dest", "evil.tar"]);
}

#[test]
fn hostile_zip_names_are_skipped() {
    let scratch = Scratch::new();
    let archive = scratch.path().join("evil.zip");
    let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for name in ["../escaped.txt", "/abs.txt", "ok.txt"] {
        zip.start_file(name, options).unwrap();
        zip.write_all(b"data").unwrap();
    }
    zip.add_symlink("link", "/etc", options).unwrap();
    zip.finish().unwrap();

    let dest = scratch.dir("dest");
    let report = extract(&archive, &dest, &Limits::default(), &never).unwrap();
    assert_eq!(report.skipped, 3);
    assert_eq!(report.files, 1);
    assert_eq!(names(&report.output), vec!["ok.txt"]);
    assert!(!scratch.path().join("escaped.txt").exists());
}

#[test]
fn limits_count_written_bytes_and_entries_and_leave_nothing_behind() {
    let scratch = Scratch::new();
    let proj = sample_tree(&scratch.dir("src"));
    let packed = compress(
        &[proj],
        &scratch.dir("packed"),
        ArchiveFormat::TarGz,
        &Limits::default(),
        &never,
    )
    .unwrap();

    let dest = scratch.dir("dest");
    let small_bytes = Limits {
        max_total_bytes: 100_000,
        ..Limits::default()
    };
    assert!(matches!(
        extract(&packed.output, &dest, &small_bytes, &never),
        Err(ArchiveError::LimitExceeded("total size"))
    ));
    let few_entries = Limits {
        max_entries: 2,
        ..Limits::default()
    };
    assert!(matches!(
        extract(&packed.output, &dest, &few_entries, &never),
        Err(ArchiveError::LimitExceeded("entry count"))
    ));
    assert!(
        names(&dest).is_empty(),
        "a refused extraction removes its staging"
    );

    // Positive control: the same archive under the default limits extracts.
    assert!(extract(&packed.output, &dest, &Limits::default(), &never).is_ok());
}

#[test]
fn rar4_rar5_and_seven_zip_are_read_without_command_line_archivers() {
    for (name, data, files) in [
        (
            "rar4.rar",
            include_bytes!("../../tests/fixtures/archives/rar4.rar").as_slice(),
            2,
        ),
        (
            "rar5.rar",
            include_bytes!("../../tests/fixtures/archives/rar5.rar").as_slice(),
            1,
        ),
        (
            "sample.7z",
            include_bytes!("../../tests/fixtures/archives/sample.7z").as_slice(),
            1,
        ),
    ] {
        let scratch = Scratch::new();
        let archive = scratch.path().join(name);
        fs::write(&archive, data).unwrap();
        let dest = scratch.dir("out");
        let report = extract(&archive, &dest, &Limits::default(), &never).unwrap();
        assert_eq!(report.files, files);
        assert_eq!(report.output.file_stem(), archive.file_stem());
        assert!(report.bytes > 0);
        if name == "rar4.rar" {
            assert_eq!(report.skipped, 1);
            assert_eq!(fs::read(report.output.join("test.txt")).unwrap().len(), 20);
        }
        if name == "rar5.rar" {
            assert_eq!(
                fs::read(report.output.join("test.bin")).unwrap().len(),
                1200
            );
        }
        if name == "sample.7z" {
            assert_eq!(
                fs::read(report.output.join("hello.txt")).unwrap(),
                b"hello archive\n"
            );
        }
    }
}

#[test]
fn encrypted_and_corrupt_native_archives_do_not_publish_partial_folders() {
    let scratch = Scratch::new();
    for (name, data) in [
        (
            "encrypted.rar",
            include_bytes!("../../tests/fixtures/archives/encrypted.rar").as_slice(),
        ),
        ("broken.7z", b"not an archive".as_slice()),
        ("broken.rar", b"Rar!".as_slice()),
        ("broken.tar.xz", b"invalid".as_slice()),
    ] {
        let archive = scratch.path().join(name);
        fs::write(&archive, data).unwrap();
        let out = scratch.dir(&format!("out-{name}"));
        let error = extract(&archive, &out, &Limits::default(), &never).unwrap_err();
        if name == "encrypted.rar" {
            assert!(
                matches!(
                    error,
                    ArchiveError::Extraction(
                        enzui_services::archive::ArchiveError::EncryptionUnsupported
                            | enzui_services::archive::ArchiveError::PasswordRequired
                    )
                ),
                "{error:?}"
            );
        } else {
            assert!(matches!(error, ArchiveError::Corrupt(_)), "{error:?}");
        }
        assert!(names(&out).is_empty());
    }
}
