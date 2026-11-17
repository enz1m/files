use super::*;
// Explicit, so the tests do not rely on the parent's private `use`.
use enzui_core::form::SecretText;

// Typed credential handoff through `MountCredentials`,
// `Mounter::mount_authenticated` and `AttachState::{backend_can_carry_password,
// attach_blocker, mount_credentials, set_password}`: no process-visible surface
// (URI, argv, env, log, error, dialog status) ever contains the plaintext.

const CORRECT_PASSWORD: &str = "correct horse battery staple";

#[test]
fn correct_secret_mounts_uri_and_records_scrubbed_handoff() {
    let mounter = AuthenticatingRecordingMounter::expecting(CORRECT_PASSWORD);
    mounter.set_current_generation(7);
    let creds = MountCredentials::new(
        7,
        NetworkProtocol::Smb,
        "fileserver".to_owned(),
        "private".to_owned(),
        String::new(),
        "alice".to_owned(),
        SecretText::new(CORRECT_PASSWORD.to_owned()),
    );
    let cancel = OperationCancel::default();
    let result = mounter.mount_authenticated(creds, &cancel);
    let uri = result.expect("correct password mounts");
    assert_eq!(uri, "smb://alice@fileserver/private");

    let record = mounter.record();
    assert_eq!(record.call_count, 1);
    assert_eq!(record.last_uri.as_deref(), Some(uri.as_str()));
    assert_eq!(record.last_username.as_deref(), Some("alice"));
    assert_eq!(record.last_generation, Some(7));
    assert_eq!(record.last_password_matched, Some(true));
    assert_eq!(record.last_password_len, Some(CORRECT_PASSWORD.len()));
    assert!(record.last_error.is_none());

    for s in [
        &record.last_uri,
        &record.last_result_uri,
        &record.last_error,
    ]
    .into_iter()
    .flatten()
    {
        assert!(!s.contains(CORRECT_PASSWORD), "leaked in surface: {s}");
    }
    for entry in &record.log_capture {
        assert!(!entry.contains(CORRECT_PASSWORD), "leaked in log: {entry}");
    }
    for argv in &record.argv_capture {
        for arg in argv {
            assert!(!arg.contains(CORRECT_PASSWORD), "leaked in argv: {arg}");
        }
    }
    for env in &record.env_capture {
        for (_, v) in env {
            assert!(!v.contains(CORRECT_PASSWORD), "leaked in env: {v}");
        }
    }
}

#[test]
fn wrong_secret_fails_without_leaking_plaintext() {
    let mounter = AuthenticatingRecordingMounter::expecting(CORRECT_PASSWORD);
    mounter.set_current_generation(1);
    let wrong = "tr0ub4dor&3";
    let creds = MountCredentials::new(
        1,
        NetworkProtocol::Smb,
        "fileserver".to_owned(),
        "private".to_owned(),
        String::new(),
        "alice".to_owned(),
        SecretText::new(wrong.to_owned()),
    );
    let cancel = OperationCancel::default();
    let result = mounter.mount_authenticated(creds, &cancel);
    let err = result.unwrap_err();
    assert_eq!(err, "Authentication failed", "a fixed generic error");

    let record = mounter.record();
    assert_eq!(record.last_password_matched, Some(false));
    // The record exposes `last_password_len` deliberately; the two lengths
    // differ, so a length oracle is acceptable.
    for candidate in [CORRECT_PASSWORD, wrong] {
        assert!(!err.contains(candidate), "error leaked: {err}");
        for entry in &record.log_capture {
            assert!(!entry.contains(candidate), "log leaked: {entry}");
        }
        for s in [
            &record.last_uri,
            &record.last_result_uri,
            &record.last_error,
        ]
        .into_iter()
        .flatten()
        {
            assert!(!s.contains(candidate), "surface leaked: {s}");
        }
    }
}

#[test]
fn owned_cancellation_stops_pending_authenticated_mount_and_retires_credentials() {
    let mounter = AuthenticatingRecordingMounter::expecting(CORRECT_PASSWORD);
    mounter.set_current_generation(3);
    mounter.cancel_at_call_start();
    let creds = MountCredentials::new(
        3,
        NetworkProtocol::Smb,
        "fileserver".to_owned(),
        "private".to_owned(),
        String::new(),
        "alice".to_owned(),
        SecretText::new(CORRECT_PASSWORD.to_owned()),
    );
    let cancel = OperationCancel::default();
    let result = mounter.mount_authenticated(creds, &cancel);
    assert_eq!(result.unwrap_err(), "Network operation cancelled");

    let record = mounter.record();
    assert_eq!(record.call_count, 1);
    assert!(
        record.last_password_len.is_none(),
        "the fixture must not observe the password length on the \
         cancel path; got {:?}",
        record.last_password_len,
    );
    assert!(
        record.last_password_matched.is_none(),
        "the cancel path must not compare bytes"
    );
    // The fixture trips the caller-owned cancel token itself.
    assert!(cancel.is_cancelled());
}

#[test]
fn stale_generation_callback_is_rejected_before_password_is_read() {
    let mounter = AuthenticatingRecordingMounter::expecting(CORRECT_PASSWORD);
    mounter.set_current_generation(9);
    let creds = MountCredentials::new(
        8,
        NetworkProtocol::Smb,
        "fileserver".to_owned(),
        "private".to_owned(),
        String::new(),
        "alice".to_owned(),
        SecretText::new(CORRECT_PASSWORD.to_owned()),
    );
    let cancel = OperationCancel::default();
    let result = mounter.mount_authenticated(creds, &cancel);
    assert_eq!(result.unwrap_err(), "Stale credential generation");

    let record = mounter.record();
    assert_eq!(record.call_count, 1);
    assert_eq!(record.last_generation, Some(8));
    assert!(record.last_password_len.is_none(), "never compared");
    assert!(record.last_password_matched.is_none(), "never compared");
    for entry in &record.log_capture {
        assert!(!entry.contains(CORRECT_PASSWORD), "log leaked: {entry}");
    }
}

// These tests emit `ask-password` on a real `gio::MountOperation` and read
// back its state; no socket, mount table or `MainContext` ownership is needed.

fn hanging_driver_with_release() -> (
    MountDriver,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let released_driver = std::sync::Arc::clone(&released);
    let observed_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_calls_driver = std::sync::Arc::clone(&observed_calls);
    let driver: MountDriver = std::sync::Arc::new(move |_uri, credentials, _cancel| {
        observed_calls_driver.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Ignoring `OperationCancel` keeps the credentials captured through the
        // grace wait, so the caller must detach.
        loop {
            if released_driver.load(std::sync::atomic::Ordering::Acquire) {
                drop(credentials);
                return Err("released by test".to_owned());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    (driver, released, observed_calls)
}

#[test]
fn gio_backend_bounds_stuck_worker_and_caps_admissions() {
    let (driver, released, observed) = hanging_driver_with_release();
    let backend = GioBackend::with_test_config(
        vec!["smb".to_owned()],
        driver,
        std::time::Duration::from_millis(200), // outer_timeout
        std::time::Duration::from_millis(200), // grace
    );

    let make_creds = || {
        MountCredentials::new(
            1,
            NetworkProtocol::Smb,
            "fileserver".to_owned(),
            "private".to_owned(),
            String::new(),
            "alice".to_owned(),
            SecretText::new(CORRECT_PASSWORD.to_owned()),
        )
    };

    let cancel1 = OperationCancel::default();
    let start = std::time::Instant::now();
    let err1 = backend
        .mount_authenticated(make_creds(), &cancel1)
        .unwrap_err();
    let elapsed = start.elapsed();

    // A `handle.join()` after the grace runs out would block on the hung driver.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "caller returned within budget: elapsed={elapsed:?}",
    );
    assert_eq!(
        observed.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "driver was invoked exactly once",
    );

    assert!(
        err1.contains("did not respond within"),
        "timeout disclosure: {err1}",
    );
    assert!(
        err1.contains("captured MountCredentials"),
        "disclosure names captured credentials: {err1}",
    );
    assert!(
        err1.contains("remain in memory"),
        "disclosure states credentials remain (no false wipe claim): {err1}",
    );
    // "credentials were dropped" would be false while the worker holds them.
    assert!(
        !err1.contains("were dropped"),
        "disclosure must not claim credentials were dropped: {err1}",
    );
    assert!(
        !err1.contains(CORRECT_PASSWORD),
        "no plaintext in timeout disclosure: {err1}",
    );

    assert_eq!(
        backend.live_orphan_count(),
        1,
        "the stuck worker is tracked as one live orphan",
    );

    // The cap bounds the leak to one worker and one credential-holding closure.
    let cancel2 = OperationCancel::default();
    let refuse_start = std::time::Instant::now();
    let err2 = backend
        .mount_authenticated(make_creds(), &cancel2)
        .unwrap_err();
    let refuse_elapsed = refuse_start.elapsed();
    assert!(
        refuse_elapsed < std::time::Duration::from_millis(200),
        "the admission refusal is immediate (no re-spawn): {refuse_elapsed:?}",
    );
    assert_eq!(
        observed.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the refused admission did NOT invoke the driver again",
    );
    assert!(
        err2.contains("previous authenticated mount"),
        "refusal names the orphan: {err2}",
    );
    assert!(
        err2.contains("Refusing new authenticated-mount admissions"),
        "refusal names the cap policy: {err2}",
    );
    assert!(
        err2.contains("anonymous mounts (empty password) are still accepted"),
        "refusal points at the still-available anonymous path: {err2}",
    );
    assert!(
        !err2.contains(CORRECT_PASSWORD),
        "no plaintext in refusal: {err2}",
    );

    // The orphan's `tx.send(result)` fails because the caller's receiver was
    // dropped; its `FinishGuard` still flips the flag.
    released.store(true, std::sync::atomic::Ordering::Release);

    // The driver checks the release flag every 10 ms.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline && backend.live_orphan_count() > 0 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        backend.live_orphan_count(),
        0,
        "released orphan retires and the admission cap unlocks",
    );

    // Re-arming the hang shows the cap counts orphans and is not a permanent
    // lockout.
    released.store(false, std::sync::atomic::Ordering::Release);
    let cancel3 = OperationCancel::default();
    let start3 = std::time::Instant::now();
    let err3 = backend
        .mount_authenticated(make_creds(), &cancel3)
        .unwrap_err();
    let elapsed3 = start3.elapsed();
    assert!(
        elapsed3 < std::time::Duration::from_secs(2),
        "third admission also returns within budget: {elapsed3:?}",
    );
    assert_eq!(
        observed.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the third admission DID invoke the driver (cap unlocked)",
    );
    assert!(
        err3.contains("did not respond within"),
        "third admission also times out honestly: {err3}",
    );

    released.store(true, std::sync::atomic::Ordering::Release);
    let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline2 && backend.live_orphan_count() > 0 {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        backend.live_orphan_count(),
        0,
        "second orphan also retires cleanly at test end",
    );
}

#[path = "tests/ask_password_tests.rs"]
mod ask_password_tests;
#[path = "tests/ask_question_tests.rs"]
mod ask_question_tests;
