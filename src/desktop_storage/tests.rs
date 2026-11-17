use super::*;

#[test]
fn mountinfo_handles_optional_fields_escapes_and_read_only_options() {
    use std::os::unix::ffi::OsStrExt;
    let entry = helper::parse_entry(
        b"200 10 0:42 / /home/u/Tresorit\\040D\xff ro shared:4 master:1 - fuse.tresorit Tresorit rw,user_id=1000",
    )
    .expect("mount entry");
    assert_eq!(
        entry.mount.as_os_str().as_bytes(),
        b"/home/u/Tresorit D\xff"
    );
    assert!(entry.options.split(',').any(|option| option == "ro"));
    assert!(entry.options.contains("user_id=1000"));
    assert!(helper::parse_entry(b"broken mount record").is_none());
}
