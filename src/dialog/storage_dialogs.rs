// Dialogs of the enzimOS mount service: Connect to Server and its sign-in,
// Format, Add Cloud Drive, unlocking an encrypted drive, the share picker and
// the confirmations around saved locations.
//
// They follow the framework's dialog defaults: the default action is an
// Accent button, Cancel and Close are quiet, and anything that erases or
// removes is Destructive. Each names what it acts on in a header row (icon,
// name, what it is), so the copy below can stay short.

use super::*;
use crate::mount_service::{
    CloudProvider, CloudState, Confirmation, FileSystem, FormatPhase, FormatState, LocationDialog,
};
use enzui_widgets::banner::{BannerTone, enz_banner};
use enzui_widgets::icon::{IconTone, enz_icon};
use enzui_widgets::radio_button::enz_radio_button;
use enzui_widgets::radio_group::RadioGroupSize;
use iced::widget::{column, container};

// Cancel or Close: quiet, as the framework's confirm and form dialogs draw it.
fn quiet(label: &str, message: Msg) -> Action {
    Action::new(label, ButtonKind::Quiet, message)
}

fn accent(label: &str, message: Msg) -> Action {
    Action::new(label, ButtonKind::Accent, message)
}

// What a dialog acts on: an icon tile, the name, and what it is.
pub(crate) fn subject<'a>(
    theme: &EnzTheme,
    icon: &str,
    title: String,
    detail: Option<String>,
) -> Element<'a, Msg> {
    let tile = enz_surface(
        container(
            enz_icon(icon)
                .size(theme.sizing.icon_md)
                .tint_theme_tone(IconTone::Muted)
                .into_element(),
        )
        .center(Length::Fixed(40.0)),
    )
    .tone(SurfaceTone::SurfaceAlt)
    .radius(10.0)
    .into_element();
    let mut text = column![enz_text(title).size(TextSize::Headline).into_element()]
        .spacing(2)
        .width(Length::Fill);
    if let Some(detail) = detail {
        text = text.push(
            enz_text(detail)
                .size(TextSize::Footnote)
                .tone(TextTone::Muted)
                .into_element(),
        );
    }
    row![tile, text]
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.md())
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

// A state line: a spinner or icon beside a title and an explanation.
pub(crate) fn status<'a>(
    theme: &EnzTheme,
    leading: Element<'a, Msg>,
    title: String,
    detail: String,
) -> Element<'a, Msg> {
    let text = column![
        enz_text(title).size(TextSize::Headline).into_element(),
        enz_text(detail).tone(TextTone::Muted).into_element(),
    ]
    .spacing(theme.spacing.xs())
    .width(Length::Fill);
    row![container(leading).center_x(Length::Fixed(40.0)), text]
        .mirrored(crate::i18n::rtl())
        .spacing(theme.spacing.md())
        .align_y(Alignment::Start)
        .width(Length::Fill)
        .into()
}

fn spinner<'a>() -> Element<'a, Msg> {
    enz_spinner().size(24.0).into_element()
}

fn icon<'a>(name: &str, tone: IconTone) -> Element<'a, Msg> {
    enz_icon(name)
        .size(24.0)
        .tint_theme_tone(tone)
        .into_element()
}

fn danger<'a>(title: &str, body: String) -> Element<'a, Msg> {
    enz_banner(title)
        .body(body)
        .tone(BannerTone::Danger)
        .width(Length::Fill)
        .into_element()
}

fn stack<'a>(theme: &EnzTheme, items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    iced::widget::Column::with_children(items)
        .spacing(theme.spacing.lg())
        .width(Length::Fill)
        .into()
}

// ---------------------------------------------------------------------------
// Format

pub(crate) const FORMAT_TITLE: &str = "Format Drive";

// The Format dialog: what will be erased, the new name and file system, then
// progress and the outcome. Formatting starts only from the red button; Enter
// in the name field does nothing destructive.
pub(crate) fn format<'a>(
    theme: &EnzTheme,
    state: &'a FormatState,
    window: Size,
) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    let header = || subject(theme, "usb", state.name.clone(), state.summary());
    let (body, actions): (Element<'a, Msg>, Vec<Action>) = match &state.phase {
        FormatPhase::Choosing { error } => {
            let problem = state.label_problem();
            let mut items = vec![header()];
            if let Some(error) = error {
                items.push(danger(
                    crate::i18n::tr("The drive was not formatted"),
                    error.clone(),
                ));
            }
            items.push(
                enz_banner(if state.whole_drive {
                    crate::i18n::tr("Formatting erases every partition on this drive")
                } else {
                    crate::i18n::tr("Formatting erases everything on this drive")
                })
                .body(crate::i18n::tr(
                    "Copy anything you want to keep to another place first.",
                ))
                .tone(BannerTone::Warning)
                .width(Length::Fill)
                .into_element(),
            );
            let mut name = enz_text_field(
                crate::i18n::tr("Name"),
                crate::i18n::tr("Untitled"),
                &state.label,
                Msg::FormatLabelChanged,
            )
            .description(state.file_system.name_hint())
            .size(TextFieldSize::Md)
            .id_prefix("files-app-format-label");
            if let Some(problem) = problem {
                name = name.error(problem);
            }
            items.push(name.into_element());

            // Each choice says in plain words what it is for.
            let indent = RadioGroupSize::Md.ring_size() + sp.sm();
            let mut systems = column![
                enz_text(crate::i18n::tr("File system"))
                    .size(TextSize::Subheadline)
                    .tone(TextTone::Muted)
                    .into_element()
            ]
            .spacing(sp.sm())
            .width(Length::Fill);
            for file_system in FileSystem::ALL {
                systems = systems.push(
                    column![
                        enz_radio_button(
                            file_system.name(),
                            file_system,
                            Some(state.file_system),
                            Msg::FormatFileSystemChosen,
                        )
                        .tokens(RadioTokens::from_palette(&theme.palette))
                        .id(iced::advanced::widget::Id::from(format!(
                            "files-app-format-{}",
                            file_system.key()
                        )))
                        .into_element(),
                        container(
                            enz_text(file_system.description())
                                .size(TextSize::Footnote)
                                .tone(TextTone::Muted)
                                .into_element()
                        )
                        .padding(if crate::i18n::rtl() {
                            iced::Padding::ZERO.right(indent)
                        } else {
                            iced::Padding::ZERO.left(indent)
                        }),
                    ]
                    .spacing(2),
                );
            }
            items.push(systems.into());
            // A switch, so it does not read as a fifth file system choice.
            items.push(
                enz_preferences_group(
                    enz_preferences_row(crate::i18n::tr("Erase the whole drive"))
                        .subtitle(crate::i18n::tr(
                            "Replaces every partition with one new volume. Use this for a \
                             stick that holds an installer image.",
                        ))
                        .subtitle_wrap(true)
                        .switch(state.whole_drive, Msg::SetFormatWholeDrive)
                        .into_element(),
                )
                .bordered(true)
                .into_element(),
            );
            (
                stack(theme, items),
                vec![
                    quiet(crate::i18n::tr("Cancel"), Msg::CloseFormat),
                    Action::new(
                        crate::i18n::tr("Format"),
                        ButtonKind::Destructive,
                        Msg::ConfirmFormat,
                    )
                    .enabled(problem.is_none()),
                ],
            )
        }
        FormatPhase::Formatting => (
            stack(
                theme,
                vec![
                    header(),
                    status(
                        theme,
                        spinner(),
                        crate::i18n::format(
                            "files-formatting-as-file-system",
                            &[("file_system", state.file_system.name().to_owned())],
                        ),
                        crate::i18n::tr(
                            "Keep the drive plugged in. You can close this window; Files \
                             tells you when it is done.",
                        )
                        .to_owned(),
                    ),
                ],
            ),
            vec![quiet(crate::i18n::tr("Close"), Msg::CloseFormat)],
        ),
        FormatPhase::Done { .. } => {
            let label = state.request().label;
            let name = if label.is_empty() {
                state.name.clone()
            } else {
                label
            };
            (
                status(
                    theme,
                    icon("circle-check", IconTone::Success),
                    crate::i18n::format(
                        "files-name-is-formatted-and-ready-to-use-6f2a8d14",
                        &[("name", name)],
                    ),
                    crate::i18n::format(
                        "files-it-uses-file-system-now-c19e4b70",
                        &[("file_system", state.file_system.name().to_owned())],
                    ),
                ),
                vec![
                    quiet(crate::i18n::tr("Close"), Msg::CloseFormat),
                    accent(crate::i18n::tr("Open"), Msg::OpenFormatted),
                ],
            )
        }
    };
    present(theme, crate::i18n::tr(FORMAT_TITLE), body, actions, window)
}

// ---------------------------------------------------------------------------
// Add Cloud Drive

pub(crate) const CLOUD_TITLE: &str = "Add Cloud Drive";

// Pick a service, give a Nextcloud address, then follow the browser sign-in.
pub(crate) fn cloud<'a>(
    theme: &EnzTheme,
    state: &'a CloudState,
    providers: &'a [CloudProvider],
    window: Size,
) -> Element<'a, Msg> {
    let close = |label: &str| quiet(label, Msg::CloseCloud);
    let (body, actions): (Element<'a, Msg>, Vec<Action>) = match state {
        CloudState::Choose { error } => {
            let mut items = vec![
                enz_text(crate::i18n::tr(
                    "You sign in to the service in your web browser. Files never sees your \
                     password.",
                ))
                .tone(TextTone::Muted)
                .into_element(),
            ];
            if let Some(error) = error {
                items.push(danger(
                    crate::i18n::tr("The cloud drive could not be added"),
                    error.clone(),
                ));
            }
            if providers.is_empty() {
                items.push(
                    enz_text(crate::i18n::tr(
                        "No cloud services can be added on this computer.",
                    ))
                    .tone(TextTone::Muted)
                    .into_element(),
                );
            } else {
                // The whole row is the choice; a chevron says it goes on.
                let rows: Vec<Element<'a, Msg>> = providers
                    .iter()
                    .map(|&provider| {
                        enz_preferences_row(provider.name())
                            .icon(provider.icon())
                            .subtitle(provider.description())
                            .subtitle_wrap(true)
                            .action("", Msg::CloudProviderChosen(provider))
                            .id_prefix(format!("files-app-cloud-{}", provider.key()))
                            .into_element()
                    })
                    .collect();
                items.push(
                    enz_preferences_group(
                        iced::widget::Column::with_children(rows).width(Length::Fill),
                    )
                    .bordered(true)
                    .into_element(),
                );
            }
            (stack(theme, items), vec![close(crate::i18n::tr("Cancel"))])
        }
        CloudState::Server { server, error } => {
            let mut field = enz_text_field(
                crate::i18n::tr("Server address"),
                "cloud.example.com",
                server,
                Msg::CloudServerChanged,
            )
            .description(crate::i18n::tr(
                "The address you open your Nextcloud with in a browser.",
            ))
            .size(TextFieldSize::Md)
            .on_submit(Msg::ConfirmCloudServer)
            .id_prefix("files-app-cloud-server");
            if let Some(error) = error {
                field = field.error(error.as_str());
            }
            (
                stack(
                    theme,
                    vec![
                        subject(
                            theme,
                            CloudProvider::Nextcloud.icon(),
                            CloudProvider::Nextcloud.name().to_owned(),
                            Some(CloudProvider::Nextcloud.description().to_owned()),
                        ),
                        field.into_element(),
                    ],
                ),
                vec![
                    quiet(crate::i18n::tr("Back"), Msg::CloudBack),
                    accent(crate::i18n::tr("Continue"), Msg::ConfirmCloudServer)
                        .enabled(!server.trim().is_empty()),
                ],
            )
        }
        CloudState::Starting { provider, .. } => (
            status(
                theme,
                spinner(),
                crate::i18n::tr("Opening your browser…").to_owned(),
                crate::i18n::format(
                    "files-starting-the-sign-in-to-service-2e8d5a61",
                    &[("service", provider.name().to_owned())],
                ),
            ),
            vec![close(crate::i18n::tr("Close"))],
        ),
        CloudState::SigningIn { service, .. } => (
            status(
                theme,
                spinner(),
                crate::i18n::tr("Continue in your browser").to_owned(),
                crate::i18n::format(
                    "files-sign-in-to-service-in-the-browser-7a3c1f08",
                    &[("service", service.clone())],
                ),
            ),
            vec![
                Action::new(
                    crate::i18n::tr("Cancel Sign-in"),
                    ButtonKind::QuietDestructive,
                    Msg::CancelCloudSignIn,
                ),
                Action::new(crate::i18n::tr("Close"), ButtonKind::Quiet, Msg::CloseCloud),
            ],
        ),
        CloudState::Done { name, .. } => (
            status(
                theme,
                icon("circle-check", IconTone::Success),
                crate::i18n::format("files-name-was-added-9d0b6e35", &[("name", name.clone())]),
                crate::i18n::tr("You find it in the sidebar under Cloud and network.").to_owned(),
            ),
            vec![
                close(crate::i18n::tr("Close")),
                accent(crate::i18n::tr("Open"), Msg::OpenCloudDrive),
            ],
        ),
        CloudState::Ended { service } => (
            status(
                theme,
                icon("circle-alert", IconTone::Danger),
                crate::i18n::format(
                    "files-service-was-not-added-4c7f2b96",
                    &[("service", service.clone())],
                ),
                crate::i18n::tr(
                    "The sign-in was cancelled or did not finish. A notification says why.",
                )
                .to_owned(),
            ),
            vec![
                close(crate::i18n::tr("Close")),
                accent(crate::i18n::tr("Try Again"), Msg::ShowAddCloud),
            ],
        ),
    };
    present(theme, crate::i18n::tr(CLOUD_TITLE), body, actions, window)
}

// ---------------------------------------------------------------------------
// Connect to Server and signing in

pub(crate) const CONNECT_TITLE: &str = "Connect to Server";

const SIGN_IN_PASSWORD: &str = "files-app-sign-in-password";

// The sign-in dialog's password field, focused when the user name is known.
pub(crate) fn sign_in_password_id() -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(SIGN_IN_PASSWORD, crate::i18n::tr("Password"))
}
pub(crate) const SIGN_IN_TITLE: &str = "Sign In";

// Connect to Server with the mount service: one address field, since the
// service works out the protocol and asks for a password, a share or trust in
// a host key only when the server needs it.
pub(crate) fn connect_to_server<'a>(
    theme: &EnzTheme,
    state: &'a crate::network::AttachState,
    window: Size,
) -> Element<'a, Msg> {
    let address = enz_text_field(
        crate::i18n::tr("Server address"),
        "smb://nas.local/share",
        &state.server,
        Msg::AttachServerChanged,
    )
    .description(crate::i18n::tr(
        "For example smb://nas.local/share, sftp://you@server/folder or nfs://server/export",
    ))
    .size(TextFieldSize::Md)
    .on_submit(Msg::ConfirmAttach)
    .id_prefix("files-app-connect-address")
    .into_element();
    let startup = state.can_mount_at_boot();
    let options = enz_preferences_group(
        column![
            enz_preferences_row(crate::i18n::tr("Connect at login"))
                .subtitle(crate::i18n::tr("Reconnect whenever you log in."))
                .switch(state.auto_connect, Msg::SetAttachAutoConnect)
                .id_prefix("files-app-connect-login")
                .into_element(),
            enz_preferences_row(crate::i18n::tr("Mount at startup"))
                .subtitle(if startup {
                    crate::i18n::tr(
                        "Before anyone logs in, for every account. Asks for administrator \
                         approval.",
                    )
                } else {
                    crate::i18n::tr("For SMB shares and NFS exports.")
                })
                .switch(state.mount_at_boot && startup, Msg::SetAttachMountAtBoot)
                .enabled(startup)
                .id_prefix("files-app-connect-startup")
                .into_element(),
        ]
        .width(Length::Fill),
    )
    .bordered(true)
    .into_element();
    present(
        theme,
        crate::i18n::tr(CONNECT_TITLE),
        stack(theme, vec![address, options]),
        vec![
            quiet(crate::i18n::tr("Cancel"), Msg::CloseAttach),
            accent(crate::i18n::tr("Connect"), Msg::ConfirmAttach)
                .enabled(!state.server.trim().is_empty()),
        ],
        window,
    )
}

// The password a saved location asked for, to mount it or to mount it at
// startup. The location is named in the header; the address is not edited
// here.
pub(crate) fn sign_in<'a>(
    theme: &EnzTheme,
    state: &'a crate::network::AttachState,
    target: &'a crate::network::ServiceTarget,
    window: Size,
) -> Element<'a, Msg> {
    use crate::network::{NetworkProtocol, TargetPurpose};
    use enzui_widgets::secure_field::enz_secure_field;
    let mut items = vec![subject(
        theme,
        "network",
        target.name.clone(),
        (!state.server.is_empty()).then(|| state.server.clone()),
    )];
    if target.retry {
        items.push(danger(
            crate::i18n::tr("The password was not accepted"),
            crate::i18n::tr("Check the user name and password, then try again.").to_owned(),
        ));
    } else if target.purpose == TargetPurpose::MountAtBoot {
        items.push(
            enz_text(crate::i18n::tr(
                "Mounting at startup needs this location's password. It is kept encrypted on \
                 this computer.",
            ))
            .tone(TextTone::Muted)
            .into_element(),
        );
    }
    items.push(
        enz_text_field(
            crate::i18n::tr("User name"),
            "",
            &state.username,
            Msg::AttachUsernameChanged,
        )
        .size(TextFieldSize::Md)
        .on_submit(Msg::ConfirmAttach)
        .id_prefix("files-app-sign-in-user")
        .into_element(),
    );
    if state.protocol == NetworkProtocol::Smb {
        items.push(
            enz_text_field(
                crate::i18n::tr("Domain"),
                crate::i18n::tr("Optional"),
                &state.domain,
                Msg::AttachDomainChanged,
            )
            .size(TextFieldSize::Md)
            .on_submit(Msg::ConfirmAttach)
            .id_prefix("files-app-sign-in-domain")
            .into_element(),
        );
    }
    // The plaintext is borrowed for one view pass.
    items.push(
        enz_secure_field(
            state.password.expose_secret(),
            Msg::AttachPasswordChanged,
            state.password_revealed,
            Msg::ToggleAttachPasswordRevealed,
        )
        .label(crate::i18n::tr("Password"))
        .placeholder("")
        .on_submit(Msg::ConfirmAttach)
        .id_prefix(SIGN_IN_PASSWORD)
        .into_element(),
    );
    if target.can_remember {
        items.push(
            enz_checkbox(
                state.remember_password,
                crate::i18n::tr("Remember this password"),
                Msg::SetAttachRememberPassword,
            )
            .tokens(CheckboxTokens::from_palette(&theme.palette))
            .into_element(),
        );
    }
    let confirm = match target.purpose {
        TargetPurpose::Mount => crate::i18n::tr("Connect"),
        TargetPurpose::MountAtBoot => crate::i18n::tr("Continue"),
    };
    present(
        theme,
        crate::i18n::tr(SIGN_IN_TITLE),
        stack(theme, items),
        vec![
            quiet(crate::i18n::tr("Cancel"), Msg::CloseAttach),
            accent(confirm, Msg::ConfirmAttach).enabled(!state.password.expose_secret().is_empty()),
        ],
        window,
    )
}

// ---------------------------------------------------------------------------
// Unlocking an encrypted drive

pub(crate) const UNLOCK_TITLE: &str = "Unlock Drive";

const UNLOCK_PASSPHRASE: &str = "files-app-unlock-passphrase";

// The passphrase field, focused again after a wrong passphrase.
pub(crate) fn unlock_passphrase_id() -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(UNLOCK_PASSPHRASE, crate::i18n::tr("Passphrase"))
}

// A wrong passphrase is answered in place while the encrypted drive stays locked.
pub(crate) fn unlock<'a>(
    theme: &EnzTheme,
    state: &'a crate::mount_service::UnlockState,
    detail: Option<String>,
    window: Size,
) -> Element<'a, Msg> {
    use enzui_widgets::secure_field::enz_secure_field;
    let mut passphrase = enz_secure_field(
        state.passphrase.expose_secret(),
        |text| Msg::UnlockPassphraseChanged(enzui_core::form::SecretText::new(text)),
        state.revealed,
        Msg::ToggleUnlockRevealed,
    )
    .label(crate::i18n::tr("Passphrase"))
    .placeholder("")
    .on_submit(Msg::ConfirmUnlock)
    .id_prefix(UNLOCK_PASSPHRASE);
    if let Some(error) = &state.error {
        passphrase = passphrase.error(error.as_str());
    }
    let mut items = vec![
        subject(theme, "lock", state.name.clone(), detail),
        enz_text(crate::i18n::tr(
            "This drive is encrypted. Enter its passphrase to open it.",
        ))
        .tone(TextTone::Muted)
        .into_element(),
        passphrase.into_element(),
    ];
    if state.busy {
        items.push(
            row![
                enz_spinner().size(16.0).into_element(),
                enz_text(crate::i18n::tr("Unlocking…"))
                    .tone(TextTone::Muted)
                    .into_element()
            ]
            .spacing(theme.spacing.sm())
            .align_y(Alignment::Center)
            .into(),
        );
    }
    present(
        theme,
        crate::i18n::tr(UNLOCK_TITLE),
        stack(theme, items),
        vec![
            quiet(crate::i18n::tr("Cancel"), Msg::CloseUnlock).enabled(!state.busy),
            accent(crate::i18n::tr("Unlock"), Msg::ConfirmUnlock)
                .enabled(!state.busy && !state.passphrase.is_empty()),
        ],
        window,
    )
}

// ---------------------------------------------------------------------------
// Saved locations: rename, share picker and confirmations

// The title of a mount service location dialog.
pub(crate) fn location_title(dialog: &LocationDialog) -> &'static str {
    match dialog {
        LocationDialog::Rename { .. } => crate::i18n::tr("Rename Location"),
        LocationDialog::ChooseShare { .. } => crate::i18n::tr("Choose a Share"),
        LocationDialog::Confirm { confirmation, .. } => match confirmation {
            Confirmation::Smb1 { .. } => crate::i18n::tr("Use an Older Protocol?"),
            Confirmation::Force { .. } => crate::i18n::tr("Files Are Still Open"),
            Confirmation::Remove => crate::i18n::tr("Remove Location?"),
        },
    }
}

pub(crate) fn location<'a>(
    theme: &EnzTheme,
    dialog: &'a LocationDialog,
    window: Size,
) -> Element<'a, Msg> {
    let cancel = quiet(crate::i18n::tr("Cancel"), Msg::CloseLocationDialog);
    let (body, actions): (Element<'a, Msg>, Vec<Action>) = match dialog {
        LocationDialog::Rename { name, .. } => (
            enz_text_field(
                crate::i18n::tr("Name"),
                "",
                name,
                Msg::LocationRenameChanged,
            )
            .description(crate::i18n::tr(
                "Only the name Files shows changes, not the server.",
            ))
            .on_submit(Msg::ConfirmLocationDialog)
            .select_all_on_focus(true)
            .size(TextFieldSize::Md)
            .id_prefix("files-app-location-rename")
            .into_element(),
            vec![
                cancel,
                accent(crate::i18n::tr("Rename"), Msg::ConfirmLocationDialog)
                    .enabled(!name.trim().is_empty()),
            ],
        ),
        LocationDialog::ChooseShare { name, shares, .. } => {
            let mut items = vec![subject(
                theme,
                "server",
                name.clone(),
                Some(crate::i18n::tr("Choose the shared folder to connect to.").to_owned()),
            )];
            if shares.is_empty() {
                items.push(
                    enz_text(crate::i18n::tr("The server lists no shares you can open."))
                        .tone(TextTone::Muted)
                        .into_element(),
                );
            } else {
                let rows: Vec<Element<'a, Msg>> = shares
                    .iter()
                    .enumerate()
                    .map(|(index, share)| {
                        let mut row = enz_preferences_row(&share.name).icon("folder");
                        if !share.comment.is_empty() {
                            row = row.subtitle(&share.comment);
                        }
                        row.action("", Msg::LocationShareChosen(index))
                            .id_prefix(format!("files-app-share-{index}"))
                            .into_element()
                    })
                    .collect();
                items.push(
                    enz_preferences_group(
                        iced::widget::Column::with_children(rows).width(Length::Fill),
                    )
                    .bordered(true)
                    .into_element(),
                );
            }
            (stack(theme, items), vec![cancel])
        }
        LocationDialog::Confirm {
            name, confirmation, ..
        } => {
            let (text, confirm, kind) = match confirmation {
                Confirmation::Smb1 { .. } => (
                    crate::i18n::format(
                        "files-name-only-supports-smb1-an-older-0c5e7f13",
                        &[("name", name.clone())],
                    ),
                    crate::i18n::tr("Connect Anyway"),
                    ButtonKind::Accent,
                ),
                Confirmation::Force { message, .. } => (
                    crate::i18n::format(
                        "files-message-disconnecting-anyway-can-lose-unsaved-4a9e2c70",
                        &[("message", message.clone())],
                    ),
                    crate::i18n::tr("Disconnect Anyway"),
                    ButtonKind::Destructive,
                ),
                Confirmation::Remove => (
                    crate::i18n::format(
                        "files-name-is-disconnected-and-removed-from-files-7d3b9a52",
                        &[("name", name.clone())],
                    ),
                    crate::i18n::tr("Remove"),
                    ButtonKind::Destructive,
                ),
            };
            (
                enz_text(text).into_element(),
                vec![
                    cancel,
                    Action::new(confirm, kind, Msg::ConfirmLocationDialog),
                ],
            )
        }
    };
    present(theme, location_title(dialog), body, actions, window)
}
