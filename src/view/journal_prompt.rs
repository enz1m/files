// Banner surface for Undo/Redo journal conflicts. Each active
// [`crate::journal::JournalPrompt`] renders one row with Retry,
// Skip and Retire buttons.
//
// Every button emits [`crate::message::Msg::ResolveJournalConflict`]
// carrying the prompt's captured `(entry_id, entry_generation)` so
// a click on a superseded entry is a typed no-op.

use crate::journal::{JournalConflictChoice, JournalPrompt};
use crate::message::Msg;
use crate::model::Model;
use enzui_widgets::button::{ButtonKind, enz_button};
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, Padding};

// Overlay to stack above the presentation, or `None` when the
// model has no live prompts.
pub(crate) fn overlay_if_any<'a>(model: &'a Model) -> Option<Element<'a, Msg>> {
    if model.journal_prompts.is_empty() {
        return None;
    }
    let stack: Element<'_, Msg> = column(
        model
            .journal_prompts
            .iter()
            .map(banner_row)
            .collect::<Vec<_>>(),
    )
    .spacing(8)
    .into();
    Some(
        container(stack)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding(Padding {
                top: 16.0,
                right: 16.0,
                bottom: 0.0,
                left: 16.0,
            })
            .into(),
    )
}

fn banner_row<'a>(prompt: &'a JournalPrompt) -> Element<'a, Msg> {
    let head = enz_text(prompt.headline.clone()).size(TextSize::Headline);
    let detail = enz_text(prompt.detail.clone())
        .size(TextSize::Footnote)
        .tone(TextTone::Muted);
    let retry = enz_button(
        crate::i18n::tr("Retry"),
        Msg::ResolveJournalConflict {
            entry_id: prompt.entry_id,
            entry_generation: prompt.entry_generation,
            choice: JournalConflictChoice::Retry,
        },
    )
    .kind(ButtonKind::Accent);
    let skip = enz_button(
        crate::i18n::tr("Skip"),
        Msg::ResolveJournalConflict {
            entry_id: prompt.entry_id,
            entry_generation: prompt.entry_generation,
            choice: JournalConflictChoice::Skip,
        },
    )
    .kind(ButtonKind::Quiet);
    let retire = enz_button(
        crate::i18n::tr("Retire"),
        Msg::ResolveJournalConflict {
            entry_id: prompt.entry_id,
            entry_generation: prompt.entry_generation,
            choice: JournalConflictChoice::Retire,
        },
    )
    .kind(ButtonKind::Quiet);
    let buttons: Element<'_, Msg> = row![retry, skip, retire]
        .mirrored(crate::i18n::rtl())
        .spacing(8)
        .into();
    container(column![head, detail, buttons].spacing(6))
        .padding(Padding::new(12.0))
        .width(Length::Fill)
        .into()
}
