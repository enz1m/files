// Inverse dispatch: route each InverseOp to its rename, trash, or move worker
// with identity verification.
use super::rollback::{
    rollback_after_mismatch, rollback_after_twin_retire_failure, rollback_after_twin_substitution,
    rollback_payload_after_publish_failure,
};
use super::{InodeIdent, InverseConflict, InverseHooks, InverseOp, InverseResult};
use std::path::Path;

pub(super) fn dispatch_inverse(op: &InverseOp, hooks: InverseHooks) -> InverseResult {
    match op {
        InverseOp::Rename {
            from,
            to,
            expected_from_inode,
        } => rename_dispatch(from, to, expected_from_inode, hooks),
        InverseOp::RestoreFromTrash {
            trashed_path,
            info_path,
            origin,
            expected_trashed_inode,
            expected_info_bytes,
        } => trash_dispatch(
            trashed_path,
            info_path,
            origin,
            expected_trashed_inode,
            expected_info_bytes,
            hooks,
        ),
        InverseOp::TrashPayload {
            origin,
            trashed_path,
            info_path,
            expected_from_inode,
            info_bytes,
        } => trash_publish_dispatch(
            origin,
            trashed_path,
            info_path,
            expected_from_inode,
            info_bytes,
            hooks,
        ),
        InverseOp::Move {
            from,
            to,
            expected_from_inode,
        } => move_dispatch(from, to, expected_from_inode, hooks),
    }
}

fn rename_dispatch(
    from: &Path,
    to: &Path,
    expected: &InodeIdent,
    hooks: InverseHooks,
) -> InverseResult {
    if let Err(conflict) = verify_identity(from, expected) {
        return InverseResult::Conflict(conflict);
    }
    (hooks.after_precheck)(from);
    if let Err(err) = crate::services::rename_reversing(from, to) {
        return classify_rename_error(&err, to);
    }
    (hooks.after_inverse)(from, to);
    match InodeIdent::of(to) {
        Ok(actual) if actual == *expected => InverseResult::Applied,
        Ok(actual) => rollback_after_mismatch(from, to, expected, Some(actual), hooks),
        Err(_) => rollback_after_mismatch(from, to, expected, None, hooks),
    }
}

// Nofollow existence probe. A dangling symlink counts as present
// because it has a directory entry.
fn nofollow_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn trash_dispatch(
    trashed_path: &Path,
    info_path: &Path,
    origin: &Path,
    expected_payload: &InodeIdent,
    expected_info_bytes: &[u8],
    hooks: InverseHooks,
) -> InverseResult {
    if !nofollow_exists(trashed_path) {
        return InverseResult::Conflict(InverseConflict::TrashItemUnavailable {
            trashed_path: trashed_path.to_path_buf(),
            info_path: info_path.to_path_buf(),
        });
    }
    if let Err(conflict) = verify_identity(trashed_path, expected_payload) {
        return InverseResult::Conflict(conflict);
    }
    (hooks.after_precheck)(trashed_path);
    let entry = crate::trash::TrashEntry {
        trashed_path: trashed_path.to_path_buf(),
        info_path: info_path.to_path_buf(),
        original_path: origin.to_path_buf(),
        name: origin
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        deletion_date: String::new(),
        retained_root: None,
        mime: String::new(),
    };
    use crate::trash::{PublishCustody, RestoreError};
    // The twin at `info_path` survives payload rollback. `Pending { custody: Source }`
    // put no bytes at `origin` and collapses into `Failed`; other custody may have,
    // so it becomes `TrashUndoSettlementUncertain` and the twin is kept.
    match crate::trash::restore_payload_without_replacing(&entry) {
        Ok(()) => {}
        Err(RestoreError::DestinationOccupied { at }) => {
            return InverseResult::Conflict(InverseConflict::DestinationOccupied { to: at });
        }
        Err(RestoreError::ParentUnavailable { parent }) => {
            return InverseResult::Conflict(InverseConflict::ParentUnavailable { parent });
        }
        Err(RestoreError::OriginalLocationUnknown) => {
            return InverseResult::Conflict(InverseConflict::ParentUnavailable {
                parent: origin
                    .parent()
                    .map_or_else(|| origin.to_path_buf(), Path::to_path_buf),
            });
        }
        Err(RestoreError::Pending {
            custody: PublishCustody::Source,
            diagnostic,
        }) => return InverseResult::Failed(diagnostic),
        Err(RestoreError::Pending {
            custody: _,
            diagnostic,
        }) => {
            return InverseResult::Conflict(InverseConflict::TrashUndoSettlementUncertain {
                payload_at: origin.to_path_buf(),
                twin_at: info_path.to_path_buf(),
                detail: diagnostic,
            });
        }
        Err(other) => return InverseResult::Failed(other.to_string()),
    }
    (hooks.after_inverse)(trashed_path, origin);
    let post_check = InodeIdent::of(origin);
    match &post_check {
        Ok(actual) if actual == expected_payload => {}
        Ok(actual) => {
            return rollback_after_mismatch(
                trashed_path,
                origin,
                expected_payload,
                Some(*actual),
                hooks,
            );
        }
        Err(_) => {
            return rollback_after_mismatch(trashed_path, origin, expected_payload, None, hooks);
        }
    }
    // Identity-bound retire: a mismatched inode is refused untouched; a match
    // unlinks through the held descriptor and settles the parent.
    use crate::trash::RetireError;
    match (hooks.retire_impl)(info_path, expected_info_bytes) {
        Ok(()) | Err(RetireError::Vanished) => {}
        Err(RetireError::Substituted { detail: _ }) => {
            return rollback_after_twin_substitution(
                trashed_path,
                origin,
                expected_payload,
                info_path,
                hooks,
            );
        }
        Err(RetireError::RemovalFailed { detail }) => {
            return rollback_after_twin_retire_failure(
                trashed_path,
                origin,
                expected_payload,
                info_path,
                std::io::Error::other(detail),
                hooks,
            );
        }
        Err(RetireError::ParentSyncFailed { detail }) => {
            return InverseResult::Conflict(InverseConflict::TrashUndoSettlementUncertain {
                payload_at: origin.to_path_buf(),
                twin_at: info_path.to_path_buf(),
                detail,
            });
        }
    }
    // Payload parents (source and destination of the reverse
    // rename) must both settle before `Applied`.
    if let Err(e) = (hooks.sync_parent_impl)(origin) {
        return InverseResult::Conflict(InverseConflict::TrashUndoSettlementUncertain {
            payload_at: origin.to_path_buf(),
            twin_at: info_path.to_path_buf(),
            detail: crate::i18n::format(
                "files-origin-parent-sync-failed-e-091c8ae5",
                &[("e", (e).to_string())],
            ),
        });
    }
    if let Err(e) = (hooks.sync_parent_impl)(trashed_path) {
        return InverseResult::Conflict(InverseConflict::TrashUndoSettlementUncertain {
            payload_at: origin.to_path_buf(),
            twin_at: info_path.to_path_buf(),
            detail: crate::i18n::format(
                "files-trash-files-parent-sync-failed-e-278b52e9",
                &[("e", (e).to_string())],
            ),
        });
    }
    InverseResult::Applied
}

// `Applied` requires the payload at `trashed_path` and the twin durably linked
// at `info_path`. Earlier failures roll the payload back to `origin` under
// bind-and-verify; a foreign twin is never overwritten.
fn trash_publish_dispatch(
    origin: &Path,
    trashed_path: &Path,
    info_path: &Path,
    expected: &InodeIdent,
    info_bytes: &[u8],
    hooks: InverseHooks,
) -> InverseResult {
    if let Err(conflict) = verify_identity(origin, expected) {
        return InverseResult::Conflict(conflict);
    }
    (hooks.after_precheck)(origin);
    if let Err(err) = crate::services::rename_reversing(origin, trashed_path) {
        return classify_rename_error(&err, trashed_path);
    }
    (hooks.after_inverse)(origin, trashed_path);
    match InodeIdent::of(trashed_path) {
        Ok(actual) if actual == *expected => {}
        Ok(actual) => {
            return rollback_after_mismatch(origin, trashed_path, expected, Some(actual), hooks);
        }
        Err(_) => {
            return rollback_after_mismatch(origin, trashed_path, expected, None, hooks);
        }
    }
    let publish = (hooks.publish_info_impl)(info_path, info_bytes);
    match publish {
        Ok(()) => {}
        // Pre-publish state intact: roll the payload back and report a retryable
        // reason.
        Err(crate::trash::PublishError::Occupied) => {
            return rollback_payload_after_publish_failure(
                origin,
                trashed_path,
                expected,
                InverseConflict::TrashMetadataPublishOccupied {
                    info_path: info_path.to_path_buf(),
                },
                hooks,
            );
        }
        Err(
            e @ (crate::trash::PublishError::NothingPublished { .. }
            | crate::trash::PublishError::UnpublishedAfterSettlementFailure { .. }
            | crate::trash::PublishError::SubstitutedForeignTwin { .. }),
        ) => {
            return rollback_payload_after_publish_failure(
                origin,
                trashed_path,
                expected,
                InverseConflict::TrashMetadataPublishFailed {
                    info_path: info_path.to_path_buf(),
                    detail: e.to_string(),
                },
                hooks,
            );
        }
        // Twin custody is uncertain: keep the payload and name both sites.
        Err(e @ crate::trash::PublishError::UncertainAfterSettlementFailure { .. }) => {
            return InverseResult::Conflict(InverseConflict::TrashPublishSettlementUncertain {
                payload_at: trashed_path.to_path_buf(),
                twin_at: info_path.to_path_buf(),
                detail: e.to_string(),
            });
        }
    }
    // Payload parents must both settle before `Applied`; the twin
    // parent already settled inside `publish_metadata_twin`.
    if let Err(e) = (hooks.sync_parent_impl)(origin) {
        return InverseResult::Conflict(InverseConflict::TrashPublishSettlementUncertain {
            payload_at: trashed_path.to_path_buf(),
            twin_at: info_path.to_path_buf(),
            detail: crate::i18n::format(
                "files-origin-parent-sync-failed-e-091c8ae5",
                &[("e", (e).to_string())],
            ),
        });
    }
    if let Err(e) = (hooks.sync_parent_impl)(trashed_path) {
        return InverseResult::Conflict(InverseConflict::TrashPublishSettlementUncertain {
            payload_at: trashed_path.to_path_buf(),
            twin_at: info_path.to_path_buf(),
            detail: crate::i18n::format(
                "files-trash-files-parent-sync-failed-e-278b52e9",
                &[("e", (e).to_string())],
            ),
        });
    }
    InverseResult::Applied
}

fn move_dispatch(
    from: &Path,
    to: &Path,
    expected: &InodeIdent,
    hooks: InverseHooks,
) -> InverseResult {
    if let Err(conflict) = verify_identity(from, expected) {
        return InverseResult::Conflict(conflict);
    }
    (hooks.after_precheck)(from);
    match (hooks.same_fs_rename_impl)(from, to) {
        Ok(()) => {
            (hooks.after_inverse)(from, to);
            match InodeIdent::of(to) {
                Ok(actual) if actual == *expected => InverseResult::Applied,
                Ok(actual) => rollback_after_mismatch(from, to, expected, Some(actual), hooks),
                Err(_) => rollback_after_mismatch(from, to, expected, None, hooks),
            }
        }
        Err(err) if crate::services::is_cross_filesystem_error(&err) => {
            // The move service cannot bind identity across a cross-filesystem copy,
            // so `Applied` cannot be claimed.
            InverseResult::Conflict(InverseConflict::CrossFilesystemInverseUncertain {
                at: to.to_path_buf(),
            })
        }
        Err(err) => classify_rename_error(&err.to_string(), to),
    }
}

fn verify_identity(path: &Path, expected: &InodeIdent) -> Result<(), InverseConflict> {
    match InodeIdent::of(path) {
        Ok(actual) if actual == *expected => Ok(()),
        Ok(actual) => Err(InverseConflict::IdentityChanged {
            path: path.to_path_buf(),
            expected: *expected,
            actual: Some(actual),
        }),
        Err(_) => Err(InverseConflict::IdentityChanged {
            path: path.to_path_buf(),
            expected: *expected,
            actual: None,
        }),
    }
}

fn classify_rename_error(err: &str, to: &Path) -> InverseResult {
    let lower = err.to_lowercase();
    if lower.contains("exists") || lower.contains("eexist") {
        InverseResult::Conflict(InverseConflict::DestinationOccupied {
            to: to.to_path_buf(),
        })
    } else {
        InverseResult::Failed(err.to_owned())
    }
}
