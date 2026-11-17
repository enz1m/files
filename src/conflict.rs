// Sticky conflict choices apply only as the queue advances and never rewrite resolved entries.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipMode {
    Cut,
    Copy,
}

// How `src` and `dst` compare, which decides whether the primary action reads
// `Merge` or `Replace`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConflictKind {
    FileVsFile,
    FolderVsFolder,
    // One of each. Treated as a replace, because there is nothing to merge.
    TypeMismatch,
}

impl ConflictKind {
    pub(crate) fn primary_label(self) -> &'static str {
        match self {
            ConflictKind::FolderVsFolder => crate::i18n::tr("Merge"),
            ConflictKind::FileVsFile | ConflictKind::TypeMismatch => crate::i18n::tr("Replace"),
        }
    }

    pub(crate) const fn is_merge(self) -> bool {
        matches!(self, ConflictKind::FolderVsFolder)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConflictChoice {
    Skip,
    Replace,
    Merge,
    // Write to a destination planned as free, never replacing what is there.
    Place,
}

impl ConflictChoice {
    pub(crate) const fn primary_for(kind: ConflictKind) -> Self {
        if kind.is_merge() {
            ConflictChoice::Merge
        } else {
            ConflictChoice::Replace
        }
    }
}

// The sticky "apply to all remaining conflicts" decision.
//
// Kept per kind, so replacing all files does not authorise merging
// directories, and neither authorises replacing a folder with a file or a
// file with a folder: each of those asks once more.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StickyDecision {
    // Applies to `FileVsFile`.
    pub(crate) files: Option<ConflictChoice>,
    // Applies to `FolderVsFolder`.
    pub(crate) folders: Option<ConflictChoice>,
    // Applies to `TypeMismatch`.
    pub(crate) mismatches: Option<ConflictChoice>,
}

impl StickyDecision {
    pub(crate) fn resolve(self, kind: ConflictKind) -> Option<ConflictChoice> {
        match kind {
            ConflictKind::FolderVsFolder => self.folders,
            ConflictKind::FileVsFile => self.files,
            ConflictKind::TypeMismatch => self.mismatches,
        }
    }

    // Records `choice` for every later conflict of `kind`.
    pub(crate) fn record(&mut self, kind: ConflictKind, choice: ConflictChoice) {
        match kind {
            ConflictKind::FolderVsFolder => self.folders = Some(choice),
            ConflictKind::FileVsFile => self.files = Some(choice),
            ConflictKind::TypeMismatch => self.mismatches = Some(choice),
        }
    }
}

// What actually happened to one queue item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ItemOutcome {
    // Moved to a free destination without a decision.
    //
    // The [`PastePlan::clear`] items; nothing was destroyed.
    Moved,
    // Copied to a free destination without a decision; the source remains.
    Copied,
    // Left alone.
    Skipped,
    // The destination file was overwritten.
    Replaced,
    // The two directory trees were unioned.
    Merged,
    // The operation was attempted and failed.
    Failed(String),
}

impl ItemOutcome {
    pub(crate) const fn changed_destination(&self) -> bool {
        matches!(
            self,
            ItemOutcome::Replaced | ItemOutcome::Merged | ItemOutcome::Moved | ItemOutcome::Copied
        )
    }
}

// What a paste would do: the part needing no decision and the conflicts.
//
// Both halves travel together so a dialog for the conflicts cannot drop the
// rest of the batch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PastePlan {
    // `(src, dst)` pairs whose destination is free.
    pub(crate) clear: Vec<(PathBuf, PathBuf)>,
    // Destinations that already exist and need a decision.
    pub(crate) conflicts: Vec<Conflict>,
    // `(src, dst, reason)` items refused outright, such as a folder pasted
    // into itself; they are reported as failures.
    pub(crate) refused: Vec<(PathBuf, PathBuf, String)>,
}

impl PastePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.clear.is_empty() && self.conflicts.is_empty() && self.refused.is_empty()
    }
}

// One resolved item, for the results list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedItem {
    pub(crate) src: PathBuf,
    pub(crate) dst: PathBuf,
    pub(crate) outcome: ItemOutcome,
}

// One conflict awaiting a decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Conflict {
    pub(crate) src: PathBuf,
    pub(crate) dst: PathBuf,
    pub(crate) kind: ConflictKind,
    // The existing destination, rendered on the `Original:` line.
    pub(crate) dst_detail: String,
    // The incoming source, rendered on the `New:` line.
    pub(crate) src_detail: String,
}

impl Conflict {
    pub(crate) fn name(&self) -> String {
        self.dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

// A paused paste: the conflict currently on screen plus everything behind it.
#[derive(Clone, Debug)]
pub(crate) struct ConflictQueue {
    // The destination directory, shown as "Destination folder".
    pub(crate) dest_dir: PathBuf,
    pub(crate) clip_mode: ClipMode,
    // The conflict on screen; `None` once the queue drains, closing the dialog.
    pub(crate) current: Option<Conflict>,
    // Conflicts not yet reached, in order.
    pub(crate) remaining: Vec<Conflict>,
    pub(crate) apply_to_all: bool,
    // The sticky decision accumulated so far.
    pub(crate) sticky: StickyDecision,
    // Everything already resolved, in resolution order.
    pub(crate) resolved: Vec<ResolvedItem>,
    pub(crate) paused: bool,
    pub(crate) cancelled: bool,
}

impl ConflictQueue {
    // Opens a queue on a whole [`PastePlan`].
    //
    // Free destinations are performed immediately and recorded as
    // [`ItemOutcome::Moved`] or [`ItemOutcome::Copied`]; conflicts queue for a
    // decision. The reducer's only constructor, so the clear half cannot be
    // dropped.
    pub(crate) fn from_plan(
        dest_dir: PathBuf,
        clip_mode: ClipMode,
        plan: PastePlan,
        perform: &mut impl FnMut(&Path, &Path, ConflictChoice, ClipMode) -> ItemOutcome,
    ) -> Self {
        let mut queue = Self::new(dest_dir, clip_mode, plan.conflicts);
        // Prepended: these precede every decision in resolution order.
        let mut moved: Vec<ResolvedItem> = Vec::with_capacity(plan.clear.len());
        for (src, dst) in plan.clear {
            // Free when planned; anything that appeared since fails the item
            // instead of being overwritten.
            let outcome = match perform(&src, &dst, ConflictChoice::Place, clip_mode) {
                ItemOutcome::Replaced => match clip_mode {
                    ClipMode::Copy => ItemOutcome::Copied,
                    ClipMode::Cut => ItemOutcome::Moved,
                },
                other => other,
            };
            moved.push(ResolvedItem { src, dst, outcome });
        }
        for (src, dst, reason) in plan.refused {
            moved.push(ResolvedItem {
                src,
                dst,
                outcome: ItemOutcome::Failed(reason),
            });
        }
        moved.extend(queue.resolved);
        queue.resolved = moved;
        queue
    }

    // Opens a queue on `conflicts`; the first becomes `current`.
    pub(crate) fn new(
        dest_dir: PathBuf,
        clip_mode: ClipMode,
        mut conflicts: Vec<Conflict>,
    ) -> Self {
        let current = if conflicts.is_empty() {
            None
        } else {
            Some(conflicts.remove(0))
        };
        Self {
            dest_dir,
            clip_mode,
            current,
            remaining: conflicts,
            apply_to_all: false,
            sticky: StickyDecision::default(),
            resolved: Vec::new(),
            paused: false,
            cancelled: false,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.current.is_some()
    }

    // The primary button's label for the current conflict, or `Replace` when empty.
    pub(crate) fn primary_label(&self) -> &'static str {
        self.current
            .as_ref()
            .map_or_else(|| crate::i18n::tr("Replace"), |c| c.kind.primary_label())
    }

    // The `1 of 3`-style position for the dialog subtitle.
    pub(crate) fn position_label(&self) -> String {
        let done = self.resolved.len();
        let total = done + usize::from(self.current.is_some()) + self.remaining.len();
        crate::i18n::format(
            "files-of-f16446b9",
            &[
                ("arg0", (done + 1).to_string()),
                ("arg1", (total.max(1)).to_string()),
            ],
        )
    }

    pub(crate) fn set_apply_to_all(&mut self, on: bool) {
        self.apply_to_all = on;
    }

    // Pauses the batch; the current conflict stays and the queue does not advance.
    pub(crate) fn pause(&mut self) {
        self.paused = true;
    }

    pub(crate) fn resume(&mut self) {
        self.paused = false;
    }

    // Abandons the rest of the batch; already resolved items stay, as this is not
    // an undo.
    pub(crate) fn cancel(&mut self) {
        self.cancelled = true;
        self.current = None;
        self.remaining.clear();
    }

    // Resolves the current conflict with `choice`, then advances.
    //
    // `perform` runs the operation and reports the outcome. Does nothing while
    // paused.
    pub(crate) fn resolve(
        &mut self,
        choice: ConflictChoice,
        perform: &mut impl FnMut(&Path, &Path, ConflictChoice, ClipMode) -> ItemOutcome,
    ) {
        if self.paused {
            return;
        }
        let Some(conflict) = self.current.take() else {
            return;
        };
        // Recorded before advancing, so it governs only later items. The checkbox
        // then resets: it describes the current decision only.
        if self.apply_to_all {
            self.sticky.record(conflict.kind, choice);
            self.apply_to_all = false;
        }
        let outcome = self.perform_one(&conflict, choice, perform);
        self.resolved.push(ResolvedItem {
            src: conflict.src,
            dst: conflict.dst,
            outcome,
        });
        self.advance(perform);
    }

    fn advance(
        &mut self,
        perform: &mut impl FnMut(&Path, &Path, ConflictChoice, ClipMode) -> ItemOutcome,
    ) {
        while let Some(next) = self.remaining.first().cloned() {
            match self.sticky.resolve(next.kind) {
                Some(choice) => {
                    self.remaining.remove(0);
                    let outcome = self.perform_one(&next, choice, perform);
                    self.resolved.push(ResolvedItem {
                        src: next.src,
                        dst: next.dst,
                        outcome,
                    });
                }
                None => {
                    self.remaining.remove(0);
                    self.current = Some(next);
                    return;
                }
            }
        }
        self.current = None;
    }

    // Runs one decision. `Skip` never touches disk; other choices go to the
    // executor with the [`ClipMode`].
    fn perform_one(
        &self,
        conflict: &Conflict,
        choice: ConflictChoice,
        perform: &mut impl FnMut(&Path, &Path, ConflictChoice, ClipMode) -> ItemOutcome,
    ) -> ItemOutcome {
        if choice == ConflictChoice::Skip {
            return ItemOutcome::Skipped;
        }
        perform(&conflict.src, &conflict.dst, choice, self.clip_mode)
    }

    // Counts top-level items whose destination was actually changed.
    pub(crate) fn completed_items(&self) -> usize {
        self.resolved
            .iter()
            .filter(|item| item.outcome.changed_destination())
            .count()
    }

    // Whether this batch wrote anything to disk, deciding whether to reload.
    //
    // Defined through [`ItemOutcome::changed_destination`] so there is one
    // definition of a write.
    pub(crate) fn changed_anything(&self) -> bool {
        self.resolved
            .iter()
            .any(|r| r.outcome.changed_destination())
    }

    pub(crate) fn summary(&self) -> String {
        if self.cancelled {
            return crate::i18n::tr("Paste cancelled").to_owned();
        }
        let moved = self
            .resolved
            .iter()
            .filter(|r| r.outcome == ItemOutcome::Moved)
            .count();
        let copied = self
            .resolved
            .iter()
            .filter(|r| r.outcome == ItemOutcome::Copied)
            .count();
        let replaced = self
            .resolved
            .iter()
            .filter(|r| r.outcome == ItemOutcome::Replaced)
            .count();
        let merged = self
            .resolved
            .iter()
            .filter(|r| r.outcome == ItemOutcome::Merged)
            .count();
        let skipped = self
            .resolved
            .iter()
            .filter(|r| r.outcome == ItemOutcome::Skipped)
            .count();
        let failed = self
            .resolved
            .iter()
            .filter(|r| matches!(r.outcome, ItemOutcome::Failed(_)))
            .count();
        let mut parts = Vec::new();
        if moved > 0 {
            parts.push(crate::i18n::format(
                "files-moved-moved-f2b69473",
                &[("moved", (moved).to_string())],
            ));
        }
        if copied > 0 {
            parts.push(crate::i18n::format(
                "files-copied-copied-9fb3b771",
                &[("copied", (copied).to_string())],
            ));
        }
        if replaced > 0 {
            parts.push(crate::i18n::format(
                "files-replaced-replaced-b3647663",
                &[("replaced", (replaced).to_string())],
            ));
        }
        if merged > 0 {
            parts.push(crate::i18n::format(
                "files-merged-merged-e3be292a",
                &[("merged", (merged).to_string())],
            ));
        }
        if skipped > 0 {
            parts.push(crate::i18n::format(
                "files-skipped-skipped-ffde23ed",
                &[("skipped", (skipped).to_string())],
            ));
        }
        if failed > 0 {
            parts.push(crate::i18n::format(
                "files-failed-failed-edcbe6ee",
                &[("failed", (failed).to_string())],
            ));
        }
        if parts.is_empty() {
            crate::i18n::tr("Nothing to paste").to_owned()
        } else {
            parts.join(", ")
        }
    }
}

// Classifies a `src` / `dst` pair. Reads the filesystem, so `services` calls it.
pub(crate) fn classify(src: &Path, dst: &Path) -> ConflictKind {
    // A link to a folder is not a folder to merge: merging through it would
    // move or write the contents of whatever it points to.
    match (is_real_dir(src), is_real_dir(dst)) {
        (true, true) => ConflictKind::FolderVsFolder,
        (false, false) => ConflictKind::FileVsFile,
        _ => ConflictKind::TypeMismatch,
    }
}

// A directory itself, not a symlink to one.
pub(crate) fn is_real_dir(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {}
