// Menu models for entries, sidebar, background, and trash.

use super::*;

pub(crate) fn menu_model(
    model: &Model,
    path: &Path,
    is_dir: bool,
) -> Vec<ContextMenuItem<Command>> {
    let mut entries: Vec<ContextMenuItem<Command>> = Vec::new();
    let mut previous: Option<RowGroup> = None;

    // Pin verbs are stateful and directory-only, so they are inserted per open
    // rather than living in the fixed `ALL` order.
    let mut pin_row: Option<RowKind> = if is_dir && !crate::services::is_virtual(path) {
        Some(if model.is_pinned(path) {
            RowKind::UnpinFromSidebar
        } else {
            RowKind::PinToSidebar
        })
    } else {
        None
    };

    // The Backup submenu and extension actions form one group placed before
    // Open With, Preview and Properties, so Properties stays the last row.
    let mut extras: Vec<ContextMenuItem<Command>> = Vec::new();
    if let Some(backup) = backup_submenu(model, path, is_dir) {
        extras.push(ContextMenuItem::separator());
        extras.push(backup);
    }
    append_extensions(model, path, false, &mut extras);

    let effective_selection_count = if model.is_selected(path) {
        model.selection.len()
    } else {
        1
    };
    for row in RowKind::ALL {
        let include = match row {
            RowKind::MountImage => !is_dir && crate::storage::is_disk_image(path),
            RowKind::RevealParent => {
                model.search_active && model.search_scope == crate::model::SearchScope::Everywhere
            }
            RowKind::OpenPreview => row.enabled(model, path, is_dir),
            // A directory is browsed in place, so the chooser has nothing to offer.
            RowKind::OpenWith => !is_dir,
            // A terminal opens in a folder; a file has none of its own.
            RowKind::OpenInTerminal => is_dir && !crate::services::is_virtual(path),
            // Only folders carry a colour tag.
            RowKind::FolderColor => is_dir && !crate::services::is_virtual(path),
            RowKind::Rename => effective_selection_count == 1,
            RowKind::RenameMultiple => effective_selection_count > 1,
            // Shown for a regular `.torrent` even without the optional backend;
            // `enabled` disables it.
            RowKind::DownloadTorrent => is_regular_torrent_entry(model, path, is_dir),
            // Archive files only, so other menus are not padded with greyed rows.
            RowKind::ExtractHere | RowKind::ExtractTo => {
                !is_dir && crate::archive::ArchiveFormat::detect(path).is_some()
            }
            _ => true,
        };
        let supported = match row {
            RowKind::Rename | RowKind::RenameMultiple => true,
            _ => row.enabled(model, path, is_dir),
        };
        if !include || !supported {
            continue;
        }
        if let Some(prev) = previous
            && prev != row.group()
        {
            // Pin closes the Open group.
            if prev == RowGroup::Navigate
                && let Some(pin) = pin_row.take()
            {
                entries.push(
                    ContextMenuItem::command(Command::Row(pin), pin.label())
                        .icon(pin.icon())
                        .enabled(pin.enabled(model, path, is_dir)),
                );
            }
            if row.group() == RowGroup::Inspect {
                // `extras` starts with its own separator when it has rows.
                entries.append(&mut extras);
            }
            entries.push(ContextMenuItem::separator());
        }
        previous = Some(row.group());

        let enabled = match row {
            RowKind::Rename => effective_selection_count == 1,
            RowKind::RenameMultiple => effective_selection_count > 1,
            _ => row.enabled(model, path, is_dir),
        };
        // The two archive formats are one "Compress" row with a submenu.
        if row == RowKind::CompressTarGz {
            continue;
        }
        if row == RowKind::CompressZip {
            let format = |kind: RowKind, label: &str| {
                ContextMenuItem::command(Command::Row(kind), label)
                    .icon(kind.icon())
                    .enabled(kind.enabled(model, path, is_dir))
            };
            entries.push(
                ContextMenuItem::submenu(
                    crate::i18n::tr("Compress"),
                    vec![
                        format(RowKind::CompressZip, "ZIP"),
                        format(RowKind::CompressTarGz, "TAR.GZ"),
                    ],
                )
                .icon(row.icon()),
            );
            continue;
        }
        let mut entry = ContextMenuItem::command(Command::Row(row), row.label())
            .icon(row.icon())
            .enabled(enabled);
        if let Some(chord) = row.shortcut() {
            entry = entry.shortcut(chord);
        }
        entries.push(entry);
    }
    // A menu that ends in the Open group still gets its pin row.
    if let Some(pin) = pin_row {
        entries.push(
            ContextMenuItem::command(Command::Row(pin), pin.label())
                .icon(pin.icon())
                .enabled(pin.enabled(model, path, is_dir)),
        );
    }

    // A menu without an Inspect group still lists them, at the end.
    entries.append(&mut extras);
    entries
}

// The Backup submenu for a file or folder in the person's home, when the
// history service is installed.
fn backup_submenu(model: &Model, path: &Path, is_dir: bool) -> Option<ContextMenuItem<Command>> {
    if !model.history_available || !crate::history::covers(&model.home, path) {
        return None;
    }
    let exclusion = crate::history::exclusion(&model.history_excluded, path);
    let mut items = vec![if is_dir {
        ContextMenuItem::command(
            Command::Backup(BackupAction::Browse),
            crate::i18n::tr("Browse Previous Versions…"),
        )
        .icon("history")
    } else {
        ContextMenuItem::command(
            Command::Backup(BackupAction::Versions),
            crate::i18n::tr("Previous Versions…"),
        )
        .icon("history")
    }];
    // Inside an excluded folder, only that folder can be included again.
    match exclusion {
        crate::history::Exclusion::Excluded => items.push(
            ContextMenuItem::command(
                Command::Backup(BackupAction::Include),
                crate::i18n::tr("Include in Backups"),
            )
            .icon("folder-check"),
        ),
        crate::history::Exclusion::Included => items.push(
            ContextMenuItem::command(
                Command::Backup(BackupAction::Exclude),
                crate::i18n::tr("Exclude from Backups"),
            )
            .icon("folder-x"),
        ),
        crate::history::Exclusion::Inside(_) => {}
    }
    items.push(ContextMenuItem::separator());
    items.push(
        ContextMenuItem::command(
            Command::Backup(BackupAction::BackUpNow),
            crate::i18n::tr("Back Up Now"),
        )
        .icon("hard-drive-upload"),
    );
    Some(ContextMenuItem::submenu(crate::i18n::tr("Backup"), items).icon("archive-restore"))
}

pub(crate) fn sidebar_menu_model() -> Vec<ContextMenuItem<Command>> {
    vec![
        ContextMenuItem::command(
            Command::Row(RowKind::UnpinFromSidebar),
            RowKind::UnpinFromSidebar.label(),
        )
        .icon(RowKind::UnpinFromSidebar.icon()),
    ]
}

// The commands for a mount service location.
pub(crate) fn location_menu_model(
    location: &crate::storage::Location,
) -> Vec<ContextMenuItem<Command>> {
    use crate::mount_service::LocationAction;
    let actions = crate::mount_service::location_actions(location);
    let mut entries = Vec::new();
    for (index, &action) in actions.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| actions[i]);
        let starts_group = previous.is_some_and(|previous| {
            (previous.connects() && !action.connects())
                || matches!(action, LocationAction::Format | LocationAction::Remove)
        });
        if starts_group {
            entries.push(ContextMenuItem::separator());
        }
        let mut entry =
            ContextMenuItem::command(Command::Location(action), action.label()).icon(action.icon());
        if action == LocationAction::Remove {
            entry = entry.destructive(true);
        }
        entries.push(entry);
    }
    entries
}

fn location_for<'a>(model: &'a Model, path: &Path) -> Option<&'a crate::storage::Location> {
    let id = path.to_str()?;
    model
        .storage_locations
        .iter()
        .find(|location| location.id == id && location.service.is_some())
}

pub(crate) fn background_menu_model(model: &Model) -> Vec<ContextMenuItem<Command>> {
    let mut entries = Vec::new();
    for row in BackgroundRowKind::ALL {
        if matches!(
            row,
            BackgroundRowKind::NewFolder
                | BackgroundRowKind::NewFile
                | BackgroundRowKind::OpenInTerminal
                | BackgroundRowKind::PasteAsLink
        ) && !can_create_here(model)
        {
            continue;
        }
        if row.starts_group() && !entries.is_empty() {
            entries.push(ContextMenuItem::separator());
        }
        let enabled = match row {
            BackgroundRowKind::NewFolder
            | BackgroundRowKind::NewFile
            | BackgroundRowKind::OpenInTerminal => {
                model.screen == crate::model::Screen::Directory
                    && !crate::services::is_virtual(&model.cwd)
            }
            BackgroundRowKind::Paste => model.can_paste(),
            BackgroundRowKind::PasteAsLink => {
                model.can_paste() && !crate::services::is_virtual(&model.cwd)
            }
            BackgroundRowKind::SelectAll => !model.visible_provider.rows.is_empty(),
            BackgroundRowKind::Undo => model.journal.can_undo(),
            BackgroundRowKind::Redo => model.journal.can_redo(),
        };
        let icon = row.icon();
        // `journal::describe` yields file names only, never paths.
        let label: String = match row {
            BackgroundRowKind::Undo => match model.journal.peek_undo() {
                Some(top) => crate::i18n::format(
                    "files-undo-9f0b38d1",
                    &[("arg0", (crate::journal::describe(top)).clone())],
                ),
                None => crate::i18n::tr("Undo").to_owned(),
            },
            BackgroundRowKind::Redo => match model.journal.peek_redo() {
                Some(top) => crate::i18n::format(
                    "files-redo-af8c5ab0",
                    &[("arg0", (crate::journal::describe(top)).clone())],
                ),
                None => crate::i18n::tr("Redo").to_owned(),
            },
            other => other.label().to_owned(),
        };
        entries.push(
            ContextMenuItem::command(Command::Background(row), label)
                .icon(icon)
                .enabled(enabled),
        );
        if row == BackgroundRowKind::NewFile {
            entries.extend(template_rows(model));
        }
    }
    if model.history_available
        && model.screen == crate::model::Screen::Directory
        && (crate::history::covers(&model.home, &model.cwd) || model.cwd == model.home)
    {
        entries.push(ContextMenuItem::separator());
        entries.push(
            ContextMenuItem::command(
                Command::Backup(BackupAction::Browse),
                crate::i18n::tr("Browse Previous Versions…"),
            )
            .icon("history"),
        );
    }
    append_extensions(model, &model.cwd, true, &mut entries);
    entries
}

fn can_create_here(model: &Model) -> bool {
    model.screen == crate::model::Screen::Directory && !crate::services::is_virtual(&model.cwd)
}

fn template_rows(model: &Model) -> Vec<ContextMenuItem<Command>> {
    use crate::templates::Templates;
    match &model.templates {
        Templates::Unscanned | Templates::Absent | Templates::Unreadable(_) => Vec::new(),
        Templates::Found(found) => found
            .iter()
            .map(|template| {
                ContextMenuItem::command(
                    Command::Template(template.path.clone()),
                    crate::i18n::format(
                        "files-new-68bac383",
                        &[("arg0", (template.label).clone())],
                    ),
                )
                .icon("file-text")
                .enabled(can_create_here(model))
            })
            .collect(),
    }
}

pub(crate) fn trash_menu_model() -> Vec<ContextMenuItem<Command>> {
    RowKind::TRASH_ALL
        .into_iter()
        .map(|row| ContextMenuItem::command(Command::Row(row), row.label()).icon(row.icon()))
        .collect()
}

// The visible entry the open menu was raised on, if it still exists.
//
// `None` when no menu is up or the entry vanished after the right-click.
pub(super) fn open_entry(model: &Model) -> Option<crate::model::FileEntry> {
    let open = model.context_menu.as_ref()?;
    if open.origin != ContextMenuOrigin::Entry {
        return None;
    }
    model.entry_for_path(&open.path).cloned()
}

// The mime of the entry at `path` in `entries` or `search_results`, or `""`.
//
// An unknown path yields `""`, which `services::is_previewable_mime` treats as
// unsupported, so the preview row stays disabled.
pub(super) fn preview_mime_of<'a>(model: &'a Model, path: &Path) -> &'a str {
    model
        .entries
        .iter()
        .chain(model.search_results.iter())
        .find(|entry| entry.path == path)
        .map_or("", |entry| entry.mime.as_str())
}

pub(super) fn open_trash_entry(model: &Model) -> Option<&crate::trash::TrashEntry> {
    let open = model.context_menu.as_ref()?;
    if open.origin != ContextMenuOrigin::Entry {
        return None;
    }
    model
        .trash_entries
        .iter()
        .find(|entry| entry.trashed_path == open.path)
}

pub(crate) fn items_for_target(
    model: &Model,
    path: &Path,
    origin: ContextMenuOrigin,
) -> Option<Vec<ContextMenuItem<Command>>> {
    match origin {
        ContextMenuOrigin::Sidebar => model.is_pinned(path).then(sidebar_menu_model),
        ContextMenuOrigin::Background => (model.cwd == path).then(|| background_menu_model(model)),
        ContextMenuOrigin::Location => location_for(model, path).map(location_menu_model),
        ContextMenuOrigin::Entry => {
            if model
                .trash_entries
                .iter()
                .any(|entry| entry.trashed_path == path)
            {
                Some(trash_menu_model())
            } else {
                let entry = model
                    .projected_entries()
                    .into_iter()
                    .find(|entry| entry.path == path)?;
                Some(menu_model(
                    model,
                    path,
                    matches!(entry.kind, EntryKind::Dir),
                ))
            }
        }
    }
}

pub(crate) fn command_allowed(items: &[ContextMenuItem<Command>], command: &Command) -> bool {
    items.iter().any(|item| match item {
        ContextMenuItem::Command {
            command: candidate,
            enabled,
            ..
        } => *enabled && candidate == command,
        ContextMenuItem::Submenu { items, .. } => command_allowed(items, command),
        ContextMenuItem::Separator => false,
    })
}

pub(crate) fn menu_label(model: &Model) -> Option<String> {
    match model.context_menu.as_ref()?.origin {
        ContextMenuOrigin::Sidebar => Some(crate::i18n::tr("Pinned folder actions").to_owned()),
        ContextMenuOrigin::Background => {
            Some(crate::i18n::tr("Folder background actions").to_owned())
        }
        ContextMenuOrigin::Location => {
            let open = model.context_menu.as_ref()?;
            location_for(model, &open.path).map(|location| location.name.clone())
        }
        ContextMenuOrigin::Entry => open_trash_entry(model)
            .map(|entry| entry.name.clone())
            .or_else(|| open_entry(model).map(|entry| entry.name)),
    }
}

fn append_extensions(
    model: &Model,
    path: &Path,
    background: bool,
    menu: &mut Vec<ContextMenuItem<Command>>,
) {
    if crate::services::is_virtual(&model.cwd) {
        return;
    }
    if !background && model.is_selected(path) && model.selection.len() > 128 {
        return;
    }
    let entries: Vec<_> = if background {
        Vec::new()
    } else {
        model
            .projected_entries()
            .into_iter()
            .filter(|entry| {
                if model.is_selected(path) {
                    model.is_selected(&entry.path)
                } else {
                    entry.path == path
                }
            })
            .cloned()
            .collect()
    };
    let actions = model.extensions.actions(&entries, background);
    if !actions.is_empty() {
        menu.push(ContextMenuItem::separator());
    }
    for (extension, action) in actions {
        menu.push(
            ContextMenuItem::command(
                Command::Extension {
                    extension: extension.id.clone(),
                    action: action.id.clone(),
                },
                extension.display_text(&action.label),
            )
            .icon(&action.icon)
            .enabled(!model.extensions.busy),
        );
    }
}
