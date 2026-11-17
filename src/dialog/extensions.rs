use super::*;
use crate::extensions::{Change, SettingKind, Worker};
use enzui_services::extension_package::{DraftValue, SettingsDraft};
use enzui_widgets::preferences_row::enz_preferences_row;
use enzui_widgets::text_field::enz_text_field_owned;
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use iced::widget::column;

// A round icon action for a row's suffix slot, named for assistive technology.
pub(super) fn row_icon<'a>(icon: &'a str, label: String, on_press: Msg) -> Element<'a, Msg> {
    enz_toolbar_icon(icon, on_press)
        .aria_label(label.clone())
        .tooltip(label)
        .into_element()
}

// A subpage's header: Back, then the page's title.
pub(super) fn subpage_header<'a>(title: String, back: Msg) -> Element<'a, Msg> {
    let back_label = crate::i18n::tr("Back").to_owned();
    row![
        row_icon("chevron-left", back_label, back),
        enz_text(title).size(TextSize::Headline).into_element(),
    ]
    .mirrored(crate::i18n::rtl())
    .spacing(8.0)
    .align_y(iced::Alignment::Center)
    .into()
}

// The installed extensions, the install and restore actions, and any pending
// package review or removal confirmation.
pub(super) fn management<'a>(model: &'a Model, theme: &'a EnzTheme) -> Element<'a, Msg> {
    let catalog = &model.extensions;
    let enabled = !catalog.busy && catalog.root.is_some();

    // Each extension reads like a built-in setting: its name and origin, its
    // settings and removal as icons, and its switch at the trailing edge.
    let mut rows = column![].width(Length::Fill);
    for entry in &catalog.entries {
        let id = entry.id.clone();
        let mut subtitle = entry.manifest.as_ref().map_or_else(String::new, |m| {
            let origin = crate::i18n::format(
                "files-extension-origin",
                &[
                    ("version", m.version.clone()),
                    ("author", m.author.clone()),
                    (
                        "origin",
                        crate::i18n::tr(if entry.bundled {
                            "Bundled"
                        } else {
                            "Installed"
                        })
                        .to_owned(),
                    ),
                ],
            );
            format!("{origin}\n{}", entry.display_text(&m.description))
        });
        if let Some(error) = &entry.problem {
            subtitle = format!("{subtitle}\n{error}");
        }
        let name = entry.display_name();
        let settings_label =
            crate::i18n::format("files-extension-settings-for", &[("name", name.clone())]);
        let remove_label =
            crate::i18n::format("files-extension-remove-name", &[("name", name.clone())]);
        let mut row = enz_preferences_row(name)
            .subtitle(subtitle)
            .id_prefix(format!("files-extension-{}", entry.id))
            .enabled(enabled && entry.problem.is_none());
        if !catalog.busy {
            row = row.suffix(row_icon(
                "settings",
                settings_label,
                Msg::ExtensionDetails(entry.id.clone()),
            ));
        }
        if enabled {
            row = row.suffix(row_icon(
                "trash-2",
                remove_label,
                Msg::AskRemoveExtension(entry.id.clone()),
            ));
        }
        rows = rows.push(
            row.switch(entry.active(), move |value| {
                Msg::ChangeExtension(Change::Enable(id.clone(), value))
            })
            .into_element(),
        );
    }
    rows = rows.push(
        enz_preferences_row(crate::i18n::tr("Install from file…"))
            .subtitle(crate::i18n::tr(
                "Install a .enzfiles package, or double-click it in Files.",
            ))
            .enabled(enabled)
            .action("", Msg::BeginInstallExtension)
            .into_element(),
    );
    rows = rows.push(
        enz_preferences_row(crate::i18n::tr("Restore bundled extensions"))
            .enabled(enabled)
            .action("", Msg::ChangeExtension(Change::Restore))
            .into_element(),
    );
    if catalog.running {
        rows = rows.push(
            enz_preferences_row(crate::i18n::tr("Cancel action"))
                .action("", Msg::CancelExtensionAction)
                .into_element(),
        );
    }

    let mut body = column![
        enz_preferences_group(rows)
            .title(crate::i18n::tr("Extensions"))
            .into_element()
    ]
    .spacing(theme.spacing.md())
    .width(Length::Fill);
    if let Some(error) = &catalog.error {
        body = body.push(enz_text(error).tone(TextTone::Danger).into_element());
    }
    if let Some(review) = package_review(model, theme, enabled) {
        body = body.push(review);
    }
    if let Some(id) = &catalog.remove {
        let name = catalog
            .entries
            .iter()
            .find(|e| e.id == *id)
            .map_or(id.as_str(), |e| e.name());
        body = body.push(
            enz_preferences_group(
                column![
                    enz_text(crate::i18n::format(
                        "files-remove-name-its-saved-settings-will-ed33921c",
                        &[("name", name.to_owned())]
                    ))
                    .size(TextSize::Caption)
                    .into_element(),
                    row![
                        enz_button(
                            crate::i18n::tr("Remove extension"),
                            Msg::ConfirmRemoveExtension
                        )
                        .kind(ButtonKind::Destructive)
                        .enabled(enabled)
                        .into_element(),
                        enz_button(crate::i18n::tr("Cancel"), Msg::CancelExtensionReview)
                            .kind(ButtonKind::Quiet)
                            .enabled(enabled)
                            .into_element()
                    ]
                    .mirrored(crate::i18n::rtl())
                    .spacing(theme.spacing.sm())
                ]
                .spacing(theme.spacing.sm()),
            )
            .title(crate::i18n::tr("Remove extension"))
            .into_element(),
        );
    }
    body.into()
}

// The review of a package chosen for install or update.
fn package_review<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    enabled: bool,
) -> Option<Element<'a, Msg>> {
    let catalog = &model.extensions;
    let package = catalog.pending.as_ref()?;
    let m = &package.manifest;
    let update = catalog.entries.iter().find(|e| e.id == m.id);
    let label = if update.is_some() {
        crate::i18n::tr("Update extension")
    } else {
        crate::i18n::tr("Install extension")
    };
    let mut review = column![
        enz_text(crate::i18n::format(
            "files-review-408b5f87",
            &[("arg0", m.name.clone())]
        ))
        .size(TextSize::Body)
        .into_element(),
        enz_text(crate::i18n::format(
            "files-extension-author-version",
            &[("author", m.author.clone()), ("version", m.version.clone())]
        ))
        .size(TextSize::Caption)
        .into_element(),
        enz_text(&m.description)
            .size(TextSize::Caption)
            .into_element()
    ]
    .spacing(theme.spacing.sm())
    .width(Length::Fill);
    if let Some(old) = update.and_then(|e| e.manifest.as_ref()) {
        review = review.push(
            enz_text(crate::i18n::format(
                "files-replaces-version-by-the-update-starts-572a249c",
                &[
                    ("arg0", (old.version).clone()),
                    ("arg1", (old.author).clone()),
                ],
            ))
            .size(TextSize::Caption)
            .into_element(),
        );
    }
    if matches!(m.worker, Worker::Native { .. }) {
        review = review.push(enz_text(crate::i18n::tr("Native extensions run with your account’s permissions. Install only extensions from authors you trust.")).size(TextSize::Caption).into_element());
    }
    if !m.permissions.is_empty() {
        review = review.push(
            enz_text(crate::i18n::format(
                "files-host-access-5b6cba8d",
                &[(
                    "arg0",
                    (m.permissions
                        .iter()
                        .map(|p| match p {
                            crate::extensions::Permission::Clipboard => {
                                crate::i18n::tr("copy text to clipboard")
                            }
                            crate::extensions::Permission::Navigate => {
                                crate::i18n::tr("open local folders")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")),
                )],
            ))
            .size(TextSize::Caption)
            .into_element(),
        );
    }
    if let Some(reason) = m.compatibility() {
        review = review.push(
            enz_text(reason)
                .size(TextSize::Caption)
                .tone(TextTone::Danger)
                .into_element(),
        );
    }
    review = review.push(
        row![
            enz_button(label, Msg::ConfirmInstallExtension)
                .kind(ButtonKind::Accent)
                .enabled(enabled)
                .into_element(),
            enz_button(crate::i18n::tr("Cancel"), Msg::CancelExtensionReview)
                .kind(ButtonKind::Quiet)
                .enabled(!catalog.busy)
                .into_element()
        ]
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.sm()),
    );
    Some(
        enz_preferences_group(review)
            .title(crate::i18n::tr("Package review"))
            .into_element(),
    )
}

// One extension's subpage: what it is, and its settings with Save.
pub(super) fn settings_page<'a>(
    model: &'a Model,
    theme: &'a EnzTheme,
    entry_id: &str,
) -> Option<Element<'a, Msg>> {
    let catalog = &model.extensions;
    let entry = catalog.entries.iter().find(|e| e.id == entry_id)?;
    let enabled = !catalog.busy && catalog.root.is_some();
    let mut content = column![subpage_header(
        entry.display_name(),
        Msg::ExtensionDetails(entry.id.clone()),
    )]
    .spacing(theme.spacing.md())
    .width(Length::Fill);
    let Some(m) = &entry.manifest else {
        if let Some(error) = &entry.problem {
            content = content.push(
                enz_text(error)
                    .size(TextSize::Caption)
                    .tone(TextTone::Danger)
                    .into_element(),
            );
        }
        return Some(content.into());
    };
    let mut about = column![
        enz_text(entry.display_text(&m.description))
            .size(TextSize::Callout)
            .into_element(),
        enz_text(crate::i18n::format(
            "files-api-actions-74ad64d9",
            &[
                ("arg0", (m.id).clone()),
                ("arg1", (m.api).to_string()),
                ("arg2", (m.actions.len()).to_string()),
            ],
        ))
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element(),
    ]
    .spacing(theme.spacing.xs());
    if matches!(m.worker, Worker::Native { .. }) {
        about = about.push(
            enz_text(crate::i18n::tr(
                "This native worker runs with your account’s permissions.",
            ))
            .size(TextSize::Caption)
            .tone(TextTone::Muted)
            .into_element(),
        );
    }
    content = content.push(about);

    if m.settings.is_empty() {
        content = content.push(
            enz_text(crate::i18n::tr("This extension has no settings."))
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .into_element(),
        );
        return Some(content.into());
    }
    let applied = SettingsDraft::new(&m.settings, &entry.settings());
    let draft = catalog.drafts.get(&entry.id).unwrap_or(&applied);
    let mut fields = column![].spacing(theme.spacing.sm()).width(Length::Fill);
    for (setting, field) in m.settings.iter().zip(draft.fields(&m.settings)) {
        let extension = entry.id.clone();
        let key = setting.id.clone();
        match (&setting.value, field.value) {
            (SettingKind::Toggle { .. }, DraftValue::Toggle(value)) => {
                fields = fields.push(
                    enz_preferences_row(setting.label.clone())
                        .id_prefix(format!("extension-{}-{}", entry.id, setting.id))
                        .enabled(enabled)
                        .switch(*value, move |value| Msg::ExtensionSettingEdited {
                            extension: extension.clone(),
                            key: key.clone(),
                            value: DraftValue::Toggle(value),
                        })
                        .into_element(),
                );
            }
            (SettingKind::Text { .. } | SettingKind::Integer { .. }, DraftValue::Text(text)) => {
                let mut input =
                    enz_text_field_owned(&setting.label, "", text.clone(), move |text| {
                        Msg::ExtensionSettingEdited {
                            extension: extension.clone(),
                            key: key.clone(),
                            value: DraftValue::Text(text),
                        }
                    })
                    .id_prefix(format!("extension-{}-{}", entry.id, setting.id))
                    .enabled(enabled);
                if let Some(problem) = field.problem {
                    input = input.error(problem.to_string());
                }
                fields = fields.push(input.into_element());
            }
            (SettingKind::Choice { choices, .. }, DraftValue::Choice(selected)) => {
                fields = fields.push(
                    enz_combo_box(
                        &setting.label,
                        choices.clone(),
                        Some(selected.clone()),
                        move |value: String| Msg::ExtensionSettingEdited {
                            extension: extension.clone(),
                            key: key.clone(),
                            value: DraftValue::Choice(value),
                        },
                    )
                    .enabled(enabled)
                    .into_element(),
                );
            }
            _ => {}
        }
    }
    content = content.push(
        enz_preferences_group(fields)
            .title(crate::i18n::tr("Settings"))
            .into_element(),
    );
    content = content.push(
        row![
            Space::new().width(Length::Fill),
            enz_button(
                crate::i18n::tr("Save settings"),
                Msg::SaveExtensionSettings(entry.id.clone()),
            )
            .kind(ButtonKind::Accent)
            .enabled(enabled && draft.is_changed(&m.settings) && draft.apply(&m.settings).is_ok())
            .into_element(),
        ]
        .mirrored(crate::i18n::rtl()),
    );
    Some(content.into())
}
