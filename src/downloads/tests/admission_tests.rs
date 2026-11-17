// Admission: portal grants, enqueue, staging, and torrents.

use super::*;

#[test]
fn no_download_is_admitted_or_written_before_a_portal_grant() {
    use enzui_core::portal::PortalResponse;
    use enzui_core::portal::backend::BackendReply;

    let root = TempDir::new("portal-grant-1916");
    let selected = root.0.join("chosen by user.iso");
    let mut backend = MockBackend::new();
    backend.script(BackendReply {
        response: PortalResponse::Success,
        uris: vec![format!("file://{}", selected.display()).replace(' ', "%20")],
    });
    let mut state = ready();
    state.set_destination_backend(Box::new(backend));

    let requested = state
        .request_destination("https://example.invalid/not-the-filename.iso")
        .expect("native picker starts");
    assert_eq!(state.len(), 0, "requesting is not queue admission");
    assert!(!selected.exists(), "opening a picker must not write");
    assert!(state.promote().is_empty(), "no grant means no worker");

    let decisions = state.pump_destinations();
    assert_eq!(decisions.len(), 1);
    assert_eq!(granted_id(&decisions[0]), Some(requested));
    assert!(
        selected.is_file(),
        "the granted name is atomically reserved"
    );
    assert_eq!(state.len(), 1, "the grant admits exactly one queue row");
    let jobs = state.promote();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].destination, selected);
}

#[test]
fn preexisting_unowned_staging_directory_is_never_claimed_or_deleted() {
    let root = TempDir::new("unowned-stage");
    let mut state = ready();
    let mut backend = MockBackend::new();
    backend.script(enzui_core::portal::backend::BackendReply {
        response: enzui_core::portal::PortalResponse::Success,
        uris: vec![format!("file://{}", root.0.display())],
    });
    state.set_destination_backend(Box::new(backend));
    let id = state
        .request_destination("magnet:?xt=urn:btih:abc&dn=../../Documents")
        .unwrap();
    let controlled = root.0.join(".files-app-transfers");
    enzui_core::secure_dir::ensure_private_dir(&controlled).unwrap();
    let session = controlled.join(&state.staging_namespace);
    enzui_core::secure_dir::ensure_private_dir(&session).unwrap();
    let stage = session.join(format!("torrent-{}", id.0));
    enzui_core::secure_dir::ensure_private_dir(&stage).unwrap();
    std::fs::write(stage.join("user-file"), b"must survive").unwrap();
    assert!(
        matches!(state.pump_destinations().as_slice(), [DestinationDecision::Refused(refused, _)] if *refused == id)
    );
    assert_eq!(state.len(), 0);
    assert!(state.promote().is_empty());
    assert!(state.stage_cleanup_job(id, Attempt::FIRST).is_none());
    drop(state);
    assert_eq!(
        std::fs::read(stage.join("user-file")).unwrap(),
        b"must survive"
    );
    assert!(stage.exists());
}

#[cfg(unix)]
#[test]
fn redirected_session_is_rejected_without_writing_outside_the_root() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new("redirect-root");
    let outside = TempDir::new("redirect-outside");
    std::fs::set_permissions(
        &outside.0,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let mut state = ready();
    let mut backend = MockBackend::new();
    backend.script(enzui_core::portal::backend::BackendReply {
        response: enzui_core::portal::PortalResponse::Success,
        uris: vec![format!("file://{}", root.0.display())],
    });
    state.set_destination_backend(Box::new(backend));
    let id = state
        .request_destination("magnet:?xt=urn:btih:abc&dn=safe")
        .unwrap();
    let controlled = root.0.join(".files-app-transfers");
    enzui_core::secure_dir::ensure_private_dir(&controlled).unwrap();
    symlink(&outside.0, controlled.join(&state.staging_namespace)).unwrap();
    assert!(
        matches!(state.pump_destinations().as_slice(), [DestinationDecision::Refused(refused, _)] if *refused == id)
    );
    assert_eq!(state.len(), 0);
    assert!(state.promote().is_empty());
    assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
}

#[test]
fn portal_selected_existing_file_is_not_claimed_or_overwritten() {
    use enzui_core::portal::PortalResponse;
    use enzui_core::portal::backend::BackendReply;

    let root = std::env::temp_dir().join(format!("files-http-owned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let existing = root.join("report.pdf");
    std::fs::write(&existing, b"user content").unwrap();
    let uri = format!("file://{}", existing.display());
    let mut backend = MockBackend::new();
    backend.script(BackendReply {
        response: PortalResponse::Success,
        uris: vec![uri],
    });

    let mut state = ready();
    state.set_destination_backend(Box::new(backend));
    state
        .request_destination("https://example.invalid/report.pdf")
        .unwrap();
    let decisions = state.pump_destinations();

    assert!(matches!(
        decisions.as_slice(),
        [DestinationDecision::Refused(_, _)]
    ));
    assert_eq!(state.len(), 0, "a failed reservation is never enqueued");
    assert_eq!(std::fs::read(&existing).unwrap(), b"user content");
    assert!(state.promote().is_empty());
    let _ = std::fs::remove_dir_all(root);
}
