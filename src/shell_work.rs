// Bounded workers keep synchronous I/O off the UI executor; full queues wait to preserve owned inputs.
//
// Work is split across pools by kind, so a stall in one kind cannot starve
// the others: folder listings ([`Pool::Listing`]), long background walks and
// thumbnail decodes ([`Pool::Background`]), and everything else
// ([`Pool::Shell`]). A hung network mount can block a worker inside a system
// call; with separate pools it holds a listing worker, not the one a launch,
// a preview or a rename is waiting for.

use enzui_core::blocking_task::{BlockingTaskError, BlockingTaskOwner};
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::OnceLock;

// Calls admitted behind the running ones, per pool; further callers wait.
const QUEUE: NonZeroUsize = NonZeroUsize::new(16).unwrap();
const SHELL_WORKERS: NonZeroUsize = NonZeroUsize::new(4).unwrap();
const LISTING_WORKERS: NonZeroUsize = NonZeroUsize::new(3).unwrap();
const BACKGROUND_WORKERS: NonZeroUsize = NonZeroUsize::new(4).unwrap();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pool {
    // Launches, previews, properties, renames and other short calls.
    Shell,
    // Directory listings.
    Listing,
    // Recursive search walks and thumbnail decodes.
    Background,
}

impl Pool {
    const fn workers(self) -> NonZeroUsize {
        match self {
            Pool::Shell => SHELL_WORKERS,
            Pool::Listing => LISTING_WORKERS,
            Pool::Background => BACKGROUND_WORKERS,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Pool::Shell => "files-shell",
            Pool::Listing => "files-listing",
            Pool::Background => "files-background",
        }
    }

    // The pool's owner, or `None` when its worker threads could not start.
    fn owner(self) -> Option<&'static BlockingTaskOwner> {
        static SHELL: OnceLock<Option<BlockingTaskOwner>> = OnceLock::new();
        static LISTING: OnceLock<Option<BlockingTaskOwner>> = OnceLock::new();
        static BACKGROUND: OnceLock<Option<BlockingTaskOwner>> = OnceLock::new();
        let cell = match self {
            Pool::Shell => &SHELL,
            Pool::Listing => &LISTING,
            Pool::Background => &BACKGROUND,
        };
        cell.get_or_init(|| BlockingTaskOwner::new(self.label(), self.workers(), QUEUE).ok())
            .as_ref()
    }
}

// Runs `work` on the shell pool and resolves to its value.
//
// A panic in `work` resumes on the awaiting task. If the owner stops first, the
// future stays pending, like an aborted task. Without an owner the call runs
// where it is awaited.
pub(crate) fn run<T, W>(work: W) -> impl Future<Output = T> + Send + 'static
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
{
    run_cancellable(Pool::Shell, move |_| work())
}

// Runs `work` on `pool`, handing it a probe that turns true once the awaiting
// future is dropped (an aborted task, a superseded search) or the pool shuts
// down. Long walks check it between steps and return early; work still
// queued when its future is dropped never starts.
pub(crate) fn run_cancellable<T, W>(pool: Pool, work: W) -> impl Future<Output = T> + Send + 'static
where
    T: Send + 'static,
    W: FnOnce(&dyn Fn() -> bool) -> T + Send + 'static,
{
    enum Route<W, J> {
        Inline(W),
        Owned(J),
    }
    let route = match pool.owner() {
        Some(owner) => {
            Route::Owned(owner.submit_waiting(0, move |context| work(&|| context.is_cancelled())))
        }
        None => Route::Inline(work),
    };
    async move {
        match route {
            Route::Inline(work) => work(&|| false),
            Route::Owned(job) => match job.await.into_result() {
                Ok(value) => value,
                Err(BlockingTaskError::Panicked) => {
                    std::panic::resume_unwind(Box::new("files shell work panicked"))
                }
                Err(_) => std::future::pending().await,
            },
        }
    }
}

// Resolves after `duration`, on any executor (the app's and the tests'), by
// parking a short-lived thread rather than borrowing a runtime timer.
pub(crate) fn delay(duration: std::time::Duration) -> impl Future<Output = ()> + Send + 'static {
    let (done, wait) = iced::futures::channel::oneshot::channel::<()>();
    let _ = std::thread::Builder::new()
        .name("files-delay".to_owned())
        .spawn(move || {
            std::thread::sleep(duration);
            let _ = done.send(());
        });
    async move {
        let _ = wait.await;
    }
}
