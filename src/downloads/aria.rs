// Aria2 backend: telemetry, process management, and probes.

use super::*;

pub(super) fn run_aria2_torrent(
    job: &TransferJob,
    emit: impl FnMut(DownloadEvent),
    telemetry: impl FnMut(TorrentTelemetry),
) -> TransferOutcome {
    // The program is carried on the job as data (snapshotted at construction),
    // so a running transfer never consults the process environment.
    let program = job.backend_program.clone();
    run_aria2_torrent_with_program(job, &program, emit, telemetry)
}

// The deterministic backend stand-in for sanctioned visual captures
// (`files-app/tests/fixtures/aria2c-fixture`), located through the
// compile-time manifest dir so nothing reads `std::env`. Used only on the
// `downloads-torrent-fixture` boot path.
pub(super) fn fixture_backend_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aria2c-fixture")
}

// Session keys forwarded to the aria2c child under `env_clear`.
const ARIA2C_SESSION_KEYS: &[&str] = &[
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "LANG",
    "LC_ALL",
];

const ARIA2C_TOTAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
// Idle deadline: no stdout progress within this window -> stall kill.
const ARIA2C_IDLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);
// Stdout capture limit for the 24 h / 1 Hz readout policy (64 MiB).
const ARIA2C_STDOUT_CAP: usize = 64 << 20;

const ARIA2C_STDERR_CAP: usize = 64 << 10;
// Maximum bytes in a single pending stdout line before discard.
pub(crate) const ARIA2C_LINE_CAP: usize = 512;
// Maximum bounded stderr ring entries.
pub(crate) const ARIA2C_STDERR_RING: usize = 8;

// Resolve an owner-private cwd (0700, per-uid). No shared-temp fallback.
fn aria2c_private_cwd() -> Result<PathBuf, String> {
    enzui_core::runtime_dir::runtime_dir()
        .map(|rd| rd.path().to_path_buf())
        .map_err(|e| {
            crate::i18n::format(
                "files-owner-private-runtime-directory-unavailable-e-5e275edc",
                &[("e", (e).to_string())],
            )
        })
}

pub(super) fn run_aria2_torrent_with_program(
    job: &TransferJob,
    program: &Path,
    mut emit: impl FnMut(DownloadEvent),
    mut telemetry: impl FnMut(TorrentTelemetry),
) -> TransferOutcome {
    use enzui_core::cancellation::{CancelCause, CancelToken};
    use enzui_core::download::transfer::TransferIntent;
    use enzui_services::process_transport::{
        self, OutputStream, ProcessOutputChunk, ProcessRequest, StdinPolicy,
    };
    use std::collections::VecDeque;

    if let Err(error) = obtain_stage_path(job) {
        emit(DownloadEvent::failed(job.id, job.attempt, error.message()));
        return TransferOutcome::Failed {
            error,
            bytes_on_disk: 0,
        };
    }

    let cwd = match aria2c_private_cwd() {
        Ok(p) => p,
        Err(msg) => {
            let error = TransferError::Transport {
                category: TransportCategory::Other,
                detail: msg,
            };
            return torrent_failure(job, error, &mut emit);
        }
    };

    let session_vars: Vec<(std::ffi::OsString, std::ffi::OsString)> = ARIA2C_SESSION_KEYS
        .iter()
        .filter_map(|key| std::env::var_os(key).map(|v| (std::ffi::OsString::from(*key), v)))
        .collect();

    let mut request = ProcessRequest::new(
        program.as_os_str().to_os_string(),
        StdinPolicy::Null,
        ARIA2C_STDOUT_CAP,
        ARIA2C_STDERR_CAP,
        ARIA2C_TOTAL_DEADLINE,
    )
    .envs(session_vars)
    .current_dir(cwd);

    match job.torrent_policy {
        TorrentPolicy::DownloadOnly => {
            request = request.arg("--seed-time=0");
        }
    }
    request = request
        .arg("--follow-torrent=true")
        .arg("--check-integrity=true")
        .arg("--allow-overwrite=false")
        .arg("--auto-file-renaming=true")
        .arg("--summary-interval=1")
        .arg("--console-log-level=notice")
        .arg("--show-console-readout=true")
        .arg("--dir")
        .arg(job.destination.as_os_str().to_os_string());
    if job.download_limit_kib > 0 {
        request = request.arg(format!("--max-download-limit={}K", job.download_limit_kib));
    }
    if job.upload_limit_kib > 0 {
        request = request.arg(format!("--max-upload-limit={}K", job.upload_limit_kib));
    }
    // A torrent is sustained network and disk work: run aria2 in its own
    // background scope, not at the window's priority.
    request = request
        .arg(job.uri.as_str())
        .background(process_transport::BackgroundScope::worker("files-torrent"));

    let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel::<ProcessOutputChunk>(128);

    let token = CancelToken::new();
    let helper_token = token.share();

    // Spawn failure is visible before any child exists.
    let helper_handle = match std::thread::Builder::new()
        .name("files-aria-transport".to_owned())
        .spawn(move || process_transport::run_streaming(request, &helper_token, chunk_tx))
    {
        Ok(h) => h,
        Err(e) => {
            let error = TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::format(
                    "files-could-not-spawn-aria2c-transport-thread-bf375494",
                    &[("e", (e).to_string())],
                ),
            };
            return torrent_failure(job, error, &mut emit);
        }
    };

    let mut stdout_pending = Vec::<u8>::new();
    let mut stdout_line_overflowed = false;
    let mut stderr_pending = Vec::<u8>::new();
    let mut stderr_line_overflowed = false;
    let mut stderr_ring: VecDeque<String> = VecDeque::with_capacity(ARIA2C_STDERR_RING);
    let mut errors: VecDeque<String> = VecDeque::with_capacity(ARIA2C_STDERR_RING);
    let mut reported_total = false;
    // Only valid telemetry resets the idle timer, so a child spamming
    // warnings cannot evade the stall deadline.
    let mut last_progress = std::time::Instant::now();
    let mut cancel_cause: Option<CancelCause> = None;

    loop {
        match job.control.intent() {
            TransferIntent::Pause | TransferIntent::Cancel => {
                token.cancel(CancelCause::Cancelled);
                cancel_cause = Some(CancelCause::Cancelled);
                break;
            }
            TransferIntent::Run => {}
        }
        if last_progress.elapsed() >= ARIA2C_IDLE_DEADLINE {
            token.cancel(CancelCause::Deadline);
            cancel_cause = Some(CancelCause::Deadline);
            break;
        }

        match chunk_rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(chunk) => {
                match chunk.stream {
                    OutputStream::Stdout => {
                        for &byte in &chunk.bytes {
                            if byte == b'\n' || byte == b'\r' {
                                if stdout_line_overflowed {
                                    // The overflow notice enters the ring
                                    // directly; `retain_aria_stdout_error`'s
                                    // keyword filter would drop it.
                                    stdout_pending.clear();
                                    stdout_line_overflowed = false;
                                    if errors.len() == ARIA2C_STDERR_RING {
                                        errors.pop_front();
                                    }
                                    errors.push_back(
                                        crate::i18n::tr(
                                            "(aria2c stdout line exceeded 512 bytes; discarded)",
                                        )
                                        .to_owned(),
                                    );
                                } else if !stdout_pending.is_empty() {
                                    let line =
                                        String::from_utf8_lossy(&stdout_pending).into_owned();
                                    stdout_pending.clear();
                                    if let Some(progress) =
                                        parse_aria2_telemetry_line(job.id, job.attempt, &line)
                                    {
                                        last_progress = std::time::Instant::now();
                                        if !reported_total && progress.total.is_some() {
                                            reported_total = true;
                                            emit(DownloadEvent::ResponseMeta {
                                                id: job.id,
                                                attempt: job.attempt,
                                                meta: TransportResponseMeta {
                                                    status: 200,
                                                    content_range: None,
                                                    response_validator: None,
                                                    content_length: progress.total,
                                                    content_encoding: None,
                                                },
                                            });
                                        }
                                        if let Some(received) = progress.received {
                                            emit(DownloadEvent::Progress {
                                                id: job.id,
                                                attempt: job.attempt,
                                                received,
                                            });
                                        }
                                        telemetry(progress);
                                    } else {
                                        retain_aria_stdout_error(&mut errors, line);
                                    }
                                }
                            } else if stdout_line_overflowed {
                                // Discard until the next CR/LF.
                            } else if stdout_pending.len() >= ARIA2C_LINE_CAP {
                                stdout_line_overflowed = true;
                            } else {
                                stdout_pending.push(byte);
                            }
                        }
                    }
                    OutputStream::Stderr => {
                        for &byte in &chunk.bytes {
                            if byte == b'\n' || byte == b'\r' {
                                if stderr_line_overflowed {
                                    stderr_pending.clear();
                                    stderr_line_overflowed = false;
                                } else if !stderr_pending.is_empty() {
                                    let line =
                                        String::from_utf8_lossy(&stderr_pending).into_owned();
                                    stderr_pending.clear();
                                    let trimmed = line.trim();
                                    if !trimmed.is_empty() {
                                        if stderr_ring.len() == ARIA2C_STDERR_RING {
                                            stderr_ring.pop_front();
                                        }
                                        stderr_ring.push_back(trimmed.to_owned());
                                    }
                                }
                            } else if stderr_line_overflowed {
                                // discard until CR/LF
                            } else if stderr_pending.len() >= ARIA2C_LINE_CAP {
                                stderr_line_overflowed = true;
                            } else {
                                stderr_pending.push(byte);
                            }
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Keep a final status line that lacks a trailing newline.
                if !stdout_line_overflowed && !stdout_pending.is_empty() {
                    let line = String::from_utf8_lossy(&stdout_pending).into_owned();
                    stdout_pending.clear();
                    if let Some(progress) = parse_aria2_telemetry_line(job.id, job.attempt, &line) {
                        if !reported_total && progress.total.is_some() {
                            emit(DownloadEvent::ResponseMeta {
                                id: job.id,
                                attempt: job.attempt,
                                meta: TransportResponseMeta {
                                    status: 200,
                                    content_range: None,
                                    response_validator: None,
                                    content_length: progress.total,
                                    content_encoding: None,
                                },
                            });
                        }
                        if let Some(received) = progress.received {
                            emit(DownloadEvent::Progress {
                                id: job.id,
                                attempt: job.attempt,
                                received,
                            });
                        }
                        telemetry(progress);
                    } else {
                        retain_aria_stdout_error(&mut errors, line);
                    }
                }
                if !stderr_line_overflowed && !stderr_pending.is_empty() {
                    let line = String::from_utf8_lossy(&stderr_pending).into_owned();
                    stderr_pending.clear();
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if stderr_ring.len() == ARIA2C_STDERR_RING {
                            stderr_ring.pop_front();
                        }
                        stderr_ring.push_back(trimmed.to_owned());
                    }
                }
                break;
            }
        }
    }

    // A helper panic is transport uncertainty.
    let Ok(transport_result) = helper_handle.join() else {
        let te = TransferError::Transport {
            category: TransportCategory::Other,
            detail: crate::i18n::tr("aria2c transport thread panicked").into(),
        };
        return torrent_failure(job, te, &mut emit);
    };

    for line in &stderr_ring {
        retain_aria_stdout_error(&mut errors, line.clone());
    }

    // A Pause or Cancel that arrived after the child exited must be honored
    // before any publication.
    match job.control.intent() {
        TransferIntent::Pause => {
            let bytes = directory_bytes(&job.destination);
            emit(DownloadEvent::Paused {
                id: job.id,
                attempt: job.attempt,
                received: bytes,
            });
            telemetry(TorrentTelemetry {
                id: job.id,
                attempt: job.attempt,
                received: Some(bytes),
                state: crate::i18n::tr("Paused").into(),
                ..Default::default()
            });
            return TransferOutcome::Paused {
                bytes_written: bytes,
                bytes_on_disk: bytes,
            };
        }
        TransferIntent::Cancel => {
            emit(DownloadEvent::Cancelled {
                id: job.id,
                attempt: job.attempt,
            });
            return TransferOutcome::Cancelled {
                partial_removed: false,
            };
        }
        TransferIntent::Run => {}
    }

    match transport_result {
        Ok(output) => {
            if !output.status.success() {
                let stderr_text = String::from_utf8_lossy(&output.stderr);
                let detail = stderr_text.trim();
                let fallback = errors.into_iter().collect::<Vec<_>>().join(" \u{00b7} ");
                let msg = if !detail.is_empty() {
                    crate::i18n::format(
                        "files-aria2c-failed-35d1e6b2",
                        &[("arg0", (detail.lines().next().unwrap_or(detail)).to_owned())],
                    )
                } else if !fallback.is_empty() {
                    crate::i18n::format(
                        "files-aria2c-failed-fallback-1797f3e4",
                        &[("fallback", (fallback))],
                    )
                } else {
                    crate::i18n::tr(
                        "aria2c exited unsuccessfully; verify the torrent and tracker availability",
                    )
                    .into()
                };
                let error = TransferError::Transport {
                    category: TransportCategory::Other,
                    detail: msg,
                };
                return torrent_failure(job, error, &mut emit);
            }
            match publish_owned_payload(job) {
                Ok(
                    PublicationOutcome::Complete { bytes }
                    | PublicationOutcome::CompleteCleanupPending { bytes },
                ) => {
                    emit(DownloadEvent::Progress {
                        id: job.id,
                        attempt: job.attempt,
                        received: bytes,
                    });
                    emit(DownloadEvent::Finished {
                        id: job.id,
                        attempt: job.attempt,
                    });
                    telemetry(TorrentTelemetry {
                        id: job.id,
                        attempt: job.attempt,
                        received: Some(bytes),
                        total: Some(bytes),
                        state: crate::i18n::tr("Complete").into(),
                        ..Default::default()
                    });
                    TransferOutcome::Completed {
                        bytes_written: bytes,
                        bytes_on_disk: bytes,
                        verified: true,
                    }
                }
                Ok(PublicationOutcome::Partial {
                    moved,
                    remaining,
                    error,
                }) => {
                    let detail = crate::i18n::format(
                        "files-published-moved-of-entries-62e69349",
                        &[
                            ("moved", (moved).to_string()),
                            ("arg0", (moved + remaining).to_string()),
                            ("arg1", (error.message())),
                        ],
                    );
                    emit(DownloadEvent::failed(job.id, job.attempt, detail.clone()));
                    TransferOutcome::Failed {
                        error: TransferError::Io {
                            category: IoFailureCategory::Unknown,
                            detail,
                        },
                        bytes_on_disk: directory_bytes(&job.destination),
                    }
                }
                Err(error) => {
                    emit(DownloadEvent::failed(job.id, job.attempt, error.message()));
                    TransferOutcome::Failed {
                        error,
                        bytes_on_disk: directory_bytes(&job.destination),
                    }
                }
            }
        }
        Err(error) => {
            map_process_error_to_outcome(job, error, cancel_cause, &mut emit, &mut telemetry)
        }
    }
}

// Retain a stdout line as a potential error diagnostic (bounded ring).
fn retain_aria_stdout_error(errors: &mut std::collections::VecDeque<String>, line: String) {
    if !line.trim().is_empty()
        && (line.contains("ERROR") || line.contains("Exception") || line.contains("failed"))
    {
        if errors.len() == ARIA2C_STDERR_RING {
            errors.pop_front();
        }
        errors.push_back(line);
    }
}

pub(crate) fn parse_aria2_telemetry_line(
    id: DownloadId,
    attempt: Attempt,
    line: &str,
) -> Option<TorrentTelemetry> {
    let start = line.find("[#")?;
    let body = line.get(start..)?.strip_prefix("[#")?.split_once(']')?.0;
    let mut out = TorrentTelemetry {
        id,
        attempt,
        state: crate::i18n::tr("Downloading").into(),
        ..Default::default()
    };
    for token in body.split_ascii_whitespace().skip(1) {
        if let Some((done, total)) = token.split_once('/') {
            out.received = parse_size_inline(done);
            out.total = parse_size_inline(total.split('(').next().unwrap_or(total));
        } else if let Some(value) = token.strip_prefix("DL:") {
            out.rate = Some(value.to_owned());
        } else if let Some(value) = token.strip_prefix("CN:") {
            out.peers = value.parse().ok();
        } else if let Some(value) = token.strip_prefix("SEED:") {
            out.peers = value.parse().ok();
            out.state = crate::i18n::tr("Seeding").into();
        }
    }
    Some(out)
}

fn parse_size_inline(raw: &str) -> Option<u64> {
    let split = raw.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let value: f64 = raw.get(..split)?.parse().ok()?;
    let unit = raw.get(split..)?.trim().to_ascii_lowercase();
    let scale = if unit.starts_with("kib") {
        1024.0
    } else if unit.starts_with("mib") {
        1024.0 * 1024.0
    } else if unit.starts_with("gib") {
        1024.0 * 1024.0 * 1024.0
    } else {
        1.0
    };
    Some((value * scale) as u64)
}

// Map a `ProcessError` to `TransferOutcome`. Cleanup uncertainty never
// publishes Cancelled/Paused/Complete.
fn map_process_error_to_outcome(
    job: &TransferJob,
    error: enzui_services::process_transport::ProcessError,
    cancel_cause: Option<enzui_core::cancellation::CancelCause>,
    emit: &mut impl FnMut(DownloadEvent),
    telemetry: &mut impl FnMut(TorrentTelemetry),
) -> TransferOutcome {
    use enzui_core::cancellation::CancelCause;
    use enzui_core::download::transfer::TransferIntent;
    use enzui_services::process_transport::ProcessErrorKind;

    match error.kind() {
        ProcessErrorKind::Cancelled(_) => match job.control.intent() {
            TransferIntent::Pause => {
                let bytes = directory_bytes(&job.destination);
                emit(DownloadEvent::Paused {
                    id: job.id,
                    attempt: job.attempt,
                    received: bytes,
                });
                telemetry(TorrentTelemetry {
                    id: job.id,
                    attempt: job.attempt,
                    received: Some(bytes),
                    state: crate::i18n::tr("Paused").into(),
                    ..Default::default()
                });
                TransferOutcome::Paused {
                    bytes_written: bytes,
                    bytes_on_disk: bytes,
                }
            }
            TransferIntent::Cancel | TransferIntent::Run => {
                if matches!(cancel_cause, Some(CancelCause::Deadline)) {
                    let te = TransferError::Transport {
                        category: TransportCategory::Timeout,
                        detail: crate::i18n::tr("aria2c idle deadline exceeded (no progress)")
                            .into(),
                    };
                    return torrent_failure(job, te, emit);
                }
                emit(DownloadEvent::Cancelled {
                    id: job.id,
                    attempt: job.attempt,
                });
                TransferOutcome::Cancelled {
                    partial_removed: false,
                }
            }
        },
        ProcessErrorKind::Deadline => {
            let te = TransferError::Transport {
                category: TransportCategory::Timeout,
                detail: crate::i18n::tr("aria2c total deadline exceeded").into(),
            };
            torrent_failure(job, te, emit)
        }
        ProcessErrorKind::Spawn => {
            let diagnostic = error
                .diagnostic()
                .unwrap_or_else(|| crate::i18n::tr("spawn failed"));
            let te = if diagnostic.contains("not found") || diagnostic.contains("No such file") {
                TransferError::BackendUnavailable {
                    backend: ARIA2C_BACKEND.into(),
                    hint: crate::i18n::tr(ARIA2C_INSTALL_HINT).into(),
                }
            } else {
                TransferError::Transport {
                    category: TransportCategory::Other,
                    detail: crate::i18n::format(
                        "files-could-not-start-aria2c-diagnostic-d89f43f0",
                        &[("diagnostic", (diagnostic).to_owned())],
                    ),
                }
            };
            torrent_failure(job, te, emit)
        }
        ProcessErrorKind::Setup => {
            let diagnostic = error
                .diagnostic()
                .unwrap_or_else(|| crate::i18n::tr("setup failed"));
            let te = TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::format(
                    "files-aria2c-process-setup-refused-diagnostic-557ee9a0",
                    &[("diagnostic", (diagnostic).to_owned())],
                ),
            };
            torrent_failure(job, te, emit)
        }
        ProcessErrorKind::OutputLimitExceeded(stream) => {
            let te = TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::format(
                    "files-aria2c-stream-exceeded-capture-limit-transfer-497f024f",
                    &[("stream", format!("{stream:?}"))],
                ),
            };
            torrent_failure(job, te, emit)
        }
        ProcessErrorKind::GroupSignal
        | ProcessErrorKind::ResidualCleanup
        | ProcessErrorKind::DirectChildCleanup => {
            let te = TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::format(
                    "files-aria2c-cleanup-uncertainty-error-5b45e162",
                    &[("error", (error).to_string())],
                ),
            };
            torrent_failure(job, te, emit)
        }
        _ => {
            let te = TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::format(
                    "files-aria2c-process-error-error-12ff8d0e",
                    &[("error", (error).to_string())],
                ),
            };
            torrent_failure(job, te, emit)
        }
    }
}
