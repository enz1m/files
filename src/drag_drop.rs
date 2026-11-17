// Pointer and keyboard transfers share validation and paste admission; the shared host owns gestures.

use enzui_core::virtual_sequence::KeyedProvider;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[cfg(test)]
use enzui_core::drag::CollectionId;
use enzui_core::drag::{
    DestinationProposal, DragOperation, DropTarget, ItemDropRequest, ItemId, PayloadKind,
    PayloadKinds, SourceCapabilities, keyboard_drop,
};

use crate::model::{EntryKind, Model};

// Encode a filesystem path as a lossless, reversible [`ItemId`].
//
// Linux paths need not be UTF-8, so the raw bytes are percent-encoded: every
// byte `>= 0x80` and `%` itself become `%XX`. The result is valid UTF-8 and
// [`id_to_path`] reverses it byte-for-byte.
#[must_use]
pub(crate) fn path_to_id(path: &Path) -> ItemId {
    let bytes = path.as_os_str().as_bytes();
    let mut s = String::with_capacity(bytes.len());
    for &b in bytes {
        if b == b'%' || b >= 0x80 {
            s.push('%');
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        } else {
            s.push(b as char);
        }
    }
    ItemId::new(s)
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

// Reverse [`path_to_id`]. Returns `None` for a truncated or non-hex `%`
// escape rather than decoding to a wrong path.
#[must_use]
pub(crate) fn id_to_path(id: &ItemId) -> Option<PathBuf> {
    let s = id.as_str().as_bytes();
    let mut bytes = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            let hi = s.get(i + 1).copied().and_then(hex_val)?;
            let lo = s.get(i + 2).copied().and_then(hex_val)?;
            bytes.push((hi << 4) | lo);
            i += 3;
        } else {
            bytes.push(s[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

// The stable collection identity shared by the list and the grid. Only one of
// the two is realised per frame, so the drag host never sees a duplicate.
pub(crate) const DIRECTORY_COLLECTION: &str = "files-app.directory";
// The breadcrumb's ancestor crumbs, as drop-only targets.
pub(crate) const BREADCRUMB_COLLECTION: &str = "files-app.breadcrumb";

// The payload kind a files drag offers and a files destination accepts. A
// named kind, rather than [`PayloadKinds::universal`], makes `negotiate`
// refuse foreign drags before [`validate_item_drop_in`].
#[must_use]
pub(crate) fn files_payload() -> PayloadKinds {
    PayloadKinds::of(&[PayloadKind::application("application/x-files-app-entry")])
}

// Source policy: a files drag may move (default) or copy (Ctrl-held / explicit
// verb).
#[must_use]
pub(crate) fn source_capabilities() -> SourceCapabilities {
    SourceCapabilities::copy_or_move()
}

// Whether a realised entry is a valid onto-item destination, decided
// without filesystem access so it is cheap per row in `view`.
//
// Symlinks to directories are already classified [`EntryKind::Dir`] by
// [`services::entry_from_path`](crate::services), so a remaining
// [`EntryKind::Symlink`] is a broken or non-directory link and is refused.
#[must_use]
pub(crate) fn accepts_drop(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::Dir)
}

// Why a drop or a keyboard Move/Copy verb was refused before any disk work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DropRefusal {
    // The destination is not (or is no longer) a directory.
    NotADirectory,
    // The destination directory cannot be written to.
    Unwritable,
    // The destination is one of the dragged items themselves.
    SelfDrop,
    // The destination lives inside one of the dragged items (a cycle).
    IntoDescendant,
    // Every dragged id has vanished from disk since the drag began.
    StaleSource,
    // A move whose sources already live in the destination directory.
    NoOp,
    // The negotiated operation was not a real move/copy (nothing to do).
    NotDroppable,
}

impl DropRefusal {
    // A user-facing status line for the refusal.
    #[must_use]
    pub(crate) fn status(&self) -> &'static str {
        match self {
            Self::NotADirectory => crate::i18n::tr("Can only drop onto a folder"),
            Self::Unwritable => crate::i18n::tr("That folder is read-only"),
            Self::SelfDrop => crate::i18n::tr("Cannot drop an item onto itself"),
            Self::IntoDescendant => crate::i18n::tr("Cannot move a folder into itself"),
            Self::StaleSource => crate::i18n::tr("The dragged items are no longer available"),
            Self::NoOp => crate::i18n::tr("The items are already in that folder"),
            Self::NotDroppable => crate::i18n::tr("That drop is not allowed"),
        }
    }
}

// A validated, executable drop: exactly the fields `Msg::BeginPaste` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedDrop {
    // Source paths, in the drag's source order, that still exist on disk.
    pub(crate) sources: Vec<PathBuf>,
    // The destination directory (a folder tile, or the current directory).
    pub(crate) dest_dir: PathBuf,
    // `true` for an explicit copy; `false` for the default move.
    pub(crate) copy: bool,
}

// What a drop's validation needs from the model, taken on the UI thread so
// the disk checks can run on a worker.
#[derive(Debug, Clone)]
pub(crate) struct DropContext {
    cwd: PathBuf,
    known: std::collections::HashSet<ItemId>,
}

impl DropContext {
    pub(crate) fn of(model: &Model) -> Self {
        Self {
            cwd: model.cwd.clone(),
            known: model
                .projected_entries()
                .into_iter()
                .map(|entry| path_to_id(&entry.path))
                .collect(),
        }
    }
}

// [`validate_item_drop_in`] against a captured [`DropContext`]; touches the
// disk, so it runs off the UI thread.
pub(crate) fn validate_item_drop_in(
    context: &DropContext,
    req: &ItemDropRequest,
) -> Result<PlannedDrop, DropRefusal> {
    let copy = match req.request.operation {
        DragOperation::Copy => true,
        DragOperation::Move => false,
        DragOperation::Delete | DragOperation::Forbidden => return Err(DropRefusal::NotDroppable),
    };

    let dest_dir = match &req.target {
        DropTarget::OntoItem(id) => {
            // A malformed id is a forged/garbled target, not a real folder.
            let path = id_to_path(id).ok_or(DropRefusal::StaleSource)?;
            // Follow symlinks: a link to a directory is a directory here.
            if !std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                return Err(DropRefusal::NotADirectory);
            }
            path
        }
        DropTarget::OntoCollection => {
            if crate::services::is_virtual(&context.cwd)
                || !std::fs::metadata(&context.cwd).is_ok_and(|m| m.is_dir())
            {
                return Err(DropRefusal::NotADirectory);
            }
            context.cwd.clone()
        }
        // A between-items reorder slot is not a files destination.
        DropTarget::Insertion(_) => return Err(DropRefusal::NotDroppable),
    };

    if !is_writable_dir(&dest_dir) {
        return Err(DropRefusal::Unwritable);
    }

    // Reject the whole request if any id is malformed, not in the visible
    // entries, or gone from disk; never move a surviving subset.
    let known = &context.known;
    if req.request.items.is_empty() {
        return Err(DropRefusal::StaleSource);
    }
    let mut sources: Vec<PathBuf> = Vec::with_capacity(req.request.items.len());
    for id in &req.request.items {
        if !known.contains(id) {
            return Err(DropRefusal::StaleSource);
        }
        let Some(path) = id_to_path(id) else {
            return Err(DropRefusal::StaleSource);
        };
        // `symlink_metadata` so a dragged symlink counts as present without
        // dereferencing it.
        if std::fs::symlink_metadata(&path).is_err() {
            return Err(DropRefusal::StaleSource);
        }
        sources.push(path);
    }

    for source in &sources {
        if paths_equal(source, &dest_dir) {
            return Err(DropRefusal::SelfDrop);
        }
        if is_descendant(&dest_dir, source) {
            return Err(DropRefusal::IntoDescendant);
        }
    }

    // A same-directory copy is a real duplicate, so only moves are refused.
    if !copy
        && sources.iter().all(|s| {
            s.parent()
                .is_some_and(|parent| paths_equal(parent, &dest_dir))
        })
    {
        return Err(DropRefusal::NoOp);
    }

    Ok(PlannedDrop {
        sources,
        dest_dir,
        copy,
    })
}

// Native peers are untrusted; reject the entire batch before planning writes.
pub(crate) fn validate_native_paths(sources: &[PathBuf], dest: &Path) -> Result<(), DropRefusal> {
    if sources.is_empty() {
        return Err(DropRefusal::StaleSource);
    }
    if !is_writable_dir(dest) {
        return Err(DropRefusal::Unwritable);
    }
    for source in sources {
        if !source.is_absolute() || std::fs::symlink_metadata(source).is_err() {
            return Err(DropRefusal::StaleSource);
        }
        if paths_equal(source, dest) {
            return Err(DropRefusal::SelfDrop);
        }
        if is_descendant(dest, source) {
            return Err(DropRefusal::IntoDescendant);
        }
    }
    Ok(())
}

// Build the [`ItemDropRequest`] a pointer drop onto `dest` would publish, for
// the keyboard Move to/Copy to verbs.
//
// Uses the framework's [`keyboard_drop`] so payload-kind and operation
// refusals match the pointer route.
#[must_use]
pub(crate) fn keyboard_item_drop(
    model: &Model,
    dest: &Path,
    copy: bool,
) -> Option<ItemDropRequest> {
    let items = selected_ids_in_order(model);
    if items.is_empty() {
        return None;
    }
    let ops = if copy {
        vec![DragOperation::Copy]
    } else {
        vec![DragOperation::Move]
    };
    let proposal = DestinationProposal::onto_item(DIRECTORY_COLLECTION, path_to_id(dest), ops)
        .accepting(files_payload());
    keyboard_drop(
        DIRECTORY_COLLECTION,
        items,
        source_capabilities(),
        files_payload(),
        proposal,
    )
}

// The current selection as drag ids, in visible order.
#[must_use]
pub(crate) fn selected_ids_in_order(model: &Model) -> Vec<ItemId> {
    let mut selected: Vec<_> = model
        .selection
        .iter()
        .filter_map(|path| {
            model
                .visible_provider
                .index_of(&enzui_core::virtual_sequence::SequenceKey::new(
                    path_to_id(path).as_str(),
                ))
                .map(|index| (index, path))
        })
        .collect();
    selected.sort_unstable_by_key(|(index, _)| *index);
    selected
        .into_iter()
        .map(|(_, path)| path_to_id(path))
        .collect()
}

#[cfg(test)]
#[must_use]
pub(crate) fn collection() -> CollectionId {
    CollectionId::new(DIRECTORY_COLLECTION)
}

// Whether `dir` is a directory the current process may create entries in.
#[allow(unsafe_code, reason = "libc access(2) FFI")]
fn is_writable_dir(dir: &Path) -> bool {
    if !std::fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated C string for the duration of the
    // call; `access` reads it and returns a status code with no other effects.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

// Whether `candidate` is `ancestor` itself or nested beneath it, comparing
// canonical paths so `.`/`..`/symlink spellings cannot smuggle a cycle past
// the guard.
fn is_descendant(candidate: &Path, ancestor: &Path) -> bool {
    let canon_candidate = std::fs::canonicalize(candidate);
    let canon_ancestor = std::fs::canonicalize(ancestor);
    match (canon_candidate, canon_ancestor) {
        (Ok(c), Ok(a)) => c.starts_with(&a),
        // Uncanonicalisable paths fall back to a lexical test, never to
        // permitting the drop.
        _ => candidate.starts_with(ancestor),
    }
}

// Compare two paths by canonical identity, falling back to a lexical compare.
fn paths_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}
