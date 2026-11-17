use super::*;

// The service refuses any secret that is not a sealed memfd, so a missing seal
// would make every password and passphrase fail.
#[test]
fn secrets_travel_in_sealed_memory_files() {
    use rustix::fs::SealFlags;
    use std::io::{Read, Write};
    let fd = sealed(b"correct horse").expect("sealed secret");
    let seals = rustix::fs::fcntl_get_seals(&fd).expect("seals");
    assert!(
        seals.contains(SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)
    );
    let mut file = std::fs::File::from(fd);
    let mut text = String::new();
    file.read_to_string(&mut text).expect("read from the start");
    assert_eq!(text, "correct horse");
    assert!(
        file.write_all(b"x").is_err(),
        "the secret cannot be changed"
    );

    assert!(sealed(&[]).is_ok(), "an empty file means no password");
    assert!(sealed(&[b'a'; SECRET_LIMIT + 1]).is_err());
}

// Error replies that need the person (a password, a share, SMB1) must open
// the matching prompt with the data it needs; anything else stays a message.
#[test]
fn service_errors_become_the_matching_prompts() {
    let name = |kind: &str| format!("{ERROR_PREFIX}{kind}");
    let shares = method_error(
        &name("ChooseShare"),
        r#"{"id": "net-9", "shares": [
            {"name": "photos", "comment": "Family", "uri": "smb://nas/photos"},
            {"name": "broken", "comment": "", "uri": ""}], "extra": 1}"#
            .to_owned(),
    );
    let ServiceError::ChooseShare(request) = shares else {
        panic!("a share picker: {shares:?}");
    };
    assert_eq!(request.id, "net-9");
    assert_eq!(
        request.shares.len(),
        1,
        "a share without an address cannot be chosen"
    );
    assert_eq!(request.shares[0].uri, "smb://nas/photos");
    assert_eq!(request.shares[0].comment, "Family");

    assert!(matches!(
        method_error(&name("ProtocolMismatch"), "Only SMB1".to_owned()),
        ServiceError::ProtocolMismatch(message) if message == "Only SMB1"
    ));
    assert!(matches!(
        method_error(
            &name("NeedsCredentials"),
            r#"{"id": "net-9", "username": "alice", "domain": "HOME", "can_remember": true, "retry": true}"#
                .to_owned()
        ),
        ServiceError::NeedsCredentials(request)
            if request.username == "alice" && request.domain == "HOME" && request.retry
    ));
    assert!(matches!(
        method_error(
            &name("HostKeyChanged"),
            r#"{"id": "net-2", "host": "box", "fingerprints": ["SHA256:abc"]}"#.to_owned()
        ),
        ServiceError::HostKey(request) if request.changed && request.fingerprints == ["SHA256:abc"]
    ));
    assert!(matches!(
        method_error(&name("Busy"), "Files are open in Text".to_owned()),
        ServiceError::Busy(message) if message == "Files are open in Text"
    ));
    // A malformed body or a foreign error never opens a prompt.
    assert!(matches!(
        method_error(&name("ChooseShare"), "not json".to_owned()),
        ServiceError::Failed(_)
    ));
    assert!(matches!(
        method_error("org.freedesktop.DBus.Error.NoReply", "secret".to_owned()),
        ServiceError::Failed(message) if !message.contains("secret")
    ));
}
