// Application wiring for the shared operation activity center.
//
// File operations do not expose streaming byte counts, so they publish the
// item count known at dispatch and remain honestly indeterminate within each
// item. Downloads use the framework adapter directly.

use enzui_core::operation_activity::{
    Activity, ActivityCenter, ActivityControl, ActivityKey, ActivityKind, ActivityProgress,
    Generation, ProviderId, PublishOutcome, TerminalOutcome,
};

pub(crate) const FILES: ProviderId = ProviderId("files");

// Identity carried from an off-thread file operation's dispatch to its reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileActivity {
    pub(crate) key: ActivityKey,
    pub(crate) generation: Generation,
}

pub(crate) fn start(
    center: &mut ActivityCenter,
    next_id: &mut u64,
    kind: ActivityKind,
    label: impl Into<String>,
    items: usize,
    now_ms: u64,
) -> FileActivity {
    let activity = FileActivity {
        key: ActivityKey::new(FILES.0, *next_id),
        generation: Generation::FIRST,
    };
    *next_id = (*next_id).saturating_add(1);
    let outcome = center.publish(
        Activity::new(activity.key, kind, label)
            .generation(activity.generation)
            .progress(ActivityProgress::Items {
                done: 0,
                total: Some(items as u64),
            }),
        now_ms,
    );
    debug_assert!(
        matches!(outcome, PublishOutcome::Added),
        "new operation key must publish exactly one activity"
    );
    activity
}

pub(crate) fn finish(
    center: &mut ActivityCenter,
    activity: FileActivity,
    completed_items: u64,
    outcome: TerminalOutcome,
    detail: impl Into<String>,
    now_ms: u64,
) -> PublishOutcome {
    // The center does not force failed or cancelled rows to 100%, so partial work
    // stays truthful while the row lingers.
    if let Some(row) = center.get(activity.key).cloned() {
        let progress = match row.progress {
            ActivityProgress::Items { total, .. } => ActivityProgress::Items {
                done: completed_items,
                total,
            },
            other => other,
        };
        let _ = center.publish(row.detail(detail).progress(progress), now_ms);
    }
    center.finish(activity.key, activity.generation, outcome, now_ms)
}

// Offers Cancel on a live row whose operation honours it.
pub(crate) fn offer_cancel(center: &mut ActivityCenter, activity: FileActivity, now_ms: u64) {
    if let Some(row) = center.get(activity.key).cloned() {
        let _ = center.publish(row.controls([ActivityControl::Cancel]), now_ms);
    }
}
