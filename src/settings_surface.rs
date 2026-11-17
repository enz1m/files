// The View Settings surface: its `ModalPresentation<()>` retention and its
// `enz_settings_dialog` builder, hosted with `.into_host_with(background, ..)`.
//
// `model.view_settings_open` holds exactly while a generation is open. Dismissal
// moves it to closing, and the view paints once more with
// `presentation_open(false)`. `Msg::ViewSettingsExitSettled(key)` retires only
// the matching generation. A peer dialog opened during the exit retires the
// outgoing generation at once, so a late settlement cannot resurrect it.

use crate::{ActiveGenericDialog, App};

// Syncs the retained presentation with `model.view_settings_open` and any peer
// overlay.
pub(crate) fn sync(app: &mut App) {
    if app.model.view_settings_open {
        if !app.settings_presentation.is_open() {
            let _ = app.settings_presentation.present(());
        }
        return;
    }
    if app.settings_presentation.is_open() {
        let _ = app.settings_presentation.dismiss();
    }
    let peer_open = matches!(
        app.active_generic_dialog(),
        Some(kind) if !matches!(kind, ActiveGenericDialog::ViewSettings)
    );
    if peer_open && let Some(key) = app.settings_presentation.key() {
        let _ = app.settings_presentation.finish_exit(key);
    }
}

pub(crate) fn is_closing(app: &App) -> bool {
    app.settings_presentation.is_closing()
}

// The generation a settlement callback must carry to retire the currently
// retained surface, `None` when no generation is presented.
pub(crate) fn presentation_key(
    app: &App,
) -> Option<enzui_widgets::modal_host::ModalPresentationKey> {
    app.settings_presentation.key()
}

// Retires the settled generation, ignoring a stale key from before a reopen.
pub(crate) fn finish_exit(app: &mut App, key: enzui_widgets::modal_host::ModalPresentationKey) {
    let _ = app.settings_presentation.finish_exit(key);
}

use enzui_theme::EnzTheme;
use enzui_widgets::enz_text::{TextSize, TextTone, enz_text};
use enzui_widgets::modal_host::{ModalHostEvent, ModalPresentationKey};
use enzui_widgets::preferences_group::enz_preferences_group;
use enzui_widgets::settings_dialog::{
    DialogSize, EnzSettingsPresentation, SettingsDialogEvent, SettingsPage, enz_settings_dialog,
};
use enzui_widgets::text_field::{TextFieldSize, enz_text_field};
use iced::widget::column;
use iced::{Element, Length};

use crate::dialog::{
    FOLDERS_FIRST_DESCRIPTION, SHOW_HIDDEN_DESCRIPTION, SINGLE_CLICK_DESCRIPTION,
    SINGLE_CLICK_LABEL, SORT_KEY_LABELS, SORT_KEYS, SORT_ORDER_LABELS, VIEW_MODE_LABELS,
    VIEW_MODES, VIEW_SETTINGS_SCOPE, VISIBLE_COLUMNS_LABEL, about_page_body, extensions_page_body,
    sort_key_index, view_mode_index,
};
use crate::message::Msg;
use crate::model::{Model, SettingsSection};

pub(crate) const TERMINAL_SECTION_LABEL: &str = "Terminal";
pub(crate) const TERMINAL_COMMAND_LABEL: &str = "Terminal command";
pub(crate) const TERMINAL_COMMAND_PLACEHOLDER: &str = "Automatic";
pub(crate) const TERMINAL_COMMAND_DESCRIPTION: &str = "Used by Open in Terminal. A program name or \
     absolute path, then any options separated by spaces; the folder is opened as the \
     terminal's working directory. Leave empty to use the first installed of \
     xdg-terminal-exec, Console, GNOME Terminal, Konsole and others.";

pub(crate) fn settings_menu_id(which: crate::model::SettingsMenu) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(match which {
        crate::model::SettingsMenu::SortBy => "files-app-view-settings-sort-by",
        crate::model::SettingsMenu::SortOrder => "files-app-view-settings-sort-order",
        crate::model::SettingsMenu::View => "files-app-view-settings-view",
    })
}

pub(crate) fn view_settings<'a>(
    theme: &'a EnzTheme,
    model: &'a Model,
    shortcuts: &'a crate::keybindings::FilesShortcuts,
    background: Element<'a, Msg>,
    window: iced::Size,
    // False for the frames after dismissal while the exit motion plays; the
    // surface is withdrawn from input, focus and AT from the first one.
    showing: bool,
    // The generation reported back by `Msg::ViewSettingsExitSettled`.
    settled_key: Option<ModalPresentationKey>,
) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    use crate::model::SettingsMenu;
    use enzui_widgets::preferences_row::enz_preferences_row;

    // A value menu per choice: the current value on the right, the choices
    // behind it, as every desktop's preferences lay them out.
    let menu = |title: &'static str,
                which: SettingsMenu,
                current: &'static str,
                options: Vec<(String, Msg)>|
     -> Element<'a, Msg> {
        enz_preferences_row(title)
            .id(settings_menu_id(which))
            .value_menu(
                crate::i18n::tr(current),
                model.settings_menu == Some(which),
                Msg::OpenSettingsMenu(which),
                Msg::CloseSettingsMenus,
                options
                    .into_iter()
                    .map(|(label, message)| (crate::i18n::text(&label), message))
                    .collect(),
            )
            .into_element()
    };
    // Sets the key without toggling order, from the default rather than the
    // folder's override.
    let sort_by = menu(
        crate::i18n::tr("Sort by"),
        SettingsMenu::SortBy,
        SORT_KEY_LABELS[sort_key_index(model.default_sort_key)],
        SORT_KEY_LABELS
            .iter()
            .zip(SORT_KEYS)
            .map(|(label, key)| ((*label).to_owned(), Msg::SetSortKey(key)))
            .collect(),
    );
    let sort_order = menu(
        crate::i18n::tr("Sort order"),
        SettingsMenu::SortOrder,
        SORT_ORDER_LABELS[if model.default_sort_ascending { 0 } else { 1 }],
        SORT_ORDER_LABELS
            .iter()
            .enumerate()
            .map(|(i, label)| ((*label).to_owned(), Msg::SetSortAscending(i == 0)))
            .collect(),
    );
    let view_mode = menu(
        crate::i18n::tr("View"),
        SettingsMenu::View,
        VIEW_MODE_LABELS[view_mode_index(model.default_view_mode)],
        VIEW_MODE_LABELS
            .iter()
            .zip(VIEW_MODES)
            .map(|(label, mode)| ((*label).to_owned(), Msg::SetViewMode(mode)))
            .collect(),
    );

    // A setting that is on or off reads as its name and description, with
    // the switch at the row's end.
    let switch = |title: &'a str,
                  description: &'a str,
                  on: bool,
                  message: Msg,
                  prefix: &str|
     -> Element<'a, Msg> {
        let mut row = enz_preferences_row(title)
            .switch(on, move |_| message.clone())
            .id(iced::advanced::widget::Id::from(prefix.to_owned()));
        if !description.is_empty() {
            row = row.subtitle(description).subtitle_wrap(true);
        }
        row.into_element()
    };
    let controls: Element<'a, Msg> = column![
        sort_by,
        sort_order,
        view_mode,
        switch(
            crate::i18n::tr("Sort folders before files"),
            crate::i18n::tr(FOLDERS_FIRST_DESCRIPTION),
            model.sort_folders_first,
            Msg::ToggleFoldersFirst,
            "files-app-view-settings-folders-first",
        ),
        switch(
            crate::i18n::tr("Show hidden files"),
            crate::i18n::tr(SHOW_HIDDEN_DESCRIPTION),
            model.show_hidden,
            Msg::ToggleHidden,
            "files-app-view-settings-hidden",
        ),
        switch(
            crate::i18n::tr("Color file-type icons"),
            crate::i18n::tr("Use familiar colors to make file formats easier to scan."),
            model.colored_file_icons,
            Msg::ToggleColoredFileIcons,
            "files-app-view-settings-colored-icons",
        ),
    ]
    .width(Length::Fill)
    .into();

    // No heading: the preset's pinned header owns the only title.
    let group = enz_preferences_group(controls).into_element();

    // Name is always shown, so only Size, Modified and Type are offered.
    let mut column_rows = column![].width(Length::Fill);
    for meta in crate::model::MetaColumn::ALL {
        column_rows = column_rows.push(switch(
            meta.label(),
            "",
            model.column_visible(meta),
            Msg::ToggleColumn(meta),
            match meta {
                crate::model::MetaColumn::Size => "files-app-view-settings-col-size",
                crate::model::MetaColumn::Modified => "files-app-view-settings-col-modified",
                crate::model::MetaColumn::Kind => "files-app-view-settings-col-type",
            },
        ));
    }
    let visible_cols: Element<'a, Msg> = enz_preferences_group(column_rows)
        .title(crate::i18n::tr(VISIBLE_COLUMNS_LABEL))
        .into_element();

    let mut grid_rows = column![].width(Length::Fill);
    for meta in crate::model::MetaColumn::ALL {
        grid_rows = grid_rows.push(switch(
            meta.label(),
            "",
            model.grid_info_visible(meta),
            Msg::ToggleGridInfo(meta),
            &format!("files-app-grid-info-{}", meta.index()),
        ));
    }
    let grid_info: Element<'a, Msg> = enz_preferences_group(grid_rows)
        .title(crate::i18n::tr("Grid item information"))
        .description(crate::i18n::tr(
            "Shown below file and folder names in Grid view.",
        ))
        .into_element();

    // Saved as typed; the folder is the working directory, never an argument.
    // Inset like a row's label, so the field lines up with the rows above.
    let row_inset = enzui_widgets::preferences_row::PreferencesRowTokens::default().padding_x;
    let terminal: Element<'a, Msg> = enz_preferences_group(
        iced::widget::container(
            enz_text_field(
                crate::i18n::tr(TERMINAL_COMMAND_LABEL),
                crate::i18n::tr(TERMINAL_COMMAND_PLACEHOLDER),
                &model.terminal_command,
                Msg::SetTerminalCommand,
            )
            .description(crate::i18n::tr(TERMINAL_COMMAND_DESCRIPTION))
            .size(TextFieldSize::Md)
            .id_prefix("files-app-view-settings-terminal")
            .into_element(),
        )
        .padding(iced::Padding::from([0.0, row_inset])),
    )
    .title(crate::i18n::tr(TERMINAL_SECTION_LABEL))
    .into_element();

    // Extensions is its own page, built by [`extensions_page_body`].
    // Groups are separated by dividers, as on every settings page.
    let body: Element<'a, Msg> =
        enzui_widgets::preferences_group::enz_preferences_page([group, visible_cols, grid_info]);

    // How Files behaves, apart from how listings look.
    let opening: Element<'a, Msg> = enz_preferences_group(
        column![switch(
            crate::i18n::tr(SINGLE_CLICK_LABEL),
            crate::i18n::tr(SINGLE_CLICK_DESCRIPTION),
            model.single_click_open,
            Msg::ToggleSingleClick,
            "files-app-view-settings-single-click",
        )]
        .width(Length::Fill),
    )
    .into_element();
    let general_body: Element<'a, Msg> =
        enzui_widgets::preferences_group::enz_preferences_page([opening, terminal]);

    // Downloads is a built-in extension: its settings open from the
    // Extensions page rather than as a page of their own.
    let extensions_body = extensions_page_body(model, theme);

    // Action names lead and chords trail; clicking a chord records a new one.
    let mut shortcuts_body = column![].spacing(sp.sm()).width(Length::Fill);
    if let Some(notice) = &model.shortcuts_notice {
        shortcuts_body = shortcuts_body.push(
            enz_text(notice.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Danger)
                .into_element(),
        );
    }
    let shortcuts_body = shortcuts_body.push(
        enzui_widgets::editable_shortcuts::enz_editable_shortcuts(
            shortcuts,
            model.shortcut_capture.as_deref(),
            Msg::Shortcut,
        )
        .feedback(
            model
                .shortcut_feedback
                .as_ref()
                .map(|(action, reason)| (action.as_str(), reason.as_str())),
        )
        .into_element(),
    );

    let about_body = about_page_body(theme, window);

    // `into_surface` ignores adaptive presentation; the hosted path consumes it.
    let settings = enz_settings_dialog::<Msg>(settled_key.is_some(), window)
        .title(crate::i18n::tr("Settings"))
        .size(DialogSize::Compact)
        .page(
            SettingsPage::new(SettingsSection::GENERAL_ID, crate::i18n::tr("General")),
            general_body,
        )
        .page(
            SettingsPage::new(SettingsSection::VIEW_SETTINGS_ID, crate::i18n::tr("View")),
            body,
        )
        .page(
            SettingsPage::new(
                SettingsSection::EXTENSIONS_ID,
                crate::i18n::tr("Extensions"),
            ),
            extensions_body,
        );
    let mut settings = settings
        .page(
            SettingsPage::new(SettingsSection::SHORTCUTS_ID, crate::i18n::tr("Shortcuts")),
            shortcuts_body,
        )
        .page(
            SettingsPage::new(SettingsSection::ABOUT_ID, crate::i18n::tr("About")),
            about_body,
        )
        .selected(model.settings_page.id())
        .on_select(Msg::SelectSettingsPage)
        .stacked_navigation(model.settings_page_open, Msg::SettingsStackedBack)
        .page_id_prefix("files-app-view-settings")
        .scope_key(VIEW_SETTINGS_SCOPE)
        .presentation(EnzSettingsPresentation::Adaptive)
        // Keeps the outgoing pixels while withdrawn from input, focus and AT;
        // `.on_exit_settled(..)` retires exactly the painted generation.
        .presentation_open(showing);
    // Resetting every shortcut sits in the footer while the Shortcuts page is
    // shown, so it stays reachable as the rows scroll.
    let shortcuts_shown = model.settings_page == SettingsSection::Shortcuts
        && (model.settings_page_open
            || settings.resolved_nav() != enzui_widgets::settings_dialog::SettingsNav::Stacked);
    if shortcuts_shown {
        // A quiet footer command: Files draws no raised neutral buttons.
        settings = settings.action(
            enzui_widgets::editable_shortcuts::editable_shortcuts_reset_all_action(|| {
                Msg::Shortcut(enzui_widgets::editable_shortcuts::EditableShortcutsEvent::ResetAll)
            })
            .kind(enzui_widgets::button::ButtonKind::Quiet),
        );
    }
    if let Some(key) = settled_key {
        settings = settings.on_exit_settled(move || Msg::ViewSettingsExitSettled(key));
    }
    settings.into_host_with(background, |event| {
        Msg::DialogEvent(match event {
            SettingsDialogEvent::Dismissed(cause) => ModalHostEvent::Dismissed(cause),
            SettingsDialogEvent::FocusNext => ModalHostEvent::FocusNext,
            SettingsDialogEvent::FocusPrevious => ModalHostEvent::FocusPrevious,
        })
    })
}
