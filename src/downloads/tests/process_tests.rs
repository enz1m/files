// Publication, fixtures, and transfer processes.

use super::*;

#[test]
fn torrent_publication_size_budget_refuses_before_moving_any_entry() {
    let root = TempDir::new("stage-size-budget");
    let (_state, job) = granted_torrent(&root.0, "budget.torrent");
    let mut path = job.destination.clone();
    for _ in 0..=PUBLISH_MAX_DEPTH {
        path.push("d");
        std::fs::create_dir(&path).unwrap();
    }
    std::fs::write(path.join("payload"), b"kept").unwrap();
    assert!(publish_owned_payload(&job).is_err());
    assert_eq!(std::fs::read(path.join("payload")).unwrap(), b"kept");
    assert!(
        !root.0.join("d").exists(),
        "unmeasured payload must not be published"
    );
    assert!(
        job.torrent_shared_stage
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .is_some()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cancelling_a_running_transfer_reaps_before_reducer_stage_cleanup() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let program = fixture_backend_path();
    assert!(crate::exe_resolver::is_executable_file(&program));
    let dir = TempDir::new("reap-on-drop");
    let (state, job) = granted_torrent(&dir.0, "running.torrent");
    let stage = job.destination.clone();
    let control = job.control.clone();
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let (live_tx, live_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let program = job.backend_program.clone();
        let mut events = Vec::new();
        let outcome = run_aria2_torrent_with_program(
            &job,
            &program,
            |event| events.push(event),
            |_: TorrentTelemetry| {
                let _ = live_tx.send(());
            },
        );
        let _ = outcome_tx.send((job, events, outcome));
    });

    // Only cancel once the backend is provably running.
    live_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the running fixture must emit a live sample before cancel");

    let start = Instant::now();
    control.cancel();
    let (job, events, outcome) = outcome_rx.recv_timeout(Duration::from_secs(5)).expect(
        "cancel must terminate the held-open backend promptly — a timeout \
         means the process group was NOT reaped and would orphan on exit",
    );
    let elapsed = start.elapsed();
    worker.join().unwrap();

    assert!(
        matches!(outcome, TransferOutcome::Cancelled { .. }),
        "a cancelled transfer must report Cancelled, got {outcome:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "reap-on-cancel took {elapsed:?}; the backend was waited out, not killed"
    );
    assert!(
        stage.exists(),
        "worker completion does not discard the cleanup capability"
    );
    let app = settle_torrent(state, &job, events, outcome);
    assert!(
        !stage.exists(),
        "the reducer must clean this owned attempt stage"
    );
    assert!(!app.model.downloads.has_unresolved_cleanup(job.id));
}

#[cfg(target_os = "linux")]
#[test]
#[allow(unsafe_code, reason = "libc getpgid/kill probes")]
fn child_guard_terminates_and_reaps_the_whole_process_group() {
    use std::process::{Command, Stdio};
    let dir = std::env::temp_dir().join(format!("files-process-group-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("descendant.pid");
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "files-app-test"]);
    command
        .arg(&pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolate_process_group(&mut command);
    let mut guard = ChildGuard::new(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !pid_file.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let descendant: i32 = std::fs::read_to_string(&pid_file)
        .expect("shell published descendant pid")
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        // SAFETY: getpgid takes the owned child PID and writes no memory.
        unsafe { libc::getpgid(guard.child.id() as i32) },
        guard.process_group
    );
    // SAFETY: getpgid takes a positive observed PID and writes no memory.
    assert_eq!(unsafe { libc::getpgid(descendant) }, guard.process_group);

    let _ = guard.terminate_and_reap();
    let running = || {
        // SAFETY: signal zero only queries the positive observed PID.
        if unsafe { libc::kill(descendant, 0) } != 0 {
            return false;
        }
        let stat = std::fs::read_to_string(format!("/proc/{descendant}/stat")).unwrap_or_default();
        stat.split_ascii_whitespace().nth(2) != Some("Z")
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while running() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!running(), "descendant survived group cleanup");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_torrent_helper_runs_in_a_background_scope_and_publishes_exact_bytes() {
    use enzui_services::process_transport::{BackgroundScope, Placement, placement};
    let root = TempDir::new("background-scope");
    let (state, job) = granted_torrent(&root.0, "scoped.torrent");
    let report = root.0.with_extension("scheduling");
    let program = root.0.with_extension("fake-aria2");
    enzui_test_support::fake_executable::write_executable(
        &program,
        &format!(
            "#!/bin/sh\n\
             while [ $# -gt 0 ]; do [ \"$1\" = --dir ] && dir=$2; shift; done\n\
             {{ grep '^0::' /proc/self/cgroup; cut -d' ' -f19 /proc/self/stat; /usr/bin/ionice -p $$; }} > '{}'\n\
             yes 0123456789abcdef | head -c 1048576 > \"$dir/payload.bin\"\n",
            report.display()
        ),
    );

    let mut events = Vec::new();
    let outcome =
        run_aria2_torrent_with_program(&job, &program, |event| events.push(event), |_| {});
    assert!(
        matches!(
            outcome,
            TransferOutcome::Completed {
                bytes_written: 1_048_576,
                ..
            }
        ),
        "{outcome:?}"
    );
    let scheduling = std::fs::read_to_string(&report).expect("the helper reported");
    let _ = std::fs::remove_file(&report);
    let _ = std::fs::remove_file(&program);
    let lines: Vec<&str> = scheduling.lines().collect();
    // Linux children inherit the launching thread, not the process leader.
    let own_nice: i64 = std::fs::read_to_string("/proc/thread-self/stat")
        .unwrap()
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .nth(16)
        .unwrap()
        .parse()
        .unwrap();
    let policy = BackgroundScope::worker("files-torrent");
    let selected = placement(&policy);
    let requested = i64::from(policy.nice());
    let expected_nice = match &selected {
        Placement::Scope { .. } => requested.max(own_nice),
        Placement::Fallback(_) => (own_nice + requested).min(19),
    }
    .to_string();
    assert_eq!(lines[1..], [expected_nice.as_str(), "idle"], "{scheduling}");
    match selected {
        Placement::Scope { slice } => assert!(
            lines[0].contains(&format!("/{slice}/files-torrent-")) && lines[0].ends_with(".scope"),
            "{scheduling}"
        ),
        Placement::Fallback(reason) => assert!(!lines[0].contains("files-torrent-"), "{reason:?}"),
    }

    let app = settle_torrent(state, &job, events, outcome);
    assert_eq!(
        app.model.downloads.queue.get(job.id).unwrap().phase,
        Phase::Complete
    );
    let expected: Vec<u8> = b"0123456789abcdef\n"
        .iter()
        .copied()
        .cycle()
        .take(1_048_576)
        .collect();
    assert_eq!(std::fs::read(root.0.join("payload.bin")).unwrap(), expected);
}

#[cfg(target_os = "linux")]
#[test]
#[allow(unsafe_code, reason = "libc getpgid/kill probes")]
fn completed_leader_keeps_identity_until_one_shot_group_cleanup() {
    use std::process::{Command, Stdio};
    let dir = std::env::temp_dir().join(format!("files-reaped-group-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("descendant.pid");
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "sleep 30 & echo $! > \"$1\"; exit 0",
        "files-app-test",
    ]);
    command
        .arg(&pid_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolate_process_group(&mut command);
    let mut guard = ChildGuard::new(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while (!pid_file.exists() || !guard.exited_preserving_identity().unwrap())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let descendant: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: getpgid takes a positive observed PID and writes no memory.
    assert_eq!(unsafe { libc::getpgid(descendant) }, guard.process_group);
    let first = guard.terminate_and_reap();
    assert!(first.is_some_and(|status| status.success()));
    assert!(guard.cleaned);
    let second = guard.terminate_and_reap();
    assert_eq!(first, second, "second cleanup must be a signal-free no-op");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    // SAFETY: signal zero queries the positive observed PID without delivering a signal.
    while unsafe { libc::kill(descendant, 0) } == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let stat = std::fs::read_to_string(format!("/proc/{descendant}/stat")).unwrap_or_default();
    assert!(stat.is_empty() || stat.split_ascii_whitespace().nth(2) == Some("Z"));
    let _ = std::fs::remove_dir_all(dir);
}
