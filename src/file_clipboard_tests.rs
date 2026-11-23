use super::*;
use enzui_clipboard::{ClipboardError, FileIntent, FileTransfer};
use iced::futures::{
    StreamExt,
    executor::block_on,
    future::{BoxFuture, Either, select},
};
use iced_runtime::{Action, task};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub(crate) struct Clipboard(Arc<Mutex<ClipboardState>>);
struct ClipboardState {
    content: Option<Result<FileTransfer, ClipboardError>>,
    offers: Vec<FileTransfer>,
    reads: usize,
    refuse_offer: bool,
}

impl file_clipboard::Provider for Clipboard {
    fn read(&self) -> BoxFuture<'static, Option<Result<FileTransfer, ClipboardError>>> {
        let state = Arc::clone(&self.0);
        Box::pin(async move {
            let mut state = state.lock().unwrap();
            state.reads += 1;
            state.content.clone()
        })
    }
    fn offer(
        &self,
        transfer: FileTransfer,
    ) -> BoxFuture<'static, Option<Result<(), ClipboardError>>> {
        let state = Arc::clone(&self.0);
        Box::pin(async move {
            let mut state = state.lock().unwrap();
            if state.refuse_offer {
                return Some(Err(ClipboardError::NotFocused));
            }
            state.offers.push(transfer.clone());
            state.content = Some(Ok(transfer));
            Some(Ok(()))
        })
    }
}

impl Clipboard {
    pub(crate) fn install(app: &mut App) -> Self {
        let clipboard = Self(Arc::new(Mutex::new(ClipboardState {
            content: Some(Err(ClipboardError::NoSelection)),
            offers: Vec::new(),
            reads: 0,
            refuse_offer: false,
        })));
        app.file_clipboard = file_clipboard::Runtime::with_provider(Arc::new(clipboard.clone()));
        clipboard
    }
    pub(crate) fn set(&self, transfer: FileTransfer) {
        self.0.lock().unwrap().content = Some(Ok(transfer));
    }
}

pub(crate) fn dispatch(app: &mut App, message: Msg) -> Vec<Msg> {
    let task = app.update(message);
    drive(app, task)
}

pub(crate) fn drive(app: &mut App, initial: Task<Msg>) -> Vec<Msg> {
    let mut tasks = VecDeque::from([initial]);
    let mut messages = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(next) = tasks.pop_front() {
        let Some(mut stream) = task::into_stream(next) else {
            continue;
        };
        loop {
            let action = match block_on(select(
                stream.next(),
                futures_timer::Delay::new(deadline.saturating_duration_since(Instant::now())),
            )) {
                Either::Left((action, _)) => action,
                Either::Right(_) => panic!("clipboard reducer task did not settle"),
            };
            let Some(action) = action else {
                break;
            };
            if let Action::Output(message) = action {
                assert!(messages.len() < 200, "unbounded reducer task loop");
                messages.push(message.clone());
                tasks.push_back(app.update(message));
            }
        }
    }
    messages
}

struct Fixture {
    root: PathBuf,
    src: PathBuf,
    dst: PathBuf,
    paths: Vec<PathBuf>,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "files-clipboard-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let src = root.join("source");
        let dst = root.join("destination");
        std::fs::create_dir_all(src.join("folder")).unwrap();
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(src.join("file.txt"), b"single-file").unwrap();
        std::fs::write(src.join("folder/nested.txt"), b"nested-file").unwrap();
        let paths = vec![src.join("file.txt"), src.join("folder")];
        Self {
            root,
            src,
            dst,
            paths,
        }
    }
    fn app(&self) -> (App, Clipboard) {
        let (mut app, _) = App::new();
        app.model.cwd = self.dst.clone();
        app.model.screen = Screen::Directory;
        let clipboard = Clipboard::install(&mut app);
        (app, clipboard)
    }
    fn assert_landed(&self) {
        assert_eq!(
            std::fs::read(self.dst.join("file.txt")).unwrap(),
            b"single-file"
        );
        assert_eq!(
            std::fs::read(self.dst.join("folder/nested.txt")).unwrap(),
            b"nested-file"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn external_file_and_folder_copy_and_cut_reach_the_real_paste_pipeline() {
    for intent in [FileIntent::Copy, FileIntent::Cut] {
        let fx = Fixture::new("external");
        let (mut app, clipboard) = fx.app();
        clipboard.set(FileTransfer::new(intent, fx.paths.clone()));
        let messages = dispatch(&mut app, Msg::PasteHere);
        assert!(
            messages
                .iter()
                .any(|m| matches!(m, Msg::PasteExecuted { .. }))
        );
        assert_eq!(clipboard.0.lock().unwrap().reads, 1);
        fx.assert_landed();
        for source in &fx.paths {
            assert_eq!(source.exists(), intent == FileIntent::Copy);
        }
        assert!(!app.model.clipboard_pending);
    }
}

#[test]
fn clipboard_refusals_never_fall_back_to_a_previous_file_selection() {
    let fx = Fixture::new("refused");
    let (mut app, clipboard) = fx.app();
    for reply in [
        None,
        Some(Err(ClipboardError::NotFocused)),
        Some(Err(ClipboardError::NoSelection)),
    ] {
        clipboard.0.lock().unwrap().content = reply;
        let messages = dispatch(&mut app, Msg::PasteHere);
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, Msg::PastePlanned { .. } | Msg::PasteExecuted { .. }))
        );
        assert_eq!(std::fs::read_dir(&fx.dst).unwrap().count(), 0);
        assert!(fx.paths.iter().all(|p| p.exists()));
        assert!(!app.model.clipboard_pending);
        assert!(app.model.status_message.is_some());
    }
}

#[test]
fn a_paste_reply_cannot_follow_navigation_away_and_back() {
    let fx = Fixture::new("navigation");
    let (mut app, clipboard) = fx.app();
    clipboard.set(FileTransfer::new(FileIntent::Cut, fx.paths.clone()));
    let pending = app.update(Msg::PasteHere);
    dispatch(&mut app, Msg::NavigateTo(fx.src.clone()));
    dispatch(&mut app, Msg::NavigateTo(fx.dst.clone()));
    let messages = drive(&mut app, pending);
    assert!(
        !messages
            .iter()
            .any(|m| matches!(m, Msg::PastePlanned { .. }))
    );
    assert!(fx.paths.iter().all(|p| p.exists()));
    assert_eq!(std::fs::read_dir(&fx.dst).unwrap().count(), 0);
}

#[test]
fn folder_paste_shows_a_pending_row_and_reports_work_inside_the_folder() {
    let fx = Fixture::new("folder-progress");
    let source = fx.src.join("folder");
    let target = fx.dst.join("folder");
    let (mut app, _) = fx.app();
    let plan = services::plan_paste(&[source], &fx.dst, conflict::ClipMode::Copy);
    let activity = app.start_file_activity(ActivityKind::Copy, "Copying files", 1);
    let task = app.update(Msg::PastePlanned {
        activity,
        dest_dir: fx.dst.clone(),
        copy: true,
        plan: Box::new(plan),
    });

    assert!(
        app.model
            .pending_copy_folders
            .iter()
            .any(|folder| { folder.path == target && folder.activity == Some(activity) })
    );
    let messages = drive(&mut app, task);
    let progress = messages
        .iter()
        .position(|message| {
            matches!(message, Msg::PasteProgress(token, fraction)
                if *token == activity && *fraction > 0.0 && *fraction < 1.0)
        })
        .expect("the folder published intermediate progress");
    let completed = messages
        .iter()
        .position(|message| {
            matches!(message, Msg::PasteExecuted { activity: token, .. } if *token == activity)
        })
        .expect("the folder copy finished");
    assert!(progress < completed);
    assert_eq!(
        std::fs::read(target.join("nested.txt")).unwrap(),
        b"nested-file"
    );
    assert!(app.model.pending_copy_folders.is_empty());
    assert!(app.model.entry_for_path(&target).is_some());
}
