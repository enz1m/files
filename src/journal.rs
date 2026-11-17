// Session-only bounded Undo/Redo journal for settled Rename,
// Trash and Move operations.
//
// Absolute paths never leave the process; a restart yields an
// empty history. Every inverse re-verifies the recorded
// `(dev, ino, kind)` identity under nofollow metadata and refuses
// to overwrite. Copy, Merge, Replaced, permanent-delete, mount,
// failed and cancelled operations are outside the journal.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// Maximum live undo entries retained.
pub(crate) const MAX_JOURNAL_ENTRIES: usize = 128;

pub(crate) const MAX_ENTRY_METADATA_BYTES: usize = 4096;

// Fixed scalar overhead attributed to every admitted entry, in
// bytes: three `u64`s on [`JournalEntry`], the largest per-variant
// scalar payload (two [`InodeIdent`]s for Trash), and the
// [`OpRecord`] discriminant.
pub(crate) const SCALAR_OVERHEAD_BYTES: usize = 3 * 8 + 2 * 3 * 8 + 8;

// Reason [`OpJournal::record`] refused to admit a forward
// operation; the filesystem op itself already completed.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionError {
    MetadataTooLarge { bytes: usize, cap: usize },
    // Id / generation space exhausted; any further admission
    // would reuse an identifier.
    IdentityExhausted,
    // An Undo or Redo attempt is in flight.
    UndoInFlight,
}

// Stable per-entry identifier; never reused within a session so a
// cached id cannot alias a superseded entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct JournalId(pub(crate) u64);

// Identifier for one in-flight Undo or Redo attempt; only the
// exact live attempt is allowed to settle.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AttemptId(pub(crate) u64);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum JournalDirection {
    Undo,
    Redo,
}

// Where a completion arm places the entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FinishPlacement {
    // Cross to the opposite stack (Undo→redo / Redo→undo).
    AppliedToOpposite,
    // Preserve on the origin stack.
    ReturnToOrigin,
}

// Verdict of `finish_*_attempt`; `WrongAttempt` drops the entry
// and the caller must not touch journal-visible state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum FinishAck {
    Accepted,
    WrongAttempt,
}

// Directory-entry kind captured at admission; a symlink is
// tracked as the link itself, not its target.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FileKind {
    #[default]
    Regular,
    Directory,
    Symlink,
    Other,
}

impl FileKind {
    fn from_metadata(m: &std::fs::Metadata) -> Self {
        let t = m.file_type();
        if t.is_symlink() {
            Self::Symlink
        } else if t.is_dir() {
            Self::Directory
        } else if t.is_file() {
            Self::Regular
        } else {
            Self::Other
        }
    }
}

// `(dev, ino, kind)` identity from [`std::fs::symlink_metadata`],
// re-verified before any inverse mutates the filesystem.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct InodeIdent {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) kind: FileKind,
}

impl InodeIdent {
    // Read the identity at `path` without following symlinks.
    pub(crate) fn of(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::symlink_metadata(path)?;
        Ok(Self {
            dev: md.dev(),
            ino: md.ino(),
            kind: FileKind::from_metadata(&md),
        })
    }

    // Read the identity of an open descriptor via `fstat`, so a
    // path substitution between the open and the check cannot
    // produce a false match.
    pub(crate) fn from_file(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let md = file.metadata()?;
        Ok(Self {
            dev: md.dev(),
            ino: md.ino(),
            kind: FileKind::from_metadata(&md),
        })
    }

    #[cfg(test)]
    pub(crate) const fn synthetic(dev: u64, ino: u64) -> Self {
        Self {
            dev,
            ino,
            kind: FileKind::Regular,
        }
    }
}

// Forward operation captured by an admitted entry. Trash carries
// the admitted `.trashinfo` bytes and the twin's captured
// `(dev, ino)` so Undo can refuse a substituted twin and Redo
// republishes exactly the admitted bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OpRecord {
    Rename {
        src: PathBuf,
        dst: PathBuf,
        dst_inode: InodeIdent,
    },
    Trash {
        origin: PathBuf,
        trashed_path: PathBuf,
        info_path: PathBuf,
        trashed_inode: InodeIdent,
        info_inode: InodeIdent,
        info_bytes: Vec<u8>,
    },
    Move {
        src: PathBuf,
        dst: PathBuf,
        dst_inode: InodeIdent,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JournalEntry {
    pub(crate) id: JournalId,
    pub(crate) op: OpRecord,
    pub(crate) settled_at_ms: u64,
    // Unique per admission; a stale-generation reply is dropped
    // by the reducer rather than aliasing a live entry.
    pub(crate) generation: u64,
}

// Concrete inverse produced by [`compute_inverse`] or
// [`compute_redo`] and applied by [`apply_inverse`]. `TrashPayload`
// only reports `Applied` after the payload lands at `trashed_path`
// and `info_bytes` are durably linked to `info_path` without
// overwriting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InverseOp {
    Rename {
        from: PathBuf,
        to: PathBuf,
        expected_from_inode: InodeIdent,
    },
    RestoreFromTrash {
        trashed_path: PathBuf,
        info_path: PathBuf,
        origin: PathBuf,
        expected_trashed_inode: InodeIdent,
        expected_info_bytes: Vec<u8>,
    },
    TrashPayload {
        origin: PathBuf,
        trashed_path: PathBuf,
        info_path: PathBuf,
        expected_from_inode: InodeIdent,
        info_bytes: Vec<u8>,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        expected_from_inode: InodeIdent,
    },
}

// Terminal outcome of one inverse attempt. `Conflict` and
// `Failed` both preserve the entry for Retry / Skip / Retire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InverseResult {
    Applied,
    Conflict(InverseConflict),
    Failed(String),
}

// Typed external condition that blocked an inverse. Each variant
// names which pathnames now hold whose bytes; foreign entries are
// never overwritten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InverseConflict {
    // The entry at `path` no longer has the recorded identity.
    IdentityChanged {
        path: PathBuf,
        expected: InodeIdent,
        actual: Option<InodeIdent>,
    },
    // A file occupies the destination; the inverse refused to overwrite.
    DestinationOccupied {
        to: PathBuf,
    },
    // A post-inverse rollback could not bind, saw a further
    // substitution, or failed at rename; foreign entries untouched.
    RollbackUncertain {
        at: PathBuf,
        detail: String,
    },
    // Payload rolled back to `trashed_path` under bind-and-verify;
    // twin bytes at `info_path` intact.
    TrashMetadataRetireFailed {
        info_path: PathBuf,
        detail: String,
    },
    // Twin identity diverged from admitted; payload and foreign
    // bytes at `info_path` both untouched.
    TrashMetadataTwinChanged {
        info_path: PathBuf,
    },
    // Payload at `payload_at`; twin removal at `twin_at` may not
    // be durable.
    TrashUndoSettlementUncertain {
        payload_at: PathBuf,
        twin_at: PathBuf,
        detail: String,
    },
    // `info_path` holds a foreign entry; payload rolled back to
    // `origin`, foreign bytes untouched.
    TrashMetadataPublishOccupied {
        info_path: PathBuf,
    },
    // Pre-publish state intact at `info_path`; payload rolled back.
    TrashMetadataPublishFailed {
        info_path: PathBuf,
        detail: String,
    },
    // Publish linked the twin but post-link settlement uncertain;
    // payload at `payload_at`, twin may still be at `twin_at`.
    TrashPublishSettlementUncertain {
        payload_at: PathBuf,
        twin_at: PathBuf,
        detail: String,
    },
    // The trash payload is gone under nofollow metadata.
    TrashItemUnavailable {
        trashed_path: PathBuf,
        info_path: PathBuf,
    },
    // The Trash entry's original parent is missing or not a directory.
    ParentUnavailable {
        parent: PathBuf,
    },
    // Cross-filesystem Move Undo: identity cannot be bound to a
    // handle across a copy.
    CrossFilesystemInverseUncertain {
        at: PathBuf,
    },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum JournalConflictChoice {
    Retry,
    Skip,
    Retire,
}

// One live user-visible prompt bound to a specific entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JournalPrompt {
    pub(crate) entry_id: JournalId,
    pub(crate) entry_generation: u64,
    pub(crate) headline: String,
    pub(crate) detail: String,
    // Retry routes back through the matching stack.
    pub(crate) is_undo: bool,
}

// Maximum live prompts retained; oldest dropped first.
pub(crate) const MAX_JOURNAL_PROMPTS: usize = 4;

// Session-only bounded journal; one instance per app.
#[derive(Debug)]
pub(crate) struct OpJournal {
    entries: VecDeque<JournalEntry>,
    redo: Vec<JournalEntry>,
    // `0` is the exhausted sentinel; the first assigned id,
    // generation, or attempt is `1`. Callers observe exhaustion
    // through [`AdmissionError::IdentityExhausted`], never a zero
    // value.
    next_id: u64,
    next_generation: u64,
    next_attempt: u64,
    in_flight: Option<InFlight>,
    cap: usize,
}

#[derive(Copy, Clone, Debug)]
struct InFlight {
    attempt: AttemptId,
    direction: JournalDirection,
    entry_id: JournalId,
    entry_generation: u64,
}

impl OpJournal {
    pub(crate) fn new() -> Self {
        Self::with_cap(MAX_JOURNAL_ENTRIES)
    }

    // Cap is clamped to a minimum of 1.
    pub(crate) fn with_cap(cap: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(cap.min(64)),
            redo: Vec::new(),
            next_id: 1,
            next_generation: 1,
            next_attempt: 1,
            in_flight: None,
            cap: cap.max(1),
        }
    }

    // Direction of any in-flight attempt, so the UI can disable
    // the opposite mutation until settlement.
    pub(crate) fn in_flight_direction(&self) -> Option<JournalDirection> {
        self.in_flight.map(|f| f.direction)
    }

    // Take an entry off the undo stack under a fresh attempt id;
    // refuses when another attempt is in flight or the stack is empty.
    pub(crate) fn begin_undo_attempt(&mut self) -> Option<(AttemptId, JournalEntry)> {
        if self.in_flight.is_some() {
            return None;
        }
        let entry = self.entries.pop_back()?;
        let attempt = self.mint_attempt()?;
        self.in_flight = Some(InFlight {
            attempt,
            direction: JournalDirection::Undo,
            entry_id: entry.id,
            entry_generation: entry.generation,
        });
        Some((attempt, entry))
    }

    // Take an entry off the redo stack under a fresh attempt id;
    // refuses when another attempt is in flight or the stack is empty.
    pub(crate) fn begin_redo_attempt(&mut self) -> Option<(AttemptId, JournalEntry)> {
        if self.in_flight.is_some() {
            return None;
        }
        let entry = self.redo.pop()?;
        let attempt = self.mint_attempt()?;
        self.in_flight = Some(InFlight {
            attempt,
            direction: JournalDirection::Redo,
            entry_id: entry.id,
            entry_generation: entry.generation,
        });
        Some((attempt, entry))
    }

    // Settle the live Undo attempt; a stale attempt, wrong
    // direction, or mismatched entry is dropped as `WrongAttempt`.
    pub(crate) fn finish_undo_attempt(
        &mut self,
        attempt: AttemptId,
        entry: JournalEntry,
        placement: FinishPlacement,
    ) -> FinishAck {
        if !self.matches_in_flight(attempt, JournalDirection::Undo, &entry) {
            return FinishAck::WrongAttempt;
        }
        match placement {
            FinishPlacement::AppliedToOpposite => self.redo.push(entry),
            FinishPlacement::ReturnToOrigin => {
                self.entries.push_back(entry);
                while self.entries.len() > self.cap {
                    self.entries.pop_front();
                }
            }
        }
        self.in_flight = None;
        FinishAck::Accepted
    }

    // Settle the live Redo attempt; the placement/direction
    // contract mirrors [`Self::finish_undo_attempt`].
    pub(crate) fn finish_redo_attempt(
        &mut self,
        attempt: AttemptId,
        entry: JournalEntry,
        placement: FinishPlacement,
    ) -> FinishAck {
        if !self.matches_in_flight(attempt, JournalDirection::Redo, &entry) {
            return FinishAck::WrongAttempt;
        }
        match placement {
            FinishPlacement::AppliedToOpposite => {
                self.entries.push_back(entry);
                while self.entries.len() > self.cap {
                    self.entries.pop_front();
                }
            }
            FinishPlacement::ReturnToOrigin => self.redo.push(entry),
        }
        self.in_flight = None;
        FinishAck::Accepted
    }

    fn matches_in_flight(
        &self,
        attempt: AttemptId,
        direction: JournalDirection,
        entry: &JournalEntry,
    ) -> bool {
        match self.in_flight {
            Some(f) => {
                f.attempt == attempt
                    && f.direction == direction
                    && f.entry_id == entry.id
                    && f.entry_generation == entry.generation
            }
            None => false,
        }
    }

    fn mint_attempt(&mut self) -> Option<AttemptId> {
        if self.next_attempt == 0 {
            return None;
        }
        let id = AttemptId(self.next_attempt);
        self.next_attempt = self.next_attempt.checked_add(1).unwrap_or(0);
        Some(id)
    }

    // Admit one durably-settled forward operation. The redo tail
    // is retired before byte/identity checks (a new forward edit
    // invalidates old redo history regardless of admission), and
    // an in-flight attempt refuses admission so the reverse
    // cannot interleave with a fresh forward.
    pub(crate) fn record(
        &mut self,
        op: OpRecord,
        now_ms: u64,
    ) -> Result<JournalId, AdmissionError> {
        if self.in_flight.is_some() {
            return Err(AdmissionError::UndoInFlight);
        }
        self.redo.clear();
        let bytes = entry_bytes_of(&op);
        if bytes > MAX_ENTRY_METADATA_BYTES {
            return Err(AdmissionError::MetadataTooLarge {
                bytes,
                cap: MAX_ENTRY_METADATA_BYTES,
            });
        }
        if self.next_id == 0 || self.next_generation == 0 {
            return Err(AdmissionError::IdentityExhausted);
        }
        let id = JournalId(self.next_id);
        let generation = self.next_generation;
        // `checked_add(1).unwrap_or(0)` sets the exhausted sentinel
        // so any subsequent call refuses instead of reusing an id.
        self.next_id = self.next_id.checked_add(1).unwrap_or(0);
        self.next_generation = self.next_generation.checked_add(1).unwrap_or(0);
        let entry = JournalEntry {
            id,
            op,
            settled_at_ms: now_ms,
            generation,
        };
        self.entries.push_back(entry);
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
        Ok(id)
    }

    pub(crate) fn peek_undo(&self) -> Option<&JournalEntry> {
        self.entries.back()
    }
    pub(crate) fn peek_redo(&self) -> Option<&JournalEntry> {
        self.redo.last()
    }

    pub(crate) fn retire(&mut self, id: JournalId) {
        self.entries.retain(|e| e.id != id);
        self.redo.retain(|e| e.id != id);
    }

    pub(crate) fn find(&self, id: JournalId) -> Option<&JournalEntry> {
        self.entries
            .iter()
            .chain(self.redo.iter())
            .find(|e| e.id == id)
    }

    pub(crate) fn generation_of(&self, id: JournalId) -> Option<u64> {
        self.find(id).map(|e| e.generation)
    }

    pub(crate) fn can_undo(&self) -> bool {
        !self.entries.is_empty()
    }
    pub(crate) fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

impl Default for OpJournal {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn compute_inverse(entry: &JournalEntry) -> InverseOp {
    match &entry.op {
        OpRecord::Rename {
            src,
            dst,
            dst_inode,
        } => InverseOp::Rename {
            from: dst.clone(),
            to: src.clone(),
            expected_from_inode: *dst_inode,
        },
        OpRecord::Trash {
            origin,
            trashed_path,
            info_path,
            trashed_inode,
            info_bytes,
            ..
        } => InverseOp::RestoreFromTrash {
            trashed_path: trashed_path.clone(),
            info_path: info_path.clone(),
            origin: origin.clone(),
            expected_trashed_inode: *trashed_inode,
            expected_info_bytes: info_bytes.clone(),
        },
        OpRecord::Move {
            src,
            dst,
            dst_inode,
        } => InverseOp::Move {
            from: dst.clone(),
            to: src.clone(),
            expected_from_inode: *dst_inode,
        },
    }
}

// Redo-direction op; pure. Trash resolves to
// [`InverseOp::TrashPayload`] carrying the recorded bytes, not to
// an unsafe swap of [`compute_inverse`].
pub(crate) fn compute_redo(entry: &JournalEntry) -> InverseOp {
    match &entry.op {
        OpRecord::Rename {
            src,
            dst,
            dst_inode,
        } => InverseOp::Rename {
            from: src.clone(),
            to: dst.clone(),
            expected_from_inode: *dst_inode,
        },
        OpRecord::Trash {
            origin,
            trashed_path,
            info_path,
            trashed_inode,
            info_bytes,
            ..
        } => InverseOp::TrashPayload {
            origin: origin.clone(),
            trashed_path: trashed_path.clone(),
            info_path: info_path.clone(),
            expected_from_inode: *trashed_inode,
            info_bytes: info_bytes.clone(),
        },
        OpRecord::Move {
            src,
            dst,
            dst_inode,
        } => InverseOp::Move {
            from: src.clone(),
            to: dst.clone(),
            expected_from_inode: *dst_inode,
        },
    }
}

// Fault-injection seams held module-private so no public API
// depends on a test type. Same-crate submodule tests construct
// `InverseHooks` directly and call [`dispatch_inverse`]; production
// callers only see [`apply_inverse`].
#[derive(Copy, Clone)]
struct InverseHooks {
    after_precheck: fn(reverse_src: &Path),
    after_inverse: fn(reverse_src: &Path, reverse_dst: &Path),
    between_postcheck_and_rollback: fn(reverse_src: &Path, reverse_dst: &Path),
    same_fs_rename_impl: fn(&Path, &Path) -> std::io::Result<()>,
    retire_impl: fn(&Path, &[u8]) -> Result<(), crate::trash::RetireError>,
    publish_info_impl: fn(&Path, &[u8]) -> Result<(), crate::trash::PublishError>,
    sync_parent_impl: fn(&Path) -> std::io::Result<()>,
}

impl InverseHooks {
    const PRODUCTION: Self = Self {
        publish_info_impl: crate::trash::publish_metadata_twin,
        retire_impl: crate::trash::retire_metadata_twin,
        sync_parent_impl: enzui_security::durable::sync_parent_dir,
        after_precheck: |_| {},
        after_inverse: |_, _| {},
        between_postcheck_and_rollback: |_, _| {},
        same_fs_rename_impl: crate::services::same_fs_no_replace_rename,
    };
}

// Apply an inverse under production seams; blocking. Every
// rename-based inverse verifies the recorded `(dev, ino, kind)`
// twice — pre-op at the source and post-op at the destination —
// and a mismatch triggers bound-and-verified rollback; `Applied`
// is never returned for an unexpected inode. Cross-filesystem Move
// Undo returns [`InverseConflict::CrossFilesystemInverseUncertain`].
pub(crate) fn apply_inverse(op: &InverseOp) -> InverseResult {
    dispatch_inverse(op, InverseHooks::PRODUCTION)
}

#[cfg(unix)]
fn path_bytes(p: &Path) -> usize {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().len()
}

#[cfg(all(not(unix), windows))]
fn path_bytes(p: &Path) -> usize {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().count().saturating_mul(2)
}

#[cfg(all(not(unix), not(windows)))]
fn path_bytes(p: &Path) -> usize {
    p.as_os_str().to_string_lossy().len()
}

// Serialized metadata cost of an [`OpRecord`], in bytes; saturating.
pub(crate) fn entry_bytes_of(op: &OpRecord) -> usize {
    let variable_sum: usize = match op {
        OpRecord::Rename { src, dst, .. } => path_bytes(src).saturating_add(path_bytes(dst)),
        OpRecord::Trash {
            origin,
            trashed_path,
            info_path,
            info_bytes,
            ..
        } => path_bytes(origin)
            .saturating_add(path_bytes(trashed_path))
            .saturating_add(path_bytes(info_path))
            .saturating_add(info_bytes.len()),
        OpRecord::Move { src, dst, .. } => path_bytes(src).saturating_add(path_bytes(dst)),
    };
    variable_sum.saturating_add(SCALAR_OVERHEAD_BYTES)
}

// Wall-clock milliseconds since the Unix epoch, `0` on error.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// Short label for an entry; file names only, never absolute paths.
pub(crate) fn describe(entry: &JournalEntry) -> String {
    fn name(p: &Path) -> String {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
    match &entry.op {
        OpRecord::Rename { src, dst, .. } => crate::i18n::format(
            "files-rename-efc9af40",
            &[("arg0", (name(src))), ("arg1", (name(dst)))],
        ),
        OpRecord::Trash { origin, .. } => {
            crate::i18n::format("files-move-to-trash-199dc63b", &[("arg0", (name(origin)))])
        }
        OpRecord::Move { src, dst, .. } => crate::i18n::format(
            "files-move-to-c1d93b7a",
            &[("arg0", (name(src))), ("arg1", (name(dst)))],
        ),
    }
}

#[path = "journal/dispatch.rs"]
mod dispatch;
#[path = "journal/rollback.rs"]
mod rollback;

use dispatch::dispatch_inverse;

#[cfg(test)]
mod tests;

#[cfg(test)]
impl OpJournal {
    pub(crate) fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub(crate) fn undo_len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn redo_applied(&mut self, entry: JournalEntry) {
        self.entries.push_back(entry);
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
    }

    pub(crate) fn take_redo_target(&mut self) -> Option<JournalEntry> {
        self.redo.pop()
    }

    pub(crate) fn undo_return(&mut self, entry: JournalEntry) {
        self.entries.push_back(entry);
        while self.entries.len() > self.cap {
            self.entries.pop_front();
        }
    }

    pub(crate) fn undo_applied(&mut self, entry: JournalEntry) {
        self.redo.push(entry);
    }

    pub(crate) fn take_undo_target(&mut self) -> Option<JournalEntry> {
        self.entries.pop_back()
    }
}
