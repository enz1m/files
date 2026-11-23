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

#[test]
fn smb_account_is_separate_from_the_shared_folder_address() {
    let address = "smb://HOME;alex%40example@nas.local/Family%20Photos/";
    assert_eq!(
        smb_authority(address),
        Some(("nas.local", Some("HOME;alex%40example"), "Family%20Photos"))
    );
    assert_eq!(
        smb_account(address),
        Some(("alex@example".to_owned(), "HOME".to_owned()))
    );
    assert_eq!(
        smb_account("smb://nas.local/Family%20Photos"),
        Some((String::new(), String::new()))
    );
}

#[test]
fn share_browser_participates_in_folder_history_without_retaining_a_password() {
    use crate::{App, Msg, model::Screen, network::server_path};

    let (mut app, _) = App::new();
    let network = PathBuf::from(crate::services::NETWORK_VIEW_ROOT);
    let _ = app.update(Msg::NavigateScreen(Screen::Network, network.clone()));
    let activity = app.start_service_activity(
        enzui_core::operation_activity::ActivityKind::Other("Mount"),
        "Open server",
    );
    let id = storage_id("net-test");
    let path = server_path(&id);
    let _ = app.update(Msg::MountServiceFinished {
        name: "smb://nas.local".to_owned(),
        result: Err(ServiceError::ChooseShare(ShareRequest {
            id: "net-test".to_owned(),
            shares: vec![ShareChoice {
                name: "Photos".to_owned(),
                uri: "smb://nas.local/Photos".to_owned(),
                ..ShareChoice::default()
            }],
        })),
        plan: Some(ConnectPlan {
            login: Some(Login {
                username: "alex".to_owned(),
                domain: String::new(),
                password: SecretText::new("test".to_owned()),
            }),
            ..ConnectPlan::default()
        }),
        opening: true,
        id,
        activity,
    });
    assert_eq!(app.model.cwd, path);
    assert!(app.model.location_dialog.is_none());
    assert!(app.model.can_go_up());
    let _ = app.update(Msg::NavigateTo(PathBuf::from("/mnt/mock/Photos")));
    assert!(app.model.network.servers[&path].plan.login.is_none());
    assert_eq!(app.model.go_back(), Some((path, Screen::Network)));
    assert_eq!(app.model.go_back(), Some((network, Screen::Network)));
}
