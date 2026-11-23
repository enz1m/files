// The compact activity status at the bottom of the sidebar.
//
// One sidebar row ("3 downloading", the aggregate percentage) with one thin
// aggregate bar under its label. Activating it opens where the work is
// managed: the Downloads page when every row is a download, the all-activity
// dialog otherwise. Per-operation controls live there, not in the sidebar.
//
// Where the shell floats the activity over content (no inline sidebar), the
// same status sits on an elevated surface so it stays legible.

use enzui_core::operation_activity::downloads::DOWNLOADS;
use enzui_core::operation_activity::{
    ActivityCenter, ActivityOverflowTarget, ActivityProgress, ActivitySummary,
};
use enzui_core::semantics::{Live, NumericValue, Role, Semantics};
use enzui_theme::EnzTheme;
use enzui_widgets::activity_surface::{REGION_ID, summary_sentence};
use enzui_widgets::nav_split::SidebarPresentation;
use enzui_widgets::progress_bar::enz_progress_bar;
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::sidebar_row::{SidebarRowTokens, enz_sidebar_row};
use enzui_widgets::surface::enz_surface;
use iced::widget::{column, container};
use iced::{Element, Length, Padding};

use crate::message::Msg;

// The stable widget id of the status row. The all-activity dialog returns
// focus here when it closes.
pub(crate) const ACTIVITY_STATUS_ID: &str = "files-app-activity-overflow";

// Height of the aggregate bar: a hairline-weight meter, not a control.
const BAR_HEIGHT: f32 = 3.0;

// What the status row says, in priority order: live work first, then what
// needs attention, then outcomes still lingering.
pub(crate) fn headline(summary: &ActivitySummary, downloads_only: bool) -> String {
    let running = summary.running;
    if running > 0 {
        let id = if downloads_only {
            "files-activity-downloading"
        } else {
            "files-activity-in-progress"
        };
        return crate::i18n::format(id, &[("count", running.to_string())]);
    }
    let framework = |key: &str, count: usize| enzui_core::localized!(key, "count" => count);
    if summary.waiting > 0 {
        framework("enzui-activity-surface-waiting", summary.waiting)
    } else if summary.paused > 0 {
        framework("enzui-activity-surface-paused", summary.paused)
    } else if summary.failed > 0 {
        framework("enzui-activity-surface-failed", summary.failed)
    } else if summary.completed > 0 {
        framework("enzui-activity-surface-completed", summary.completed)
    } else {
        framework("enzui-activity-surface-cancelled", summary.cancelled)
    }
}

// Where activating the status goes: the provider's own panel when one
// provider owns every row, the all-activity dialog otherwise.
pub(crate) fn target(center: &ActivityCenter) -> ActivityOverflowTarget {
    let mut providers = Vec::new();
    for row in center.iter() {
        if !providers.contains(&row.key.provider) {
            providers.push(row.key.provider);
        }
    }
    match providers.as_slice() {
        [provider] if *provider == DOWNLOADS => ActivityOverflowTarget::Provider(*provider),
        _ => ActivityOverflowTarget::AllActivity { providers },
    }
}

// The centre withholds an aggregate while any live row has no known size (a
// queued download before its first response). The bar still shows the share
// of the sized byte work, which is what the user is waiting on.
fn known_bytes_fraction(center: &ActivityCenter) -> Option<f32> {
    let (mut done, mut total) = (0u64, 0u64);
    for row in center.iter().filter(|row| !row.phase.is_terminal()) {
        if let ActivityProgress::Bytes {
            done: d,
            total: Some(t),
        } = row.progress
        {
            done = done.saturating_add(d.min(t));
            total = total.saturating_add(t);
        }
    }
    (total > 0).then(|| (done as f64 / total as f64) as f32)
}

pub(crate) fn view<'a>(
    center: &ActivityCenter,
    theme: &'a EnzTheme,
    presentation: SidebarPresentation,
    overlay_open: bool,
) -> Element<'a, Msg> {
    let summary = center.view().summary;
    let target = target(center);
    let downloads_only = matches!(target, ActivityOverflowTarget::Provider(_));
    let failed_only = summary.active() == 0 && summary.failed > 0;
    let icon = if failed_only {
        "circle-alert"
    } else if downloads_only {
        "download"
    } else {
        "activity"
    };
    let fraction = summary
        .fraction
        .filter(|f| f.is_finite())
        .or_else(|| known_bytes_fraction(center));
    let percent = fraction
        .filter(|_| summary.active() > 0)
        .map(|f| {
            let percentage = (f.clamp(0.0, 1.0) * 100.0).round() as u32;
            enzui_core::localized!("enzui-download-percent", "percentage" => percentage)
        })
        .unwrap_or_default();

    let tokens = SidebarRowTokens::from_palette(&theme.palette);
    let row = enz_sidebar_row(
        headline(&summary, downloads_only),
        Msg::ShowActivityOverflow(target),
    )
    .id(iced::advanced::widget::Id::new(ACTIVITY_STATUS_ID))
    .icon(icon)
    .count(percent)
    .show_highlight(false)
    .tokens(tokens)
    .sizing(theme.sizing)
    .into_element();

    let mut body = column![row].width(Length::Fill);
    if let Some(fraction) = fraction.filter(|_| summary.active() > 0) {
        // Under the label, from the label's start to the count's end.
        let lead = tokens.label_inset_x() + tokens.icon_size + tokens.icon_gap;
        let trail = tokens.label_inset_x();
        let (left, right) = if crate::i18n::rtl() {
            (trail, lead)
        } else {
            (lead, trail)
        };
        let mut bar = enz_progress_bar(fraction)
            .bar_height(BAR_HEIGHT)
            .busy(summary.running > 0);
        if summary.running == 0 {
            bar = bar.fill_color(theme.palette.text_subtle);
        }
        body = body.push(
            container(bar.into_element())
                .padding(Padding {
                    top: 0.0,
                    right,
                    bottom: theme.spacing.xs(),
                    left,
                })
                .width(Length::Fill),
        );
    }

    let in_sidebar =
        presentation == SidebarPresentation::Inline || (presentation.is_overlay() && overlay_open);
    let placed: Element<'a, Msg> = if in_sidebar {
        // On the sidebar's own rail, like every other sidebar row.
        container(body)
            .padding(Padding {
                top: 0.0,
                right: theme.spacing.sm(),
                bottom: theme.spacing.sm(),
                left: theme.spacing.sm(),
            })
            .width(Length::Fill)
            .into()
    } else {
        enz_surface(body)
            .padding(theme.spacing.xs())
            .elevation(1)
            .width(Length::Fill)
            .into()
    };

    // A polite status region: the full summary for assistive technology,
    // with the aggregate as its value.
    let mut semantics = Semantics::new(Role::Status)
        .label(enzui_core::localized!(
            "enzui-activity-region",
            "summary" => summary_sentence(&summary)
        ))
        .live(Some(Live::Polite));
    if let Some(fraction) = fraction {
        semantics = semantics.numeric(NumericValue {
            value: f64::from(fraction.clamp(0.0, 1.0)),
            min: 0.0,
            max: 1.0,
            step: None,
        });
    }
    SemanticsContainer::new(placed, semantics)
        .id(iced::advanced::widget::Id::new(REGION_ID))
        .into()
}
