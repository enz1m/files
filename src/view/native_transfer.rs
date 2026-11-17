//! Native file gestures share the same selection and validated paste reducer.
use crate::{
    message::Msg,
    model::{FileEntry, Model},
};
use iced::Element;
use std::path::PathBuf;

pub(super) fn target<'a>(
    model: &Model,
    path: PathBuf,
    content: Element<'a, Msg>,
) -> Element<'a, Msg> {
    target_if(model, path, content, true)
}

pub(super) fn target_if<'a>(
    model: &Model,
    path: PathBuf,
    content: Element<'a, Msg>,
    active: bool,
) -> Element<'a, Msg> {
    let enabled = active
        && !crate::services::is_virtual(&path)
        && model.screen == crate::model::Screen::Directory
        && model.inline_rename.is_none();
    let key = (path, model.load_generation);
    let content: Element<'a, Msg> =
        enzui_widgets::file_drop::enz_file_drop_target_with_receipt(content, {
            let key = key.clone();
            move |drop| Msg::NativeFilesDropped {
                drop,
                target: key.clone(),
            }
        })
        .enabled(enabled)
        .preferred_action(enzui_widgets::file_drop::DragAction::Move)
        .key(&key)
        .into();
    enzui_widgets::file_drop::enz_data_drop_target(
        content,
        crate::native_drop::content_preferences(),
        {
            let key = key.clone();
            move |drop| Msg::NativeContentDropped {
                drop,
                target: key.clone(),
            }
        },
    )
    .enabled(enabled)
    .key(key)
    .into()
}

pub(super) fn entry<'a>(
    model: &Model,
    entry: &FileEntry,
    content: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let paths = if model.inline_rename.is_some() || model.screen != crate::model::Screen::Directory
    {
        Vec::new()
    } else if model.is_selected(&entry.path) {
        crate::drag_drop::selected_ids_in_order(model)
            .iter()
            .filter_map(crate::drag_drop::id_to_path)
            .collect::<Vec<_>>()
    } else {
        vec![entry.path.clone()]
    };
    let source = enzui_widgets::file_drag::enz_file_drag_source(content, paths)
        .key((&entry.path, model.load_generation))
        .actions(enzui_widgets::file_drag::DragActions::BOTH)
        .allows_captured_press(true)
        .on_finished(|_| Msg::Refresh)
        .into();
    if crate::drag_drop::accepts_drop(entry.kind) {
        target(model, entry.path.clone(), source)
    } else {
        source
    }
}
