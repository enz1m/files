// View settings and file properties.

use super::*;
use iced::widget::column;

// Focus scope shared with the app's generic dialog lifecycle.
pub(crate) const VIEW_SETTINGS_SCOPE: &str = GENERIC_DIALOG_SCOPE;

// The View Settings dialog, built on the framework `enz_settings_dialog` preset.
//
// It has View Settings and About pages. Dismissal is the surface's "Close" X
// rather than a "Done" button, because most controls apply immediately and
// feature pages own their scoped Apply. The surface uses
// `DialogSize::Compact` (the app's `dialog_width_md`), keeps the pinned
// "Settings" header, and fits its content up to the rung's height cap.
pub(crate) fn downloads_settings_page<'a>(model: &'a Model, theme: &EnzTheme) -> Element<'a, Msg> {
    let draft = &model.downloads.settings_draft;
    let down = enz_text_field(
        crate::i18n::tr("Download limit (KiB/s; 0 = unlimited)"),
        "0",
        &draft.max_download_kib,
        Msg::TorrentDownloadLimitChanged,
    )
    .input_purpose(iced::advanced::input_method::Purpose::Digits)
    .into_element();
    let up = enz_text_field(
        crate::i18n::tr("Torrent upload limit (KiB/s; 0 = unlimited)"),
        "0",
        &draft.max_upload_kib,
        Msg::TorrentUploadLimitChanged,
    )
    .input_purpose(iced::advanced::input_method::Purpose::Digits)
    .into_element();
    let concurrency = enz_text_field(
        crate::i18n::tr("Concurrent downloads (1–16)"),
        "5",
        &draft.concurrency,
        Msg::DownloadConcurrencyChanged,
    )
    .input_purpose(iced::advanced::input_method::Purpose::Digits)
    .into_element();
    let mut content = column![
        enz_text(crate::i18n::tr("Each download opens the system Save dialog. Files never write to a typed or remembered path."))
            .size(TextSize::Caption)
            .tone(TextTone::Muted)
            .into_element(),
        down,
        up,
        concurrency,
        enz_text(crate::i18n::format("files-torrent-policy-16b3a907", &[("arg0", model.downloads.settings.torrent_policy.description().to_owned())]))
        .size(TextSize::Caption)
        .tone(TextTone::Muted)
        .into_element(),
    ]
    .spacing(theme.spacing.md())
    .width(Length::Fill);
    // Persistent refusal banner with Retry/Replace.
    if let Some(refusal) = &model.downloads.settings_refusal {
        content = content.push(
            enz_text(crate::i18n::format(
                "files-label-detail",
                &[
                    (
                        "title",
                        crate::i18n::tr(crate::downloads::SETTINGS_REFUSED_TITLE).to_owned(),
                    ),
                    ("detail", refusal.message().to_owned()),
                ],
            ))
            .size(TextSize::Caption)
            .tone(TextTone::Danger)
            .into_element(),
        );
        content = content.push(
            enz_button(
                crate::i18n::tr("Retry loading settings"),
                Msg::RetryDownloadSettings,
            )
            .kind(ButtonKind::Quiet)
            .into_element(),
        );
        content = content.push(
            enz_button(
                crate::i18n::tr("Replace with defaults"),
                Msg::RetireDownloadSettings,
            )
            .kind(ButtonKind::Destructive)
            .into_element(),
        );
    } else if model.downloads.settings_durability_debt {
        // Persistent durability-debt banner.
        content = content.push(
            enz_text(crate::i18n::tr(
                "Settings saved but durability is uncertain",
            ))
            .size(TextSize::Caption)
            .tone(TextTone::Danger)
            .into_element(),
        );
        content = content.push(
            enz_button(
                crate::i18n::tr("Retry saving settings"),
                Msg::RetryDownloadSettings,
            )
            .kind(ButtonKind::Quiet)
            .into_element(),
        );
    }
    if let Some(error) = &draft.error {
        content = content.push(
            enz_text(error.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Danger)
                .into_element(),
        );
    }
    // Block Apply while settings are refused or have durability debt.
    if !model.downloads.has_settings_debt_or_refusal() {
        // Trailing, like every settings subpage's commit button.
        content = content.push(
            row![
                Space::new().width(Length::Fill),
                enz_button(
                    crate::i18n::tr("Apply download settings"),
                    Msg::ApplyDownloadSettings,
                )
                .kind(ButtonKind::Accent)
                .into_element(),
            ]
            .mirrored(crate::i18n::rtl()),
        );
    }
    content.into()
}

// Permission-grid row labels and column headers in 3×3 matrix order; the
// indices are the `(row, col)` carried by `Msg::TogglePermissionBit` and read
// by `PropertiesInfo::get_bit`.
pub(crate) const PERM_ROWS: [&str; 3] = ["Owner", "Group", "Other"];
pub(crate) const PERM_COLS: [&str; 3] = ["Read", "Write", "Exec"];

// Width of one permissions-grid column, shared by header and checkbox cell so
// their centres align.
//
// Documented token exception: `SizingTokens` has no grid-cell alignment width.
pub(crate) const PERM_CELL_WIDTH: f32 = 48.0;

// Width of the permissions-grid row-label lane, so checkbox columns start at
// the same x in every row.
//
// Documented token exception: tuned to hold "Owner" at `TextSize::Footnote`.
pub(crate) const PERM_ROW_LABEL_WIDTH: f32 = 64.0;

// A label/value metadata row that sits side by side when wide and stacks when
// narrow, using `enz_labeled_content`'s auto-compact.
//
// Shared with the detached preview window so its rows match this dialog.
pub(crate) fn meta_row<'a>(label: &'a str, value: impl Into<String>) -> Element<'a, Msg> {
    enz_labeled_content(label).value(value).into_element()
}

// A metadata row that always stacks, with the value left-aligned.
//
// A standalone row is wide enough for [`meta_row`]'s wide mode, which would
// right-align a short value away from its label.
pub(crate) fn meta_row_stacked<'a>(label: &'a str, value: impl Into<String>) -> Element<'a, Msg> {
    enzui_widgets::labeled_content::enz_labeled_content_compact(label)
        .value(value)
        .into_element()
}

// Component-width breakpoint (px) below which a [`meta_pair`] stacks.
//
// Measured on the pair itself, between its desktop width (~380 px) and its
// width at the 375 rung (~270 px).
pub(crate) const PROPERTIES_PAIR_BREAKPOINT: f32 = 340.0;

// Two metadata fields side by side via the framework `reflow_pair`, stacking
// below `PROPERTIES_PAIR_BREAKPOINT` without rebuilding either child.
fn meta_pair<'a>(
    row_gap: f32,
    stack_gap: f32,
    left: Element<'a, Msg>,
    right: Element<'a, Msg>,
) -> Element<'a, Msg> {
    reflow_pair(
        left,
        right,
        PROPERTIES_PAIR_BREAKPOINT,
        PairArm {
            spacing: row_gap,
            leading_width: Length::Fill,
            align: Alignment::Start,
            ..PairArm::default()
        },
        PairArm {
            spacing: stack_gap,
            ..PairArm::default()
        },
    )
    .width(Length::Fill)
    .into()
}

// The Backups section of Properties, for an item in the home when the
// enzimOS history service is installed.
pub(crate) struct BackupInfo {
    pub(crate) path: std::path::PathBuf,
    pub(crate) dir: bool,
    pub(crate) exclusion: crate::history::Exclusion,
}

fn backups_group<'a>(backup: BackupInfo) -> Element<'a, Msg> {
    use crate::history::{Exclusion, HistoryMsg};
    let (subtitle, enabled) = match &backup.exclusion {
        Exclusion::Included => (
            crate::i18n::tr("Kept in backups, with earlier versions every hour.").to_owned(),
            true,
        ),
        Exclusion::Excluded => (crate::i18n::tr("Left out of backups.").to_owned(), true),
        Exclusion::Inside(folder) => (
            crate::i18n::format(
                "files-folder-is-not-backed-up",
                &[(
                    "folder",
                    folder
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                )],
            ),
            false,
        ),
    };
    let path = backup.path.clone();
    let toggle = enz_preferences_row(crate::i18n::tr("Back up"))
        .subtitle(subtitle)
        .switch(backup.exclusion == Exclusion::Included, move |on| {
            Msg::History(HistoryMsg::Exclude(path.clone(), !on))
        })
        .enabled(enabled)
        .id_prefix("files-app-properties-backup")
        .into_element();
    let versions = if backup.dir {
        enz_preferences_row(crate::i18n::tr("Browse Previous Versions"))
            .action("", Msg::History(HistoryMsg::Browse(backup.path)))
    } else {
        enz_preferences_row(crate::i18n::tr("Previous Versions"))
            .action("", Msg::History(HistoryMsg::ShowVersions(backup.path)))
    }
    .id_prefix("files-app-properties-versions")
    .into_element();
    enz_preferences_group(column![toggle, versions].width(Length::Fill))
        .title(crate::i18n::tr("Backups"))
        .into_element()
}

pub(crate) fn properties<'a>(
    theme: &EnzTheme,
    info: &PropertiesInfo,
    art: Option<enzui_widgets::icon::IconSource>,
    perms_expanded: bool,
    dirty: bool,
    backup: Option<BackupInfo>,
    window: iced::Size,
) -> Element<'a, Msg> {
    let sp = &theme.spacing;
    let checkbox_tokens = CheckboxTokens::from_palette(&theme.palette);
    let row_gap = sp.lg();
    let stack_gap = sp.xs();

    // Related fields pair up and stack at narrow widths; long singletons such as
    // a link path keep a full-width row.
    let mut meta = column![].spacing(sp.sm()).width(Length::Fill);

    let type_cell = meta_row(crate::i18n::tr("Type"), info.kind_label.clone());
    if info.mime_type.is_empty() {
        meta = meta.push(type_cell);
    } else {
        meta = meta.push(meta_pair(
            row_gap,
            stack_gap,
            type_cell,
            meta_row(crate::i18n::tr("MIME Type"), info.mime_type.clone()),
        ));
    }

    if info.is_symlink && !info.link_target.is_empty() {
        meta = meta.push(meta_row(
            crate::i18n::tr("Link Target"),
            info.link_target.clone(),
        ));
    }

    // A directory's Total Size arrives asynchronously, with a spinner until then.
    let size_cell: Element<'a, Msg> = if info.recursive_size_pending() {
        let spinner_gap = sp.space3;
        enz_labeled_content(crate::i18n::tr("Total Size"))
            .control_view(move |_| {
                row![
                    // Documented token exception: 14 px sits between `icon_xs`
                    // and `icon_sm` to match `TextSize::Footnote`.
                    enz_spinner().size(14.0).into_element(),
                    enz_text(crate::i18n::tr(RECURSIVE_SIZE_COMPUTING))
                        .size(TextSize::Footnote)
                        .tone(TextTone::Muted)
                        .into_element(),
                ]
                .mirrored(crate::i18n::rtl())
                .spacing(spinner_gap)
                .align_y(Alignment::Center)
                .into()
            })
            .into_element()
    } else if !info.recursive_size_text.is_empty() {
        meta_row(
            crate::i18n::tr("Total Size"),
            info.recursive_size_text.clone(),
        )
    } else {
        meta_row(crate::i18n::tr("Total Size"), info.size_text.clone())
    };
    if info.content_summary.is_empty() {
        // A standalone size row is wide enough for wide mode, which would
        // right-align the value; the stacked form keeps it under its label.
        let stacked_size_cell: Element<'a, Msg> = if info.recursive_size_pending() {
            size_cell
        } else if !info.recursive_size_text.is_empty() {
            meta_row_stacked(
                crate::i18n::tr("Total Size"),
                info.recursive_size_text.clone(),
            )
        } else {
            meta_row_stacked(crate::i18n::tr("Total Size"), info.size_text.clone())
        };
        meta = meta.push(stacked_size_cell);
    } else {
        meta = meta.push(meta_pair(
            row_gap,
            stack_gap,
            size_cell,
            meta_row(crate::i18n::tr("Contents"), info.content_summary.clone()),
        ));
    }

    meta = meta.push(meta_pair(
        row_gap,
        stack_gap,
        meta_row(crate::i18n::tr("Owner"), info.owner.clone()),
        meta_row(crate::i18n::tr("Group"), info.group.clone()),
    ));

    match (info.modified_text.is_empty(), info.accessed_text.is_empty()) {
        (false, false) => {
            meta = meta.push(meta_pair(
                row_gap,
                stack_gap,
                meta_row(crate::i18n::tr("Date modified"), info.modified_text.clone()),
                meta_row(crate::i18n::tr("Accessed"), info.accessed_text.clone()),
            ));
        }
        (false, true) => {
            meta = meta.push(meta_row(
                crate::i18n::tr("Date modified"),
                info.modified_text.clone(),
            ));
        }
        (true, false) => {
            meta = meta.push(meta_row(
                crate::i18n::tr("Accessed"),
                info.accessed_text.clone(),
            ));
        }
        (true, true) => {}
    }

    let meta_group = enz_preferences_group(meta.width(Length::Fill)).into_element();

    // Documented token exception: the leading spacer matches the row-label
    // lane, and headers share `PERM_CELL_WIDTH` with the cells below.
    let mut header = row![Space::new().width(Length::Fixed(PERM_ROW_LABEL_WIDTH))]
        .mirrored(crate::i18n::rtl())
        .spacing(sp.sm());
    for col in PERM_COLS {
        header = header.push(
            iced::widget::container(
                enz_text(crate::i18n::tr(col))
                    .size(TextSize::Caption)
                    .tone(TextTone::Muted)
                    .into_element(),
            )
            .width(Length::Fixed(PERM_CELL_WIDTH))
            .align_x(Alignment::Center),
        );
    }

    let mut grid = column![header].spacing(sp.xs());
    for (r, row_label) in PERM_ROWS.iter().enumerate() {
        let mut grid_row = row![
            iced::widget::container(
                enz_text(crate::i18n::tr(row_label))
                    .size(TextSize::Footnote)
                    .into_element(),
            )
            .width(Length::Fixed(PERM_ROW_LABEL_WIDTH))
        ]
        .mirrored(crate::i18n::rtl())
        .spacing(sp.sm())
        .align_y(Alignment::Center);
        for c in 0..PERM_COLS.len() {
            let checked = info.get_bit(r, c).unwrap_or(false);
            let id = iced::advanced::widget::Id::from(format!("files-app-perm-{r}-{c}"));
            let cell = enz_checkbox(checked, "", move |_| Msg::TogglePermissionBit {
                row: r,
                col: c,
            })
            .tokens(checkbox_tokens)
            .id(id)
            .into_element();
            // Documented token exception: `PERM_CELL_WIDTH` keeps cells under
            // their headers; `iced::widget::container` has no framework wrapper.
            grid_row = grid_row.push(
                iced::widget::container(cell)
                    .width(Length::Fixed(PERM_CELL_WIDTH))
                    .align_x(Alignment::Center),
            );
        }
        grid = grid.push(grid_row);
    }

    // Projected from the same bits so `ls -l` and octal always match the grid.
    let symbolic = info.symbolic_permissions();
    let octal = info.octal_permissions();
    let caption = enz_text(crate::i18n::format(
        "files-permission-codes",
        &[("symbolic", symbolic), ("octal", octal)],
    ))
    .size(TextSize::Caption)
    .tone(TextTone::Muted)
    .into_element();

    let perm_body: Element<'a, Msg> = column![grid, caption]
        .spacing(sp.sm())
        .width(Length::Fill)
        .into();

    let permissions = enz_expander(
        crate::i18n::tr("Permissions"),
        perms_expanded,
        Msg::TogglePermissionsSection,
    )
    .content(perm_body)
    .into_element();

    let title = enz_text(info.name.clone())
        .size(TextSize::Title3)
        .tone(TextTone::Default)
        .heading(HeadingLevel::Page)
        .into_element();
    // The file's own icon beside its name, when the icon theme draws one.
    let title: Element<'a, Msg> = match art {
        Some(art) => row![art.into_element(crate::icon::PROPERTIES_ART), title]
            .mirrored(crate::i18n::rtl())
            .spacing(sp.md())
            .align_y(Alignment::Center)
            .into(),
        None => title,
    };
    let body: Element<'a, Msg> = column![title, meta_group, permissions,]
        .extend(backup.map(backups_group))
        .spacing(sp.lg())
        .width(Length::Fill)
        .into();

    // The surface narrows at the 375 rung so the pair reflow can collapse.
    let width = properties_width(theme, window);
    present_sized(
        theme,
        crate::i18n::tr("Properties"),
        body,
        vec![
            // Inert until a bit differs from `baseline_mode`, which every disk
            // read refreshes.
            Action::new(
                crate::i18n::tr("Apply Permissions"),
                ButtonKind::Accent,
                Msg::ApplyPermissions,
            )
            .enabled(dirty),
        ],
        width,
        window,
    )
}

// The Properties surface width: `dialog_width_md` (460) on desktop rungs,
// shrinking to fit a narrow window with a small margin.
pub(crate) fn properties_width(theme: &EnzTheme, window: iced::Size) -> f32 {
    let max = theme.sizing.dialog_width_md;
    if window.width <= 0.0 {
        return max;
    }
    (window.width - 40.0).clamp(280.0, max)
}
