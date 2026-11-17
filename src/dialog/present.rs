// Delete confirmation, presentation shell, and view indices.

use super::*;
use iced::widget::column;

// The title and accessible description of a permanent-delete confirmation.
//
// Reads only the captured request, so a selection change while the dialog is open
// cannot alter the summary. The body is also the dialog's accessible description.
pub(crate) fn delete_confirm_copy(request: &crate::trash::DeleteRequest) -> (String, String) {
    match request {
        crate::trash::DeleteRequest::Files(items) => {
            let title = match items.len() {
                0 => crate::i18n::tr("Delete permanently?").to_owned(),
                1 => crate::i18n::format(
                    "files-permanently-delete-98308767",
                    &[("arg0", (items[0].name).clone())],
                ),
                n => crate::i18n::format(
                    "files-permanently-delete-n-items-190e7166",
                    &[("n", (n).to_string())],
                ),
            };
            let pairs: Vec<(String, bool)> = items
                .iter()
                .map(|item| (item.name.clone(), item.is_dir))
                .collect();
            let mut body = String::from(delete_warning(items.len()));
            for line in summary_item_lines(&pairs) {
                body.push('\n');
                body.push_str(&line);
            }
            (title, body)
        }
        // Empty Trash has no top-level prefix to name.
        crate::trash::DeleteRequest::Trash {
            entries: _,
            empty_all: true,
        } => (
            crate::i18n::tr("Empty Trash?").to_owned(),
            crate::i18n::tr(
                "All items in Trash will be permanently deleted and cannot be recovered.",
            )
            .to_owned(),
        ),
        crate::trash::DeleteRequest::Trash { entries, .. } => {
            let title = match entries.as_slice() {
                [entry] => crate::i18n::format(
                    "files-permanently-delete-98308767",
                    &[("arg0", (entry.name).clone())],
                ),
                entries => crate::i18n::format(
                    "files-permanently-delete-items-a48b572b",
                    &[("arg0", (entries.len()).to_string())],
                ),
            };
            // Trash rows carry no kind, so they list as plain names.
            let pairs: Vec<(String, bool)> = entries
                .iter()
                .map(|entry| (entry.name.clone(), false))
                .collect();
            let mut body = String::from(delete_warning(entries.len()));
            for line in summary_item_lines(&pairs) {
                body.push('\n');
                body.push_str(&line);
            }
            (title, body)
        }
    }
}

// The permanent-delete confirmation dialog, using [`enz_confirm_dialog`].
//
// Every exit (default, cancel, Escape, scrim) arrives as one
// `ConfirmDialogEvent::Resolved(cause)`, which [`Msg::DeleteConfirmEvent`] maps
// through [`enzui_core::confirm::ConfirmDecision::from_cause`].
pub(crate) fn delete_confirm<'a>(
    request: Option<&'a crate::trash::DeleteRequest>,
    state: &'a enzui_core::confirm::ConfirmState,
    background: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let Some(request) = request else {
        return enzui_widgets::modal_host::enz_modal_host(background, None, |event| {
            Msg::DeleteConfirmEvent(
                enzui_widgets::confirm_dialog::ConfirmDialogEvent::from_host(event),
            )
        })
        .scope(enzui_widgets::modal_host::presentation_scope_id(
            DELETE_CONFIRM_SCOPE,
        ))
        .into_element();
    };
    let (title, message) = delete_confirm_copy(request);
    enz_confirm_dialog(title, message, state, Msg::DeleteConfirmEvent)
        .confirm_label(crate::i18n::tr("Delete"))
        .cancel_label(crate::i18n::tr("Cancel"))
        .destructive(true)
        .alert(true)
        .into_scoped_host(
            background,
            enzui_widgets::modal_host::presentation_scope_id(DELETE_CONFIRM_SCOPE),
        )
}

pub(crate) struct Action {
    pub(crate) label: String,
    pub(crate) kind: ButtonKind,
    pub(crate) message: Msg,
    // Whether the button is pressable; defaults to `true`. Set false when the
    // commit is illegal in the current state, such as an empty download URL.
    pub(crate) enabled: bool,
}

impl Action {
    pub(crate) fn new(label: impl Into<String>, kind: ButtonKind, message: Msg) -> Self {
        Self {
            label: label.into(),
            kind,
            message,
            enabled: true,
        }
    }

    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

pub(crate) fn present<'a>(
    theme: &EnzTheme,
    title: impl Into<String>,
    body: Element<'a, Msg>,
    actions: Vec<Action>,
    window: Size,
) -> Element<'a, Msg> {
    present_sized(
        theme,
        title,
        body,
        actions,
        theme.sizing.dialog_width_md,
        window,
    )
}

// [`present()`] with an explicit surface width instead of `dialog_width_md`.
pub(super) fn present_sized<'a>(
    theme: &EnzTheme,
    title: impl Into<String>,
    body: Element<'a, Msg>,
    actions: Vec<Action>,
    width: f32,
    window: Size,
) -> Element<'a, Msg> {
    let title = title.into();

    let sp = &theme.spacing;
    let mut button_row = enzui_widgets::flow::enz_flow()
        .gap(enzui_theme::SpaceToken::Sm)
        .align_items(Alignment::Center);
    for action in actions {
        button_row = button_row.push(
            enz_button(action.label, action.message)
                .kind(action.kind)
                .enabled(action.enabled),
        );
    }

    // The measured body excludes surface padding and the host's window margin.
    let surface_radius = enzui_theme::text_defaults::shape_radius(|shape| shape.dialog);
    let header_inset = enzui_widgets::modal_surface::close_inset(surface_radius);
    let viewport_height = if window.height.is_finite() && window.height > 0.0 {
        window.height
    } else {
        900.0
    };
    let content_max_height = (viewport_height
        - header_inset
        - sp.space9
        - 2.0 * enzui_widgets::modal_host::DIALOG_WINDOW_MARGIN)
        .max(0.0);
    let heading: Element<'a, Msg> = iced::widget::container(
        enz_text(title)
            .size(TextSize::Title2)
            .tone(TextTone::Default)
            .heading(HeadingLevel::Page)
            .into_element(),
    )
    .height(enzui_widgets::modal_surface::CLOSE_CONTROL_SIZE)
    .align_y(Alignment::Center)
    .into();
    let scrolling_body: Element<'a, Msg> = enz_scroll_view(body).height(Length::Fill).into();
    let content = enz_capped_dialog_body(
        vec![heading],
        scrolling_body,
        vec![
            iced::widget::container(button_row.into_element())
                .width(Length::Fill)
                .align_x(Alignment::End)
                .into(),
        ],
        sp.lg(),
        content_max_height,
    )
    .into_element();

    enz_modal_surface(content)
        .on_dismiss(|| {
            Msg::DialogEvent(ModalHostEvent::Dismissed(
                enzui_core::modal::DismissCause::CloseButton,
            ))
        })
        .close_button_id(iced::advanced::widget::Id::new("files-app-dialog-close"))
        .close_label(crate::i18n::tr("Close dialog"))
        .radius(surface_radius)
        .padding(sp.space9)
        .padding_top(header_inset)
        .width(Length::Fill.max(width))
        .tokens(ModalSurfaceTokens::from_palette(&theme.palette))
        .into_element()
}

pub(crate) const ABOUT_DESCRIPTION: &str =
    "A file manager for browsing, organizing and sharing your files.";

pub(crate) fn about_page_body<'a>(_theme: &EnzTheme, _window: iced::Size) -> Element<'a, Msg> {
    let mark = iced::widget::svg(iced::widget::svg::Handle::from_memory(
        include_bytes!("../../assets/io.github.enzui.Files.svg").as_slice(),
    ))
    .width(64)
    .height(64);
    column![
        mark,
        enz_text(crate::i18n::tr("Files"))
            .size(TextSize::Title1)
            .width(Length::Fill)
            .align(enzui_widgets::text::TextAlign::Center),
        enz_text(crate::i18n::format(
            "files-version-7b947319",
            &[("arg0", env!("CARGO_PKG_VERSION").to_owned())]
        ))
        .size(TextSize::Footnote)
        .tone(TextTone::Muted)
        .width(Length::Fill)
        .align(enzui_widgets::text::TextAlign::Center),
        enz_text(crate::i18n::tr(ABOUT_DESCRIPTION))
            .tone(TextTone::Muted)
            .width(Length::Fill)
            .align(enzui_widgets::text::TextAlign::Center),
    ]
    .spacing(8)
    .padding(iced::Padding::ZERO.top(24.0))
    .align_x(iced::Alignment::Center)
    .width(Length::Fill)
    .into()
}

// The Extensions page body.
//
// A disabled feature schedules no background work: the gating reducer arms read
// `model.features.enabled(..)`.
pub(crate) fn extensions_page_body<'a>(model: &'a Model, theme: &'a EnzTheme) -> Element<'a, Msg> {
    use enzui_widgets::preferences_row::enz_preferences_row;

    // A built-in or installed extension's settings open as a subpage in place.
    if model.downloads_settings_open
        && model
            .features
            .enabled(crate::features::FeatureId::Downloads)
    {
        return column![
            super::extensions::subpage_header(
                crate::i18n::tr("Downloads").to_owned(),
                Msg::ShowDownloadsSettings(false),
            ),
            super::properties::downloads_settings_page(model, theme),
        ]
        .spacing(theme.spacing.md())
        .width(Length::Fill)
        .into();
    }
    if let Some(id) = &model.extensions.expanded
        && let Some(page) = super::extensions::settings_page(model, theme, id)
    {
        return page;
    }

    let sp = &theme.spacing;
    let features_blocked = model.features_refusal.is_some() || model.features_durability_debt;
    let mut notices = column![].spacing(sp.xs()).width(Length::Fill);
    if let Some(refusal) = &model.features_refusal {
        notices = notices.push(
            enz_text(crate::i18n::format(
                "files-label-detail",
                &[
                    (
                        "title",
                        crate::i18n::tr(crate::features::FEATURES_REFUSED_TITLE).to_owned(),
                    ),
                    ("detail", refusal.message().to_owned()),
                ],
            ))
            .size(TextSize::Caption)
            .tone(TextTone::Danger)
            .into_element(),
        );
        notices = notices.push(
            row![
                enz_button(
                    crate::i18n::tr("Retry loading settings"),
                    Msg::RetryFeatureSettings,
                )
                .kind(ButtonKind::Quiet)
                .into_element(),
                enz_button(
                    crate::i18n::tr("Replace with defaults"),
                    Msg::ReplaceFeatureSettings,
                )
                .kind(ButtonKind::Destructive)
                .into_element(),
            ]
            .mirrored(crate::i18n::rtl())
            .spacing(sp.sm()),
        );
    } else if model.features_durability_debt {
        notices = notices.push(
            enz_text(crate::i18n::tr(
                "Feature settings saved but durability is uncertain",
            ))
            .size(TextSize::Caption)
            .tone(TextTone::Danger)
            .into_element(),
        );
        notices = notices.push(
            enz_button(
                crate::i18n::tr("Retry saving settings"),
                Msg::RetryFeatureSettings,
            )
            .kind(ButtonKind::Quiet)
            .into_element(),
        );
    }

    // Built-in features are extensions too: a switch each, and a settings
    // icon for one that has settings.
    let mut features = column![notices].width(Length::Fill);
    for descriptor in crate::features::REGISTRY {
        if descriptor.id == crate::features::FeatureId::Converters {
            continue;
        }
        let enabled = model.features.enabled(descriptor.id);
        let name = crate::i18n::tr(descriptor.name);
        let mut row = enz_preferences_row(name)
            .subtitle(crate::i18n::tr(descriptor.description))
            .id_prefix(format!("files-app-feature-{}", descriptor.id.id()))
            .enabled(!features_blocked);
        // Downloads is the one built-in feature with settings of its own.
        if descriptor.id == crate::features::FeatureId::Downloads && enabled && !features_blocked {
            row = row.suffix(super::extensions::row_icon(
                "settings",
                crate::i18n::format("files-extension-settings-for", &[("name", name.to_owned())]),
                Msg::ShowDownloadsSettings(true),
            ));
        }
        features = features.push(
            row.switch(enabled, move |_| Msg::ToggleFeature(descriptor.id))
                .into_element(),
        );
    }

    enzui_widgets::preferences_group::enz_preferences_page([
        super::extensions::management(model, theme),
        enz_preferences_group(features)
            .title(crate::i18n::tr("Built-in features"))
            .into_element(),
    ])
}

// The "Sort by" radio labels, in display order.
//
// [`SORT_KEYS`] holds the matching keys at the same indices; the radio group
// selects by index.
pub(crate) const SORT_KEY_LABELS: [&str; 4] = ["Name", "Size", "Type", "Modified"];
pub(crate) const SORT_KEYS: [SortKey; 4] = [
    SortKey::Name,
    SortKey::Size,
    SortKey::Kind,
    SortKey::Modified,
];

pub(crate) const SORT_ORDER_LABELS: [&str; 2] = ["Ascending", "Descending"];

pub(crate) const VIEW_MODE_LABELS: [&str; 2] = ["List View", "Grid View"];
pub(crate) const VIEW_MODES: [ViewMode; 2] = [ViewMode::List, ViewMode::Grid];

pub(crate) const SHOW_HIDDEN_DESCRIPTION: &str =
    "Display files and folders whose name starts with a dot.";

// The "Sort folders before files" toggle's description.
pub(crate) const FOLDERS_FIRST_DESCRIPTION: &str = "Group directories at the top of every listing.";

// The list-view column toggles' section title. Name is always shown.
pub(crate) const VISIBLE_COLUMNS_LABEL: &str = "Visible columns (list view)";

pub(crate) const SINGLE_CLICK_LABEL: &str = "Single-click to open";

pub(crate) const SINGLE_CLICK_DESCRIPTION: &str =
    "Open files and folders with a single click instead of double-click.";

// No Terminal Application section: there is no terminal setting to configure.

pub(crate) const RECURSIVE_SIZE_COMPUTING: &str = "Computing\u{2026}";

// The index of the currently-active sort key in [`SORT_KEYS`].
pub(crate) fn sort_key_index(key: SortKey) -> usize {
    SORT_KEYS.iter().position(|k| *k == key).unwrap_or(0)
}

// The index of the currently-active view mode in [`VIEW_MODES`].
pub(crate) fn view_mode_index(mode: ViewMode) -> usize {
    VIEW_MODES.iter().position(|m| *m == mode).unwrap_or(0)
}

// A mount's question for the user, such as whether to trust an unknown SFTP host
// key.
//
// The body is the backend's text with the fingerprint, then advice. Each backend
// choice is a button, with "Cancel" first; dismissing declines.
pub(crate) fn mount_question<'a>(
    theme: &EnzTheme,
    question: &crate::network::MountQuestion,
    window: Size,
) -> Element<'a, Msg> {
    use crate::network::MountAnswer;
    // Fingerprints are long unbroken words; they wrap by glyph rather than
    // run past the dialog's edge.
    let body = enzui_widgets::enz_vstack(vec![
        enz_text(question.message.trim_end().to_owned())
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .into(),
        enz_text(crate::i18n::tr(MOUNT_QUESTION_ADVICE))
            .tone(TextTone::Muted)
            .into(),
    ])
    .gap(enzui_theme::SpaceToken::Sm)
    .into_element();
    // Cancel first; the backend's indices are kept whatever the on-screen order.
    let mut order: Vec<(usize, &String)> = question.choices.iter().enumerate().collect();
    order.sort_by_key(|(_, choice)| !choice.eq_ignore_ascii_case("cancel"));
    let actions = order
        .into_iter()
        .map(|(index, choice)| {
            let kind = if choice.eq_ignore_ascii_case("cancel") {
                ButtonKind::Quiet
            } else if question.warning {
                ButtonKind::Destructive
            } else {
                ButtonKind::Accent
            };
            Action::new(
                choice.clone(),
                kind,
                Msg::MountQuestionAnswered(MountAnswer::Choice(index)),
            )
        })
        .collect();
    present(
        theme,
        if question.warning {
            crate::i18n::tr("The server's key has changed")
        } else {
            crate::i18n::tr("Verify the server")
        },
        body,
        actions,
        window,
    )
}

// What to check before trusting a server not connected to before.
pub(crate) const MOUNT_QUESTION_ADVICE: &str = "Continue only if the fingerprint matches the one \
     the server's owner gave you. If it does not, someone may be impersonating the server.";

pub(crate) fn device_confirm<'a>(
    theme: &EnzTheme,
    label: &str,
    op: crate::devices::DeviceOp,
    window: Size,
) -> Element<'a, Msg> {
    use crate::devices::DeviceOp;
    let (title, body) = match op {
        DeviceOp::PowerOff => (
            crate::i18n::format(
                "files-safely-remove-label-d81c869c",
                &[("label", (label).to_owned())],
            ),
            crate::i18n::tr(
                "Every volume on this drive is unmounted and the drive is powered off, \
             so it can be unplugged. Close any files it holds first.",
            ),
        ),
        _ => (
            crate::i18n::format(
                "files-eject-label-964a108e",
                &[("label", (label).to_owned())],
            ),
            crate::i18n::tr(
                "Every volume on this drive is unmounted and the media is ejected. \
             Close any files it holds first.",
            ),
        ),
    };
    present(
        theme,
        title,
        enz_text(body).into(),
        vec![
            Action::new(
                crate::i18n::tr("Cancel"),
                ButtonKind::Quiet,
                Msg::CancelDeviceAction,
            ),
            Action::new(op.label(), ButtonKind::Accent, Msg::ConfirmDeviceAction),
        ],
        window,
    )
}
