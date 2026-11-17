// Live inventory: a monitor helper streaming snapshots, restarted with
// backoff when it dies, killed when the subscription is dropped.

use super::{Location, StorageError};
use iced::futures::{Stream, channel::mpsc};
use std::io::{BufRead, BufReader, Read};
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

pub(crate) type Update = Result<Vec<Location>, StorageError>;

#[derive(Default)]
struct State {
    stopped: bool,
    latest: Option<Update>,
    child: Option<Child>,
}

// The desktop's storage, live: the newest snapshot after each change, at
// most one pending at a time. Dropping the stream stops its helper.
pub(crate) fn stream() -> impl Stream<Item = Update> + Send + 'static {
    watch()
}

struct Watch {
    wake: mpsc::Receiver<()>,
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Stream for Watch {
    type Item = Update;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Update>> {
        match Pin::new(&mut self.wake).poll_next(cx) {
            Poll::Ready(Some(())) => {
                let update = lock(&self.shared.0).latest.take();
                if let Some(update) = update {
                    Poll::Ready(Some(update))
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.0);
        state.stopped = true;
        if let Some(child) = &mut state.child {
            let _ = child.kill();
        }
        self.shared.1.notify_all();
    }
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn watch() -> Watch {
    let (mut sender, wake) = mpsc::channel(1);
    let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
    let owner = std::sync::Arc::clone(&shared);
    let worker_sender = sender.clone();
    let spawned = std::thread::Builder::new()
        .name("enzui-storage".into())
        .spawn(move || {
            let sender = worker_sender;
            let mut backoff = 1;
            loop {
                if lock(&owner.0).stopped {
                    break;
                }
                let result = run(&owner, &sender);
                let mut state = lock(&owner.0);
                if let Some(mut child) = state.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if state.stopped {
                    break;
                }
                state.latest = Some(Err(result.err().unwrap_or_else(|| {
                    StorageError(crate::i18n::tr("The desktop storage monitor stopped").into())
                })));
                let _ = sender.clone().try_send(());
                let (state, _) = owner
                    .1
                    .wait_timeout(state, Duration::from_secs(backoff))
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.stopped {
                    break;
                }
                backoff = (backoff * 2).min(30);
            }
        });
    if spawned.is_err() {
        lock(&shared.0).latest = Some(Err(StorageError(
            crate::i18n::tr("The storage monitor could not start").into(),
        )));
        let _ = sender.try_send(());
    }
    Watch { wake, shared }
}

fn run(
    shared: &Arc<(Mutex<State>, Condvar)>,
    sender: &mpsc::Sender<()>,
) -> Result<(), StorageError> {
    let failed = |_| StorageError(crate::i18n::tr("The storage monitor could not start").into());
    let executable = std::env::current_exe().map_err(failed)?;
    let mut child = Command::new(executable)
        .arg(super::helper::MONITOR)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failed)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| StorageError(crate::i18n::tr("The storage monitor has no output").into()))?;
    {
        let mut state = lock(&shared.0);
        if state.stopped {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }
        state.child = Some(child);
    }
    let mut reader = BufReader::new(stdout);
    loop {
        let mut frame = Vec::new();
        let count = (&mut reader)
            .take(super::FRAME_LIMIT as u64 + 1)
            .read_until(b'\n', &mut frame)
            .map_err(|_| StorageError(crate::i18n::tr("The storage monitor stopped").into()))?;
        if count == 0 {
            return Ok(());
        }
        if count > super::FRAME_LIMIT {
            return Err(StorageError(
                crate::i18n::tr("The storage inventory exceeded its size limit").into(),
            ));
        }
        let locations = serde_json::from_slice(&frame).map_err(|_| {
            StorageError(crate::i18n::tr("The storage inventory was invalid").into())
        })?;
        lock(&shared.0).latest = Some(Ok(locations));
        let _ = sender.clone().try_send(());
    }
}

#[cfg(test)]
mod tests {}
