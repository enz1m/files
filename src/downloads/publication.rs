// Publication: custody, debt, reconciliation, and payload.

use super::*;

// Custody state retained after a publication failure.
#[derive(Debug)]
pub(crate) enum PublicationCustody {
    // Source entry was never moved; retry against same output dir is safe.
    Source(enzui_security::secure_dir::RetainedEntry),
    // Destination or mixed custody; the move partially completed.
    // Generic stage cleanup must not erase source bytes while this is live.
    Committed(enzui_security::secure_dir::MoveCustody),
}

// Publication debt with every retained handle, shared between the worker and
// the reducer. Reconciliation consumes or preserves the handles.
#[derive(Debug)]
pub(crate) struct PublicationDebt {
    // The entry that failed and its custody; `None` once reconciled.
    pub(crate) failed: Option<PublicationCustody>,
    // Remaining entries not yet attempted (retained descriptors).
    pub(crate) remaining: Vec<enzui_security::secure_dir::RetainedEntry>,
    // How many entries were successfully moved before the failure.
    pub(crate) moved: u64,
    pub(crate) total: u64,
    pub(crate) detail: String,
    // Set permanently when an unwind dropped an in-flight descriptor. The debt
    // can never complete, and cleanup, retry, process exit and new transfers stay
    // blocked until manual recovery or restart.
    pub(crate) custody_lost: bool,
}

// Shared publication debt written by the worker, read by the reducer.
pub(crate) type SharedPublicationDebt = Arc<Mutex<Option<PublicationDebt>>>;

// Result of publication reconciliation on the bounded worker.
#[derive(Clone, Debug)]
pub(crate) struct PublicationSettlement {
    pub(crate) id: DownloadId,
    pub(crate) attempt: Attempt,
    // True when every entry is published and the debt is cleared.
    pub(crate) complete: bool,
    pub(crate) detail: Option<String>,
}

// A panic-safe lease on publication debt during reconciliation.
//
// Dropping it restores the debt unless committed. Each fallible step updates
// the inner debt first, so an unwind leaves the latest debt in the mutex.
pub(super) struct ReconciliationLease<'a> {
    pub(super) guard: &'a mut Option<PublicationDebt>,
    pub(super) debt: Option<PublicationDebt>,
    // Set while a descriptor is inside a fallible operation; a drop while set
    // marks `custody_lost` permanently.
    pub(super) in_flight: bool,
}

impl<'a> ReconciliationLease<'a> {
    pub(super) fn new(guard: &'a mut Option<PublicationDebt>) -> Option<Self> {
        let debt = guard.take()?;
        Some(Self {
            guard,
            debt: Some(debt),
            in_flight: false,
        })
    }

    pub(super) fn debt_mut(&mut self) -> &mut PublicationDebt {
        self.debt.as_mut().expect("lease consumed after commit")
    }

    // Marks a descriptor as entering a fallible operation; `land()` must clear
    // it before the next.
    pub(super) fn enter_flight(&mut self) {
        debug_assert!(!self.in_flight, "nested in_flight");
        self.in_flight = true;
    }

    // Marks the in-flight operation finished, with the descriptor consumed or
    // stored back.
    fn land(&mut self) {
        self.in_flight = false;
    }

    // Clears the debt on completion, making drop a no-op. Debug-panics on
    // violated preconditions.
    pub(super) fn commit(mut self) {
        if let Some(d) = &self.debt {
            debug_assert!(!d.custody_lost, "commit with custody_lost");
            debug_assert!(d.failed.is_none(), "commit with failed entry");
            debug_assert!(d.remaining.is_empty(), "commit with remaining");
            debug_assert_eq!(d.moved, d.total, "commit with moved != total");
        }
        self.debt = None;
    }
}

impl Drop for ReconciliationLease<'_> {
    fn drop(&mut self) {
        if let Some(mut debt) = self.debt.take() {
            if self.in_flight {
                debt.custody_lost = true;
                debt.detail =
                    crate::i18n::tr("publication custody lost: entry descriptor dropped by unwind")
                        .into();
            }
            *self.guard = Some(debt);
        }
    }
}

pub(super) fn run_publication_reconciliation(
    debt_arc: &SharedPublicationDebt,
    output_dir: &enzui_security::secure_dir::RetainedDir,
    id: DownloadId,
    attempt: Attempt,
) -> PublicationSettlement {
    use enzui_security::secure_dir::MoveCustody;

    let mut guard = debt_arc
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut lease) = ReconciliationLease::new(&mut guard) else {
        return PublicationSettlement {
            id,
            attempt,
            complete: true,
            detail: None,
        };
    };

    // Lost custody blocks further moves; the debt stays visible.
    if lease.debt_mut().custody_lost {
        let detail = lease.debt_mut().detail.clone();
        drop(lease);
        return PublicationSettlement {
            id,
            attempt,
            complete: false,
            detail: Some(detail),
        };
    }

    // Mark custody in flight before taking the entry so an unwind records its loss.
    lease.enter_flight();
    let failed = lease.debt_mut().failed.take();
    let reconciled = match failed {
        None => {
            lease.land();
            true
        }
        Some(PublicationCustody::Source(entry)) => {
            let raw = entry.name().to_os_string();
            let display =
                safe_display_component(&raw.to_string_lossy()).unwrap_or_else(|| "Download".into());
            let result = move_entry_noreplace(entry, output_dir, &display);
            lease.land();
            match result {
                Ok(_dest) => {
                    lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                    true
                }
                Err(custody) => {
                    lease.debt_mut().failed = Some(*custody);
                    lease.debt_mut().detail =
                        crate::i18n::tr("retry of source entry failed").into();
                    false
                }
            }
        }
        Some(PublicationCustody::Committed(MoveCustody::Destination(entry))) => {
            let result = entry.verify();
            lease.land();
            match result {
                Ok(()) => {
                    lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                    true
                }
                Err(e) => {
                    lease.debt_mut().failed = Some(PublicationCustody::Committed(
                        MoveCustody::Destination(entry),
                    ));
                    lease.debt_mut().detail = crate::i18n::format(
                        "files-destination-verify-failed-e-5f8a217f",
                        &[("e", (e).to_string())],
                    );
                    false
                }
            }
        }
        Some(PublicationCustody::Committed(MoveCustody::Both {
            source,
            destination,
        })) => {
            let dest_result = destination.verify();
            match dest_result {
                Ok(()) => {
                    let src_result = source.remove_if_identity();
                    lease.land();
                    match src_result {
                        Ok(true | false) => {
                            lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                            true
                        }
                        Err(e) => {
                            lease.debt_mut().failed =
                                Some(PublicationCustody::Committed(MoveCustody::Both {
                                    source,
                                    destination,
                                }));
                            lease.debt_mut().detail = crate::i18n::format(
                                "files-source-removal-after-both-failed-e-994b51f6",
                                &[("e", (e).to_string())],
                            );
                            false
                        }
                    }
                }
                Err(e) => {
                    lease.land();
                    lease.debt_mut().failed =
                        Some(PublicationCustody::Committed(MoveCustody::Both {
                            source,
                            destination,
                        }));
                    lease.debt_mut().detail = crate::i18n::format(
                        "files-destination-verify-in-both-failed-e-9054f7c3",
                        &[("e", (e).to_string())],
                    );
                    false
                }
            }
        }
        Some(PublicationCustody::Committed(MoveCustody::Uncertain {
            source,
            destination,
        })) => {
            // `verify()` borrows; both handles stay out of the debt, so the lease
            // stays in flight until results are stored back.
            let dest_ok = destination.verify().is_ok();
            let src_ok = source.verify().is_ok();
            match (dest_ok, src_ok) {
                (true, false) => {
                    lease.land();
                    lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                    true
                }
                (false, true) => {
                    // `move_entry_noreplace` consumes `source`.
                    let raw = source.name().to_os_string();
                    let display = safe_display_component(&raw.to_string_lossy())
                        .unwrap_or_else(|| "Download".into());
                    let result = move_entry_noreplace(source, output_dir, &display);
                    lease.land();
                    match result {
                        Ok(_) => {
                            lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                            true
                        }
                        Err(custody) => {
                            lease.debt_mut().failed = Some(*custody);
                            lease.debt_mut().detail =
                                crate::i18n::tr("uncertain->source retry failed").into();
                            false
                        }
                    }
                }
                _ => {
                    lease.land();
                    lease.debt_mut().failed =
                        Some(PublicationCustody::Committed(MoveCustody::Uncertain {
                            source,
                            destination,
                        }));
                    lease.debt_mut().detail =
                        crate::i18n::tr("uncertain placement: both or neither verify").into();
                    false
                }
            }
        }
        Some(PublicationCustody::Committed(MoveCustody::Source(entry))) => {
            let raw = entry.name().to_os_string();
            let display =
                safe_display_component(&raw.to_string_lossy()).unwrap_or_else(|| "Download".into());
            let result = move_entry_noreplace(entry, output_dir, &display);
            lease.land();
            match result {
                Ok(_dest) => {
                    lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
                    true
                }
                Err(custody) => {
                    lease.debt_mut().failed = Some(*custody);
                    lease.debt_mut().detail =
                        crate::i18n::tr("retry of committed-source entry failed").into();
                    false
                }
            }
        }
    };

    if !reconciled {
        let detail = lease.debt_mut().detail.clone();
        drop(lease); // Drop restores debt into guard.
        return PublicationSettlement {
            id,
            attempt,
            complete: false,
            detail: Some(detail),
        };
    }

    // Pop individually so an unwind loses at most the in-flight entry.
    loop {
        // In flight before the pop, so an unwind sets `custody_lost`.
        lease.enter_flight();
        let Some(entry) = lease.debt_mut().remaining.pop() else {
            lease.land();
            break;
        };
        let raw = entry.name().to_os_string();
        let display =
            safe_display_component(&raw.to_string_lossy()).unwrap_or_else(|| "Download".into());
        let result = move_entry_noreplace(entry, output_dir, &display);
        lease.land();
        match result {
            Ok(_dest) => {
                lease.debt_mut().moved = lease.debt_mut().moved.saturating_add(1);
            }
            Err(custody) => {
                lease.debt_mut().failed = Some(*custody);
                let debt = lease.debt_mut();
                debt.detail = crate::i18n::format(
                    "files-remaining-entry-failed-at-75966b07",
                    &[
                        ("arg0", (debt.moved).to_string()),
                        ("arg1", (debt.total).to_string()),
                    ],
                );
                let detail = debt.detail.clone();
                drop(lease);
                return PublicationSettlement {
                    id,
                    attempt,
                    complete: false,
                    detail: Some(detail),
                };
            }
        }
    }

    lease.commit();
    PublicationSettlement {
        id,
        attempt,
        complete: true,
        detail: None,
    }
}

// Failed moves preserve descriptor custody for reconciliation.
fn move_entry_noreplace(
    mut entry: enzui_security::secure_dir::RetainedEntry,
    dest_dir: &enzui_security::secure_dir::RetainedDir,
    display_name: &str,
) -> Result<enzui_security::secure_dir::RetainedEntry, Box<PublicationCustody>> {
    use enzui_security::secure_dir::MoveCustody;
    use std::ffi::OsString;

    for n in 1..10_000u32 {
        let candidate = if n == 1 {
            OsString::from(display_name)
        } else {
            let path = Path::new(display_name);
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Download");
            match path.extension().and_then(|s| s.to_str()) {
                Some(ext) => OsString::from(format!("{stem} ({n}).{ext}")),
                None => OsString::from(format!("{stem} ({n})")),
            }
        };
        match entry.move_noreplace(dest_dir, &candidate) {
            Ok(moved) => return Ok(moved),
            Err(failure) => {
                let is_exists = failure.error.kind() == std::io::ErrorKind::AlreadyExists;
                match *failure.custody {
                    MoveCustody::Source(returned) if is_exists => {
                        entry = returned;
                    }
                    MoveCustody::Source(returned) => {
                        return Err(Box::new(PublicationCustody::Source(returned)));
                    }
                    other => return Err(Box::new(PublicationCustody::Committed(other))),
                }
            }
        }
    }
    Err(Box::new(PublicationCustody::Source(entry)))
}

// Bounded limits for stage publication enumeration.
const PUBLISH_MAX_ENTRIES: usize = 10_000;
const PUBLISH_MAX_NAME_BYTES: usize = 2_560_000;
pub(super) const PUBLISH_MAX_DEPTH: usize = 64;

// Result of a completed or partially completed publication.
pub(super) enum PublicationOutcome {
    // All entries moved and stage cleanup complete.
    Complete {
        bytes: u64,
    },
    // All entries moved but stage cleanup did not finish.
    CompleteCleanupPending {
        bytes: u64,
    },
    // Some entries moved before a failure; custody is retained.
    Partial {
        moved: u64,
        remaining: u64,
        error: TransferError,
    },
}

pub(super) fn publish_owned_payload(
    job: &TransferJob,
) -> Result<PublicationOutcome, TransferError> {
    let output_dir = job
        .torrent_output_dir
        .as_ref()
        .ok_or_else(|| TransferError::Io {
            category: IoFailureCategory::Unknown,
            detail: crate::i18n::tr("torrent job has no retained output directory").into(),
        })?;
    let shared = job
        .torrent_shared_stage
        .as_ref()
        .ok_or_else(|| TransferError::Io {
            category: IoFailureCategory::Unknown,
            detail: crate::i18n::tr("torrent job has no stage authority").into(),
        })?;
    let stage_dir = {
        let guard = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stage = guard.as_ref().ok_or_else(|| TransferError::Io {
            category: IoFailureCategory::Unknown,
            detail: crate::i18n::tr("torrent stage authority was already consumed").into(),
        })?;
        stage
            .root()
            .ordinary_directory()
            .map_err(|e| TransferError::from_io(&e))?
    };
    let limits = enzui_security::secure_dir::EnumerationLimits {
        max_entries: PUBLISH_MAX_ENTRIES,
        max_name_bytes: PUBLISH_MAX_NAME_BYTES,
    };
    let entries = stage_dir
        .retain_entries_bounded(&limits)
        .map_err(|e| TransferError::from_io(&e))?;
    // Measured after the backend joined and before the stage is emptied.
    let bytes = stage_dir
        .logical_size_bounded(&enzui_security::secure_dir::TreeSizeLimits {
            max_entries: PUBLISH_MAX_ENTRIES,
            max_name_bytes: PUBLISH_MAX_NAME_BYTES,
            max_depth: PUBLISH_MAX_DEPTH,
        })
        .map_err(|e| TransferError::from_io(&e))?;
    let total = entries.len() as u64;
    let mut moved: u64 = 0;
    let mut iter = entries.into_iter();
    while let Some(entry) = iter.next() {
        let raw_name = entry.name().to_os_string();
        let display = safe_display_component(&raw_name.to_string_lossy())
            .unwrap_or_else(|| "Download".into());
        match move_entry_noreplace(entry, output_dir, &display) {
            Ok(_dest) => {
                moved = moved.saturating_add(1);
            }
            Err(custody) => {
                let detail_str = match custody.as_ref() {
                    PublicationCustody::Source(_) => {
                        crate::i18n::tr("move failed with source custody")
                    }
                    PublicationCustody::Committed(
                        enzui_security::secure_dir::MoveCustody::Destination(_),
                    ) => crate::i18n::tr("move completed but source unlink failed"),
                    PublicationCustody::Committed(
                        enzui_security::secure_dir::MoveCustody::Both { .. },
                    ) => crate::i18n::tr("copy published but source removal failed"),
                    PublicationCustody::Committed(
                        enzui_security::secure_dir::MoveCustody::Uncertain { .. },
                    ) => crate::i18n::tr("rename succeeded but destination identity unverified"),
                    PublicationCustody::Committed(
                        enzui_security::secure_dir::MoveCustody::Source(_),
                    ) => crate::i18n::tr("move failed with source custody"),
                };
                let detail = crate::i18n::format(
                    "files-could-not-publish-detail-str-b0f186f5",
                    &[
                        ("arg0", (raw_name.to_string_lossy()).to_string()),
                        ("detail_str", (detail_str).to_owned()),
                    ],
                );
                let remaining_entries: Vec<_> = iter.collect();
                let remaining_count = remaining_entries.len() as u64;
                *job.publication_debt
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PublicationDebt {
                    failed: Some(*custody),
                    remaining: remaining_entries,
                    moved,
                    total,
                    detail: detail.clone(),
                    custody_lost: false,
                });
                return Ok(PublicationOutcome::Partial {
                    moved,
                    remaining: remaining_count + 1,
                    error: TransferError::Io {
                        category: IoFailureCategory::Unknown,
                        detail,
                    },
                });
            }
        }
    }
    let cleanup_complete = try_cleanup_stage(job);
    if cleanup_complete {
        consume_cleaned_stage(job);
        Ok(PublicationOutcome::Complete { bytes })
    } else {
        Ok(PublicationOutcome::CompleteCleanupPending { bytes })
    }
}
