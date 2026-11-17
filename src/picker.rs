// Parent exports and selection authority stay bound to one chooser request.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use enzui_widgets::file_chooser::{
    CancelReason, ChooserOptions, ChooserParent, ChooserSession, FallbackEvent, FileResult,
    PortalOperation, PortalOutcome, PortalWake,
};

// Routes a completed chooser request to its initiating action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerTarget {
    // "Move to…": choose a folder, then move the selection into it.
    MoveSelection,
    // "Copy to…": choose a folder, then copy the selection into it.
    CopySelection,
    LinkSelection,
    // "Extract To…": choose the folder an archive extracts into.
    ExtractArchive,
    // Reviews a local package without installing or running it.
    InstallExtension,
}

impl PickerTarget {
    pub(crate) fn operation(self) -> PortalOperation {
        // No wildcard: a future target must name its own operation.
        match self {
            Self::InstallExtension => PortalOperation::OpenFile,
            Self::MoveSelection
            | Self::CopySelection
            | Self::LinkSelection
            | Self::ExtractArchive => PortalOperation::OpenDirectory,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::MoveSelection => crate::i18n::tr("Move selection to folder"),
            Self::CopySelection => crate::i18n::tr("Copy selection to folder"),
            Self::LinkSelection => crate::i18n::tr("Create links in folder"),
            Self::ExtractArchive => crate::i18n::tr("Extract archive to folder"),
            Self::InstallExtension => crate::i18n::tr("Install a Files extension (.enzfiles)"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PickerRequest {
    pub(crate) target: PickerTarget,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct SelectionContext {
    folder: PathBuf,
    load_generation: u64,
    selection: Vec<PathBuf>,
    visible_selection: Vec<enzui_core::drag::ItemId>,
}

impl SelectionContext {
    fn capture(model: &crate::model::Model) -> Self {
        let mut visible_selection = crate::drag_drop::selected_ids_in_order(model);
        visible_selection.sort();
        let mut selection: Vec<_> = model.selection.iter().cloned().collect();
        selection.sort();
        Self {
            folder: model.cwd.clone(),
            load_generation: model.load_generation,
            selection,
            visible_selection,
        }
    }
}

// Portal/fallback ownership and the app context authorized to consume its result.
#[derive(Debug)]
pub(crate) struct Picker {
    session: ChooserSession<PickerRequest>,
    generation: u64,
    active: Option<(PickerRequest, Option<SelectionContext>)>,
    parenting: Option<(
        PickerRequest,
        ChooserOptions,
        iced::window::Id,
        enzui_core::portal::cancel::PortalCancel,
    )>,
    exports: Option<enzui_widgets::export_pool::ExportPoolOwner>,
    #[cfg(test)]
    scripted: bool,
}

impl Default for Picker {
    fn default() -> Self {
        Self::new()
    }
}

impl Picker {
    pub(crate) fn new() -> Self {
        Self::from_session(ChooserSession::new())
    }

    fn from_session(session: ChooserSession<PickerRequest>) -> Self {
        Self {
            session: session.with_fallback(),
            generation: 0,
            active: None,
            parenting: None,
            exports: None,
            #[cfg(test)]
            scripted: false,
        }
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.active.is_some() || self.session.is_pending()
    }

    pub(crate) fn start(
        &mut self,
        target: PickerTarget,
        model: &crate::model::Model,
        window: Option<iced::window::Id>,
    ) -> iced::Task<crate::message::Msg> {
        use crate::message::Msg;
        use enzui_widgets::portal_parent::{self, ParentExport};
        if self.is_pending() {
            return iced::Task::none();
        }
        let Some(generation) = self.generation.checked_add(1) else {
            return iced::Task::none();
        };
        let request = PickerRequest { target, generation };
        let context = matches!(
            target,
            PickerTarget::MoveSelection | PickerTarget::CopySelection | PickerTarget::LinkSelection
        )
        .then(|| SelectionContext::capture(model));
        let options = ChooserOptions::new()
            .title(target.title())
            .current_folder(model.cwd.clone());
        self.generation = generation;
        self.active = Some((request, context));
        #[cfg(test)]
        if self.scripted {
            return match self.session.start(request, target.operation(), options) {
                Ok(()) => iced::Task::none(),
                Err(outcome) => iced::Task::done(Msg::PortalOutcome(request, outcome)),
            };
        }
        let Some(window) = window else {
            return iced::Task::done(Msg::PortalOutcome(
                request,
                PortalOutcome::ParentUnavailable,
            ));
        };
        let cancel = enzui_core::portal::cancel::PortalCancel::new();
        self.parenting = Some((request, options, window, cancel.clone()));
        let pool = self
            .exports
            .get_or_insert_with(|| {
                enzui_widgets::export_pool::ExportPoolOwner::start()
                    .unwrap_or_else(|_| enzui_widgets::export_pool::ExportPoolOwner::unavailable())
            })
            .dispatcher();
        iced::window::run(window, |window| {
            window
                .window_handle()
                .ok()
                .map_or(ParentExport::Unsupported, |handle| {
                    portal_parent::export_parent(&handle.as_raw())
                })
        })
        .then(move |export| match export {
            ParentExport::Ready(handle) => iced::Task::done(Msg::PickerParentReady(
                request,
                ParentReply::new(Ok(ChooserParent::X11 { window, handle })),
            )),
            ParentExport::NeedsWaylandExport => portal_parent::export_parent_task(
                &pool,
                generation,
                window,
                cancel.clone(),
                Duration::from_secs(3),
            )
            .map(move |handoff| {
                let parent = handoff
                    .take()
                    .and_then(|settled| settled.into_result().ok())
                    .and_then(|adoption| match adoption {
                        enzui_wayland::ParentAdoption::Parented { export, .. } => {
                            Some(ChooserParent::Wayland { window, export })
                        }
                        _ => None,
                    })
                    .ok_or(PortalOutcome::ParentUnavailable);
                Msg::PickerParentReady(request, ParentReply::new(parent))
            }),
            ParentExport::Unsupported => iced::Task::done(Msg::PickerParentReady(
                request,
                ParentReply::new(Err(PortalOutcome::ParentUnavailable)),
            )),
        })
    }

    pub(crate) fn parent_ready(
        &mut self,
        request: PickerRequest,
        reply: ParentReply,
        live_window: Option<iced::window::Id>,
    ) -> iced::Task<crate::message::Msg> {
        if !self
            .parenting
            .as_ref()
            .is_some_and(|(pending, _, _, _)| *pending == request)
        {
            return iced::Task::none();
        }
        let Some(parent) = reply.take() else {
            return iced::Task::none();
        };
        let (_, options, window, _) = self.parenting.take().expect("matched parent request");
        let outcome = if live_window == Some(window) {
            parent.and_then(|parent| {
                self.session.start_parented(
                    request,
                    request.target.operation(),
                    options,
                    parent,
                    |id| Some(id) == live_window,
                )
            })
        } else {
            Err(PortalOutcome::ParentUnavailable)
        };
        match outcome {
            Ok(()) => iced::Task::none(),
            Err(outcome) => iced::Task::done(crate::message::Msg::PortalOutcome(request, outcome)),
        }
    }

    pub(crate) fn settle(
        &mut self,
        request: PickerRequest,
        model: &crate::model::Model,
    ) -> Option<(PickerTarget, bool)> {
        if !self
            .active
            .as_ref()
            .is_some_and(|(active, _)| *active == request)
        {
            return None;
        }
        let (_, context) = self.active.take()?;
        Some((
            request.target,
            context.is_none_or(|snapshot| snapshot == SelectionContext::capture(model)),
        ))
    }

    pub(crate) fn completion_subscription(&self) -> iced::Subscription<PortalWake> {
        self.session.subscription()
    }

    pub(crate) fn pump(&mut self) -> Option<(PickerRequest, PortalOutcome<FileResult>)> {
        let mut captured = None;
        let _task: iced::Task<()> = self.session.pump(|tag, outcome| {
            captured = Some((tag, outcome));
        });
        captured
    }

    pub(crate) fn view<'a>(
        &'a self,
        content: iced::Element<'a, crate::message::Msg>,
    ) -> iced::Element<'a, crate::message::Msg> {
        self.session
            .view(content, crate::message::Msg::PickerFallback)
    }

    pub(crate) fn handle(&mut self, event: FallbackEvent) -> iced::Task<crate::message::Msg> {
        self.session
            .handle(event, crate::message::Msg::PortalOutcome)
    }

    pub(crate) fn cancel(&mut self) {
        self.active = None;
        if let Some((_, _, window, cancel)) = self.parenting.take() {
            cancel.cancel(CancelReason::ParentClosed);
            let _ = self.session.parent_closed(window);
        }
        let _ = self.session.cancel(CancelReason::ParentClosed);
    }
}

type ParentResult = Result<ChooserParent, PortalOutcome<FileResult>>;

#[derive(Clone, Debug)]
pub(crate) struct ParentReply(Arc<Mutex<Option<ParentResult>>>);

impl ParentReply {
    fn new(parent: ParentResult) -> Self {
        Self(Arc::new(Mutex::new(Some(parent))))
    }

    fn take(&self) -> Option<ParentResult> {
        self.0.lock().ok().and_then(|mut parent| parent.take())
    }
}

#[cfg(test)]
pub(crate) mod test_backend {
    // An in-process [`PortalBackend`] for reducer tests, with no D-Bus or dialog.
    //
    // It records the operation, title and parent handle of each request and
    // scripts one reply.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_destination_rejects_selection_changes_and_replayed_replies() {
        let mut model = crate::model::Model::new(std::env::temp_dir());
        let mut picker = Picker::new();
        let _ = picker.start(PickerTarget::LinkSelection, &model, None);
        let request = picker.active.as_ref().unwrap().0;
        model.selection.insert(model.cwd.join("different-file"));
        assert_eq!(
            picker.settle(request, &model),
            Some((PickerTarget::LinkSelection, false))
        );
        assert_eq!(picker.settle(request, &model), None);
    }
}
