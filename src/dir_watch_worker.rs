// Latest-wins ownership for potentially blocking directory-watch installs.
//
// One dedicated thread runs at most one install or retire while one mailbox
// slot retains the latest requested target. A thread stuck in one call for
// longer than [`STUCK_AFTER`] is replaced on the next request. Dropping the owner closes its
// channels and detaches an unfinished thread; if the OS call later returns,
// that thread observes closure and retires any admitted handle before exit.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use enzui_core::file_tree_watch::{DirectoryWatchService, WatchDelivery, WatchId};
use enzui_core::worker_bridge::{self, WorkerBridgeClose, WorkerReceiver, WorkerSender};
use iced::Subscription;

use crate::WatchInstallOutcome;
use crate::message::Msg;

const BRIDGE_CAPACITY: usize = 4;

#[derive(Debug, Clone)]
pub(crate) struct InstallRequest {
    pub(crate) generation: u64,
    pub(crate) target: Option<PathBuf>,
}

// A submit overwrites the sole pending slot while one request may be active.
struct Mailbox {
    state: Mutex<MailboxState>,
    wake: Condvar,
}

#[derive(Default)]
struct MailboxState {
    pending: Option<InstallRequest>,
    closed: bool,
}

// How long one kernel watch install or retire may run before the next
// request gives up on that thread and starts a fresh one.
const STUCK_AFTER: Duration = Duration::from_secs(3);

pub(crate) struct DirWatchWorker {
    installer: Mutex<Installer>,
    service: Arc<DirectoryWatchService>,
}

// One installer thread with its mailbox and its message bridge. A thread
// stuck in the kernel (a watch on a dead network mount) is detached and
// replaced, so one hung folder cannot stop live updates everywhere.
struct Installer {
    mailbox: Arc<Mailbox>,
    // Iced deduplicates recipes by id; only the first recipe takes this receiver.
    bridge_receiver: Arc<Mutex<Option<WorkerReceiver<Msg>>>>,
    bridge_close: WorkerBridgeClose<Msg>,
    thread: Option<JoinHandle<()>>,
    subscription_id: u64,
    // When the running kernel call started, or `None` while idle.
    busy_since: Arc<Mutex<Option<Instant>>>,
}

impl std::fmt::Debug for DirWatchWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirWatchWorker").finish_non_exhaustive()
    }
}

static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);

impl Installer {
    fn spawn(service: &Arc<DirectoryWatchService>) -> Self {
        let capacity = NonZeroUsize::new(BRIDGE_CAPACITY).expect("nonzero capacity");
        let (sender, receiver, close) = worker_bridge::channel::<Msg>(capacity);
        let mailbox = Arc::new(Mailbox {
            state: Mutex::new(MailboxState::default()),
            wake: Condvar::new(),
        });
        let subscription_id = NEXT_WORKER_ID.fetch_add(1, Ordering::Relaxed);
        let busy_since = Arc::new(Mutex::new(None));
        let service_thread = Arc::clone(service);
        let mailbox_thread = Arc::clone(&mailbox);
        let busy_thread = Arc::clone(&busy_since);
        // Panic unwind must close the producer side too.
        let close_in_thread = close.clone();
        let thread = std::thread::Builder::new()
            .name("files-dir-watch-installer".into())
            .spawn(move || {
                let _bridge_guard = close_in_thread.into_guard();
                run(service_thread, mailbox_thread, busy_thread, sender);
            })
            .expect("spawn files-dir-watch-installer");
        Self {
            mailbox,
            bridge_receiver: Arc::new(Mutex::new(Some(receiver))),
            bridge_close: close,
            thread: Some(thread),
            subscription_id,
            busy_since,
        }
    }

    fn stuck(&self) -> bool {
        self.busy_since
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|since| since.elapsed() >= STUCK_AFTER)
    }

    fn submit(&self, request: InstallRequest) {
        let mut state = self
            .mailbox
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.pending = Some(request);
        drop(state);
        self.mailbox.wake.notify_one();
    }
}

impl Drop for Installer {
    fn drop(&mut self) {
        {
            let mut state = self
                .mailbox
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            state.pending = None;
        }
        self.mailbox.wake.notify_all();
        self.bridge_close.close();
        // A kernel install or handle drop may never return. Detach it; the
        // thread retains the service until it can clean up and exit.
        drop(self.thread.take());
    }
}

impl DirWatchWorker {
    pub(crate) fn spawn(service: Arc<DirectoryWatchService>) -> Self {
        Self {
            installer: Mutex::new(Installer::spawn(&service)),
            service,
        }
    }

    fn installer(&self) -> std::sync::MutexGuard<'_, Installer> {
        self.installer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn submit(&self, request: InstallRequest) {
        let mut installer = self.installer();
        if installer.stuck() {
            // The old thread retires whatever it installed if its call ever
            // returns; the new one starts with nothing installed.
            *installer = Installer::spawn(&self.service);
        }
        installer.submit(request);
    }

    pub(crate) fn arm_subscription(&self) -> Subscription<Msg> {
        use iced::advanced::subscription::{Hasher, Recipe, from_recipe};
        use iced::futures::stream::BoxStream;
        struct ArmRecipe {
            id: u64,
            receiver: Arc<Mutex<Option<WorkerReceiver<Msg>>>>,
        }
        impl Recipe for ArmRecipe {
            type Output = Msg;
            fn hash(&self, state: &mut Hasher) {
                use std::hash::Hash;
                "files-app::dir-watch-installer".hash(state);
                self.id.hash(state);
            }
            fn stream(
                self: Box<Self>,
                _events: iced::advanced::subscription::EventStream,
            ) -> BoxStream<'static, Msg> {
                use iced::futures::stream::{self, StreamExt};
                let receiver = self
                    .receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                match receiver {
                    Some(rx) => rx.boxed(),
                    None => stream::empty().boxed(),
                }
            }
        }
        let installer = self.installer();
        from_recipe(ArmRecipe {
            id: installer.subscription_id,
            receiver: Arc::clone(&installer.bridge_receiver),
        })
    }

    pub(crate) fn delivery_subscription(&self) -> Subscription<Vec<WatchDelivery>> {
        self.service.subscription()
    }
}

// Marks the installer busy for the duration of one kernel call.
fn busy<T>(busy_since: &Mutex<Option<Instant>>, call: impl FnOnce() -> T) -> T {
    *busy_since
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    let result = call();
    *busy_since
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    result
}

fn run(
    service: Arc<DirectoryWatchService>,
    mailbox: Arc<Mailbox>,
    busy_since: Arc<Mutex<Option<Instant>>>,
    sender: WorkerSender<Msg>,
) {
    let mut current_installed: Option<WatchId> = None;
    loop {
        let (request, closed) = wait_for_request(&mailbox);
        if closed {
            if let Some(id) = current_installed.take() {
                service.unwatch(id);
            }
            let _ = sender.finish(None, Msg::Noop);
            return;
        }
        let Some(request) = request else {
            continue;
        };
        // Serial retire-before-install keeps resident watch ownership singular.
        if let Some(prev) = current_installed.take() {
            busy(&busy_since, || service.unwatch(prev));
        }
        if mailbox_closed(&mailbox) {
            let _ = sender.finish(None, Msg::Noop);
            return;
        }
        let outcome = match request.target {
            None => WatchInstallOutcome::NotOnDirectoryScreen,
            Some(path) => match busy(&busy_since, || service.watch(path)) {
                Ok(id) => {
                    current_installed = Some(id);
                    WatchInstallOutcome::Armed(id)
                }
                Err(refusal) => WatchInstallOutcome::Refused(refusal),
            },
        };
        // Replaced while the call hung: this thread's install is nobody's.
        if mailbox_closed(&mailbox) {
            if let Some(id) = current_installed.take() {
                service.unwatch(id);
            }
            let _ = sender.finish(None, Msg::Noop);
            return;
        }
        let msg = Msg::WatchArmed {
            generation: request.generation,
            outcome,
        };
        if sender.send_lifecycle(msg).is_err() {
            if let Some(id) = current_installed.take() {
                service.unwatch(id);
            }
            return;
        }
    }
}

fn mailbox_closed(mailbox: &Mailbox) -> bool {
    mailbox
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .closed
}

fn wait_for_request(mailbox: &Mailbox) -> (Option<InstallRequest>, bool) {
    let mut state = mailbox
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if state.closed {
            return (None, true);
        }
        if let Some(request) = state.pending.take() {
            return (Some(request), false);
        }
        state = mailbox
            .wake
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

#[cfg(test)]
pub(crate) mod test_support {}

#[cfg(test)]
mod worker_tests {}
