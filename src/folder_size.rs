// Dedicated bounded workers keep long size walks from delaying browsing; task drop cancels the walk.

use crate::message::Msg;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(crate) const REPORT_EVERY: Duration = Duration::from_millis(100);

pub(crate) const MAX_RUNNING: usize = 4;

// What a walk has counted: bytes of every non-directory entry, and every
// entry below the root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    pub(crate) bytes: u64,
    pub(crate) items: u64,
}

// A folder's size as the view shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FolderSize {
    // The walk is running; the total so far.
    Measuring(Tally),
    // The walk finished.
    Measured(Tally),
}

pub(crate) const CALCULATE_LABEL: &str = "Calculate Size";

pub(crate) const STOP_LABEL: &str = "Stop Calculating";
// The icon of [`CALCULATE_LABEL`].
pub(crate) const CALCULATE_ICON: &str = "calculator";

pub(crate) fn label(size: FolderSize) -> String {
    match size {
        FolderSize::Measured(tally) => crate::services::format_bytes(tally.bytes),
        FolderSize::Measuring(tally) => crate::i18n::format(
            "files-growing-size",
            &[("size", crate::services::format_bytes(tally.bytes))],
        ),
    }
}

pub(crate) fn items_label(items: u64) -> String {
    crate::i18n::count("files-item-count", items)
}

// One report from a running walk.
#[derive(Clone, Debug)]
pub(crate) enum MeasureEvent {
    Progress(Tally),
    // The final total, or `None` when the walk was cancelled.
    Done(Option<Tally>),
}

// Walk `root` without following symlinks, reporting the running total to
// `report` at most every [`REPORT_EVERY`]. Returns `None` as soon as `cancel`
// is set. Unreadable directories and entries are skipped, as `du` does.
pub(crate) fn walk(root: &Path, cancel: &AtomicBool, report: impl FnMut(Tally)) -> Option<Tally> {
    walk_reporting_every(root, cancel, REPORT_EVERY, report)
}

fn walk_reporting_every(
    root: &Path,
    cancel: &AtomicBool,
    every: Duration,
    mut report: impl FnMut(Tally),
) -> Option<Tally> {
    let mut tally = Tally::default();
    let mut pending = vec![root.to_path_buf()];
    let mut last_report = Instant::now();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            tally.items += 1;
            // The type comes from the directory listing itself, and the
            // metadata is read relative to the open directory without
            // following links: no path is built or resolved per file, and a
            // link counts as itself, so a link cycle cannot loop.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if let Ok(metadata) = entry.metadata() {
                tally.bytes = tally.bytes.saturating_add(metadata.len());
            }
            if last_report.elapsed() >= every {
                last_report = Instant::now();
                report(tally);
            }
        }
    }
    (!cancel.load(Ordering::Relaxed)).then_some(tally)
}

// Sets the flag when dropped, so an aborted task stops its walk.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

// Measure `path` on a thread of its own, streaming
// [`Msg::FolderMeasure`] reports for `generation`.
pub(crate) fn task(path: PathBuf, generation: u64, cancel: Arc<AtomicBool>) -> iced::Task<Msg> {
    use iced::futures::{SinkExt, StreamExt};

    let stream = iced::stream::channel(
        4,
        move |mut output: iced::futures::channel::mpsc::Sender<Msg>| async move {
            let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
            let _cancel_on_drop = CancelOnDrop(std::sync::Arc::clone(&cancel));
            let root = path.clone();
            let worker = std::thread::Builder::new()
                .name("files-folder-size".into())
                .spawn(move || {
                    let progress = tx.clone();
                    let done = walk(&root, &cancel, |tally| {
                        let _ = progress.unbounded_send(MeasureEvent::Progress(tally));
                    });
                    let _ = tx.unbounded_send(MeasureEvent::Done(done));
                });
            if worker.is_err() {
                let _ = output
                    .send(Msg::FolderMeasure {
                        path,
                        generation,
                        event: MeasureEvent::Done(None),
                    })
                    .await;
                return;
            }
            while let Some(event) = rx.next().await {
                let msg = Msg::FolderMeasure {
                    path: path.clone(),
                    generation,
                    event,
                };
                if output.send(msg).await.is_err() {
                    break;
                }
            }
        },
    );
    iced::Task::run(stream, |msg| msg)
}

// The App's running measurements: each folder's generation and cancel flag.
#[derive(Default)]
pub(crate) struct Measurements {
    running: HashMap<PathBuf, (u64, Arc<AtomicBool>)>,
    next_generation: u64,
}

impl Measurements {
    // Start measuring `path`, or `None` when it is already being measured or
    // [`MAX_RUNNING`] measurements are running.
    pub(crate) fn start(&mut self, path: &Path) -> Option<(u64, Arc<AtomicBool>)> {
        if self.running.contains_key(path) || self.running.len() >= MAX_RUNNING {
            return None;
        }
        self.next_generation += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.insert(
            path.to_path_buf(),
            (self.next_generation, std::sync::Arc::clone(&cancel)),
        );
        Some((self.next_generation, cancel))
    }

    // Stop measuring `path`; whether a measurement was running.
    pub(crate) fn cancel(&mut self, path: &Path) -> bool {
        self.running
            .remove(path)
            .map(|(_, cancel)| cancel.store(true, Ordering::Relaxed))
            .is_some()
    }

    // Whether `generation` is the live measurement of `path`.
    pub(crate) fn is_current(&self, path: &Path, generation: u64) -> bool {
        self.running
            .get(path)
            .is_some_and(|(live, _)| *live == generation)
    }

    pub(crate) fn finish(&mut self, path: &Path) {
        self.running.remove(path);
    }

    // Cancel every measurement `keep` rejects.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Path) -> bool) {
        self.running.retain(|path, (_, cancel)| {
            let kept = keep(path);
            if !kept {
                cancel.store(true, Ordering::Relaxed);
            }
            kept
        });
    }

    pub(crate) fn len(&self) -> usize {
        self.running.len()
    }
}
