use super::transfer_tests::TempDir;
use super::*;
use enzui_core::download::Phase;
use enzui_core::portal::PortalResponse;
use enzui_core::portal::backend::{BackendReply, MockBackend};

#[path = "tests/admission_tests.rs"]
mod admission_tests;
#[path = "tests/process_tests.rs"]
mod process_tests;

fn granted_id(decision: &DestinationDecision) -> Option<DownloadId> {
    match decision {
        DestinationDecision::Granted { id, .. } => Some(*id),
        _ => None,
    }
}

fn ready() -> DownloadsState {
    DownloadsState {
        backend: Aria2Status::Ready,
        ..Default::default()
    }
}

fn granted_torrent(root: &Path, name: &str) -> (DownloadsState, TransferJob) {
    let mut state = ready();
    let mut backend = MockBackend::new();
    backend.script(BackendReply {
        response: PortalResponse::Success,
        uris: vec![format!("file://{}", root.display())],
    });
    state.set_destination_backend(Box::new(backend));
    state.backend_program = fixture_backend_path();
    let id = state
        .request_destination(&format!("magnet:?xt=urn:btih:fixture&dn={name}"))
        .unwrap();
    assert_eq!(granted_id(&state.pump_destinations()[0]), Some(id));
    let job = state.promote().pop().expect("granted torrent is runnable");
    (state, job)
}

fn settle_torrent(
    state: DownloadsState,
    job: &TransferJob,
    events: Vec<DownloadEvent>,
    outcome: TransferOutcome,
) -> crate::App {
    let (mut app, _) = crate::App::new();
    app.model.downloads = state;
    for event in events {
        crate::file_clipboard_tests::dispatch(&mut app, crate::Msg::TransferEvent(event));
    }
    crate::file_clipboard_tests::dispatch(
        &mut app,
        crate::Msg::TransferOutcome(job.id, job.attempt, outcome),
    );
    app
}
