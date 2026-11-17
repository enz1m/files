// Rollback: restore observed identities after a mismatch, moving no foreign
// entry.
use super::{InodeIdent, InverseConflict, InverseHooks, InverseResult};
use std::path::Path;

// Bound-and-verify rollback: the exact observed impostor identity
// must still be at `reverse_dst` before rename, else no foreign
// entry is moved.
pub(super) fn rollback_after_mismatch(
    reverse_src: &Path,
    reverse_dst: &Path,
    expected: &InodeIdent,
    observed: Option<InodeIdent>,
    hooks: InverseHooks,
) -> InverseResult {
    let Some(reclaiming) = observed else {
        return InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: reverse_dst.to_path_buf(),
            detail: crate::i18n::tr(
                "post-inverse identity was unreadable; refusing to move an unknown entry",
            )
            .into(),
        });
    };
    (hooks.between_postcheck_and_rollback)(reverse_src, reverse_dst);
    match InodeIdent::of(reverse_dst) {
        Ok(now) if now == reclaiming => {}
        Ok(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: reverse_dst.to_path_buf(),
                detail: crate::i18n::format(
                    "files-post-inverse-entity-at-changed-before-5e0c8bdd",
                    &[("arg0", (reverse_dst.display()).to_string())],
                ),
            });
        }
        Err(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: reverse_dst.to_path_buf(),
                detail: crate::i18n::format(
                    "files-post-inverse-entity-at-disappeared-before-1971a2fe",
                    &[("arg0", (reverse_dst.display()).to_string())],
                ),
            });
        }
    }
    if let Err(detail) = crate::services::rename_reversing(reverse_dst, reverse_src) {
        return InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: reverse_dst.to_path_buf(),
            detail,
        });
    }
    match InodeIdent::of(reverse_src) {
        Ok(after) if after == reclaiming => {
            InverseResult::Conflict(InverseConflict::IdentityChanged {
                path: reverse_src.to_path_buf(),
                expected: *expected,
                actual: Some(reclaiming),
            })
        }
        _ => InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: reverse_src.to_path_buf(),
            detail: crate::i18n::tr(
                "rollback rename returned Ok but the reclaimed identity is not at target",
            )
            .into(),
        }),
    }
}

// Payload rollback after twin retire failed; identity at `origin`
// is already confirmed and the twin bytes remain for Retry.
pub(super) fn rollback_after_twin_retire_failure(
    trashed_path: &Path,
    origin: &Path,
    expected: &InodeIdent,
    info_path: &Path,
    unlink_err: std::io::Error,
    hooks: InverseHooks,
) -> InverseResult {
    (hooks.between_postcheck_and_rollback)(trashed_path, origin);
    match InodeIdent::of(origin) {
        Ok(now) if now == *expected => {}
        Ok(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: origin.to_path_buf(),
                detail: crate::i18n::format(
                    "files-payload-at-changed-before-metadata-retire-87f30c41",
                    &[("arg0", (origin.display()).to_string())],
                ),
            });
        }
        Err(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: origin.to_path_buf(),
                detail: crate::i18n::format(
                    "files-payload-at-disappeared-before-metadata-retire-0f20637f",
                    &[("arg0", (origin.display()).to_string())],
                ),
            });
        }
    }
    if let Err(detail) = crate::services::rename_reversing(origin, trashed_path) {
        return InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: origin.to_path_buf(),
            detail,
        });
    }
    match InodeIdent::of(trashed_path) {
        Ok(after) if after == *expected => {
            InverseResult::Conflict(InverseConflict::TrashMetadataRetireFailed {
                info_path: info_path.to_path_buf(),
                detail: unlink_err.to_string(),
            })
        }
        _ => InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: trashed_path.to_path_buf(),
            detail: crate::i18n::tr(
                "metadata-retire rollback rename returned Ok but expected identity is not at trash",
            )
            .into(),
        }),
    }
}

// Payload rollback after twin substitution; foreign inode at
// `info_path` is never touched.
pub(super) fn rollback_after_twin_substitution(
    trashed_path: &Path,
    origin: &Path,
    expected: &InodeIdent,
    info_path: &Path,
    hooks: InverseHooks,
) -> InverseResult {
    (hooks.between_postcheck_and_rollback)(trashed_path, origin);
    match InodeIdent::of(origin) {
        Ok(now) if now == *expected => {}
        Ok(_) | Err(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: origin.to_path_buf(),
                detail: crate::i18n::format(
                    "files-payload-at-changed-before-twin-substitution-6df00885",
                    &[("arg0", (origin.display()).to_string())],
                ),
            });
        }
    }
    if let Err(detail) = crate::services::rename_reversing(origin, trashed_path) {
        return InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: origin.to_path_buf(),
            detail,
        });
    }
    match InodeIdent::of(trashed_path) {
        Ok(after) if after == *expected => {
            InverseResult::Conflict(InverseConflict::TrashMetadataTwinChanged {
                info_path: info_path.to_path_buf(),
            })
        }
        _ => InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: trashed_path.to_path_buf(),
            detail:
                crate::i18n::tr("twin-substitution rollback rename returned Ok but expected identity is not at trash")
                    .into(),
        }),
    }
}

// Bind-and-verify rollback of the just-landed payload back to
// `origin`. Foreign bytes at `info_path` are never touched.
pub(super) fn rollback_payload_after_publish_failure(
    origin: &Path,
    trashed_path: &Path,
    expected: &InodeIdent,
    publish_conflict: InverseConflict,
    hooks: InverseHooks,
) -> InverseResult {
    (hooks.between_postcheck_and_rollback)(origin, trashed_path);
    match InodeIdent::of(trashed_path) {
        Ok(now) if now == *expected => {}
        Ok(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: trashed_path.to_path_buf(),
                detail: crate::i18n::format(
                    "files-payload-at-changed-before-publish-failure-e36cfe24",
                    &[("arg0", (trashed_path.display()).to_string())],
                ),
            });
        }
        Err(_) => {
            return InverseResult::Conflict(InverseConflict::RollbackUncertain {
                at: trashed_path.to_path_buf(),
                detail: crate::i18n::format(
                    "files-payload-at-disappeared-before-publish-failure-685cefb9",
                    &[("arg0", (trashed_path.display()).to_string())],
                ),
            });
        }
    }
    if let Err(detail) = crate::services::rename_reversing(trashed_path, origin) {
        return InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: trashed_path.to_path_buf(),
            detail,
        });
    }
    match InodeIdent::of(origin) {
        Ok(after) if after == *expected => InverseResult::Conflict(publish_conflict),
        _ => InverseResult::Conflict(InverseConflict::RollbackUncertain {
            at: origin.to_path_buf(),
            detail:
                crate::i18n::tr("publish-failure rollback rename returned Ok but expected identity is not at origin")
                    .into(),
        }),
    }
}
