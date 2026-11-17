// Icon-theme artwork for entries, places and devices; color markers; and the
// bundled MIME glyphs a monochrome list draws.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use enzui_icons::file_type::{FileTypeTable, icon_for_mime, tone_for_mime};
use enzui_theme::Palette;
use enzui_widgets::file_icon::{FileIconQuery, themed_file_icon, themed_icon};
use enzui_widgets::icon::IconSource;
use iced::Color;

use crate::model::{EntryKind, FileEntry};

// The generic file glyph unclassified files fall back to.

pub(crate) const FOLDER: &str = FileTypeTable::DIRECTORY_ICON;
// A symlink whose target is a file.
pub(crate) const SYMLINK: &str = FileTypeTable::SYMLINK_ICON;

// The Lucide icon name for a directory entry.
//
// Kind decides directories and symlinks; everything else uses the framework's
// total MIME map.
pub(crate) fn icon_name_for(entry: &FileEntry) -> &'static str {
    match entry.kind {
        EntryKind::Dir => FOLDER,
        EntryKind::Symlink => SYMLINK,
        EntryKind::File => {
            if entry.mime.is_empty() {
                // The file vanished before its stat; classify by name instead of
                // falling back to the generic glyph.
                FileTypeTable::host().icon_name(&entry.name)
            } else {
                icon_for_mime(&entry.mime)
            }
        }
    }
}

// The ink a file's glyph is painted with, over `backdrop`.
//
// Uses the framework's [`enzui_widgets::tokens::file_type_ink`], which raises
// dim families to the non-text contrast floor against `backdrop`. Pass the fill
// really behind the glyph ([`list_row_backdrop`], [`grid_tile_backdrop`] or the
// pane's), since selection replaces a row's fill.
pub(crate) fn icon_color_for(
    entry: &FileEntry,
    palette: &Palette,
    colored: bool,
    backdrop: Color,
) -> Color {
    if matches!(entry.kind, EntryKind::Dir) {
        return palette.accent;
    }
    if !colored || !matches!(entry.kind, EntryKind::File) {
        return palette.text_muted;
    }
    enzui_widgets::tokens::file_type_ink(tone_for_mime(&entry.mime), palette, backdrop)
}

// A listed entry's glyph ink: a tagged folder's colour raised to the 3:1 floor
// over `backdrop`, else [`icon_color_for`].
pub(crate) fn entry_icon_ink(
    model: &crate::model::Model,
    entry: &FileEntry,
    palette: &Palette,
    backdrop: Color,
) -> Color {
    if matches!(entry.kind, EntryKind::Dir)
        && let Some(key) = model.folder_color(&entry.path)
    {
        let tint = enzui_theme::WorkspaceAccentPalette::for_mode(palette).resolve(key);
        return enzui_theme::raise_to_floor(tint, backdrop, enzui_theme::ContrastFloor::NonText)
            .color();
    }
    icon_color_for(entry, palette, model.colored_file_icons, backdrop)
}

// A listed entry's accessible name: its file name, plus a tagged folder's
// colour, so the tag is not conveyed by colour alone.
pub(crate) fn entry_accessible_name(model: &crate::model::Model, entry: &FileEntry) -> String {
    match model.folder_color(&entry.path) {
        Some(key) if matches!(entry.kind, EntryKind::Dir) => {
            format!("{} ({})", entry.name, key.label())
        }
        _ => entry.name.clone(),
    }
}

// The fill behind an entry glyph in the list pane: none at rest, since the pane
// is blended, and the framework row's selected fill when selected.
pub(crate) fn list_row_backdrop(palette: &Palette, selected: bool) -> Color {
    if selected {
        enzui_widgets::list_row_surface::ListRowSurfaceTokens::from_palette(palette)
            .selected_background
    } else {
        palette.background
    }
}

// The fill behind an entry glyph in the grid pane.
//
// A selected tile uses `PressableStyle::Plain`'s `accent_soft`, not the list's
// `surface_selected`; the style resolves it so the two cannot drift.
pub(crate) fn grid_tile_backdrop(palette: &Palette, selected: bool) -> Color {
    if selected {
        enzui_widgets::pressable::PressableStyle::Plain.selected_fill(
            &enzui_widgets::pressable::PressableTokens::from_palette(palette),
        )
    } else {
        palette.background
    }
}

// How much larger theme artwork is drawn than the glyph it replaces: the art
// sits on a padded 128 px canvas with room for its shadow, so at the glyph's
// size a page would be about half as wide as the glyph. Small list glyphs need
// the larger factor for a page to stay legible beside body text.
fn art_scale(glyph_size: f32) -> f32 {
    if glyph_size < 32.0 { 1.5 } else { 4.0 / 3.0 }
}

// Find the icon theme and list it, off the render thread, before the first
// directory is drawn.
pub(crate) fn initialize() {
    let _ = theme_available();
    let _ = special_folders();
}

// Whether the desktop icon theme draws file icons at all.
pub(crate) fn theme_available() -> bool {
    themed_file_icon(&FileIconQuery::mime("application/octet-stream"), 24.0).is_some()
}

// Whether entries draw theme artwork rather than tinted glyphs.
pub(crate) fn uses_art(model: &crate::model::Model) -> bool {
    model.colored_file_icons && theme_available()
}

// The icon slot for a view whose glyphs are `glyph_size`: larger when the
// view draws theme artwork.
pub(crate) fn slot_size(model: &crate::model::Model, glyph_size: f32) -> f32 {
    if uses_art(model) {
        (glyph_size * art_scale(glyph_size)).round()
    } else {
        glyph_size
    }
}

// The XDG user directories and Home, by path, with their theme icon names.
fn special_folders() -> &'static HashMap<PathBuf, &'static str> {
    static FOLDERS: OnceLock<HashMap<PathBuf, &'static str>> = OnceLock::new();
    FOLDERS.get_or_init(|| {
        let mut map = HashMap::new();
        let home = dirs::home_dir();
        for (dir, icon) in [
            (dirs::document_dir(), "folder-documents"),
            (dirs::download_dir(), "folder-download"),
            (dirs::picture_dir(), "folder-pictures"),
            (dirs::audio_dir(), "folder-music"),
            (dirs::video_dir(), "folder-videos"),
            (dirs::public_dir(), "folder-publicshare"),
            (dirs::template_dir(), "folder-templates"),
            (dirs::desktop_dir(), "user-desktop"),
        ] {
            // A disabled XDG directory points at Home itself.
            if let Some(dir) = dir.filter(|dir| Some(dir) != home.as_ref()) {
                map.entry(dir).or_insert(icon);
            }
        }
        if let Some(home) = home {
            map.insert(home, "user-home");
        }
        map
    })
}

// The theme names for a folder at `path`: its special icon, then `folder`.
pub(crate) fn folder_icon_names(path: &Path) -> Vec<&'static str> {
    special_folders()
        .get(path)
        .copied()
        .into_iter()
        .chain(["folder"])
        .collect()
}

// Theme artwork for a folder at `path`.
pub(crate) fn folder_art(path: &Path, size: f32) -> Option<IconSource> {
    themed_icon(&folder_icon_names(path), size)
}

// Theme artwork for an entry: its folder or file-type icon.
pub(crate) fn entry_art(entry: &FileEntry, size: f32) -> Option<IconSource> {
    match entry.kind {
        EntryKind::Dir => folder_art(&entry.path, size),
        EntryKind::File | EntryKind::Symlink => {
            let query = if entry.mime.is_empty() {
                FileIconQuery::file_name(entry.name.clone())
            } else {
                FileIconQuery::mime(entry.mime.clone())
            };
            themed_file_icon(&query, size)
        }
    }
}

// A small round badge at the slot's bottom corner over `base`, `inset` px in
// from the side (a page is narrower than its slot; a folder fills it).
fn with_badge<'a>(
    base: iced::Element<'a, crate::message::Msg>,
    badge: iced::Element<'a, crate::message::Msg>,
    size: f32,
    leading: bool,
    inset: f32,
) -> iced::Element<'a, crate::message::Msg> {
    use iced::widget::{container, stack};
    use iced::{Alignment, Length, Padding};
    let leading = leading != crate::i18n::rtl();
    let padding = if leading {
        Padding::ZERO.left(inset)
    } else {
        Padding::ZERO.right(inset)
    };
    stack![
        base,
        container(badge)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .padding(padding)
            .align_x(if leading {
                Alignment::Start
            } else {
                Alignment::End
            })
            .align_y(Alignment::End)
    ]
    .into()
}

// The colour-tag marker: a solid dot in the tag's colour.
fn colour_marker<'a>(color: Color, size: f32) -> iced::Element<'a, crate::message::Msg> {
    use iced::Length;
    let diameter = (size * 0.4).clamp(8.0, 18.0);
    enzui_widgets::surface::enz_surface(iced::widget::Space::new())
        .width(Length::Fixed(diameter))
        .height(Length::Fixed(diameter))
        .radius(diameter / 2.0)
        .fill(color)
        .into()
}

// The link badge on a symbolic link's artwork: the theme's
// `emblem-symbolic-link`, else an arrow on a light disc.
fn link_badge<'a>(palette: &Palette, size: f32) -> iced::Element<'a, crate::message::Msg> {
    use enzui_widgets::icon::enz_icon;
    use iced::Length;
    // The emblem is drawn for 12–24 px.
    let diameter = (size * 0.42).clamp(12.0, 22.0).round();
    if let Some(emblem) = themed_icon(&["emblem-symbolic-link"], diameter) {
        return emblem.into_element(diameter);
    }
    enzui_widgets::surface::enz_surface(
        iced::widget::container(
            enz_icon("arrow-up-right")
                .size((diameter * 0.72).round())
                .tint(palette.text)
                .into_element(),
        )
        .center(Length::Fill),
    )
    .width(Length::Fixed(diameter))
    .height(Length::Fixed(diameter))
    .radius(diameter / 2.0)
    .fill(palette.surface)
    .into()
}

// An entry's icon in a `size` px slot: theme artwork when the view uses it
// (folders always, as the desktop draws them), else the tinted glyph. A tagged
// folder carries its colour dot; a link carries an arrow badge.
pub(crate) fn entry_element<'a>(
    model: &crate::model::Model,
    entry: &FileEntry,
    palette: &Palette,
    backdrop: Color,
    size: f32,
) -> iced::Element<'a, crate::message::Msg> {
    use enzui_widgets::icon::enz_icon;
    let art = if entry.kind == EntryKind::Dir || model.colored_file_icons {
        entry_art(entry, size)
    } else {
        None
    };
    let Some(art) = art else {
        return enz_icon(icon_name_for(entry))
            .size(size)
            .tint(entry_icon_ink(model, entry, palette, backdrop))
            .into();
    };
    let base = art.into_element(size);
    if entry.kind == EntryKind::Dir && model.folder_color(&entry.path).is_some() {
        let color = entry_icon_ink(model, entry, palette, backdrop);
        return with_badge(base, colour_marker(color, size), size, false, 0.0);
    }
    if entry.kind == EntryKind::Symlink {
        // The page's corner sits about a seventh of the slot in from its side.
        return with_badge(
            base,
            link_badge(palette, size),
            size,
            true,
            (size * 0.14).round(),
        );
    }
    base
}

// An icon for a file known only by name (Trash, history): theme artwork in a
// `size` slot when the view uses it, else the tinted `glyph_size` glyph.
pub(crate) fn named_element<'a>(
    model: &crate::model::Model,
    name: &str,
    is_dir: bool,
    size: f32,
) -> iced::Element<'a, crate::message::Msg> {
    use enzui_widgets::icon_slot::enz_icon_slot;
    let query = if is_dir {
        FileIconQuery::directory()
    } else {
        FileIconQuery::file_name(name)
    };
    let art = if is_dir {
        themed_icon(&["folder"], size)
    } else if model.colored_file_icons {
        themed_file_icon(&query, size)
    } else {
        None
    };
    match art {
        Some(art) => art.into_element(size),
        None => enz_icon_slot(query.glyph())
            .icon_size(size)
            .slot_width(size)
            .into_element(),
    }
}

// Theme artwork beside a two-line row (downloads, history).
pub(crate) const ROW_ART: f32 = 32.0;

// The icon beside a two-line row for a file known by name: theme artwork at
// [`ROW_ART`], else the `glyph_size` glyph.
pub(crate) fn row_file_icon<'a>(
    name: &str,
    is_dir: bool,
    glyph_size: f32,
) -> iced::Element<'a, crate::message::Msg> {
    use enzui_widgets::icon_slot::enz_icon_slot;
    let query = if is_dir {
        FileIconQuery::directory()
    } else {
        FileIconQuery::file_name(name)
    };
    match themed_file_icon(&query, ROW_ART) {
        Some(art) => art.into_element(ROW_ART),
        None => enz_icon_slot(query.glyph())
            .icon_size(glyph_size)
            .slot_width(glyph_size)
            .into_element(),
    }
}

// The icon beside the Properties title.
pub(crate) const PROPERTIES_ART: f32 = 48.0;

// Theme artwork for the Properties title: the folder's icon, or the file's
// type icon when the view draws theme art.
pub(crate) fn properties_art(
    model: &crate::model::Model,
    info: &crate::model::PropertiesInfo,
) -> Option<IconSource> {
    if info.icon == "folder" {
        return folder_art(&info.path, PROPERTIES_ART);
    }
    if !model.colored_file_icons {
        return None;
    }
    let query = if info.mime_type.is_empty() {
        FileIconQuery::file_name(info.name.clone())
    } else {
        FileIconQuery::mime(info.mime_type.clone())
    };
    themed_file_icon(&query, PROPERTIES_ART)
}
