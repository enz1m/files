// File entries, providers, thumbnails, and kinds.

use super::*;

// One row in the file view. `path` is the stable identity for keyed lists.
#[derive(Clone, Debug)]
pub(crate) struct FileEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) kind: EntryKind,
    pub(crate) size: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) is_hidden: bool,
    pub(crate) mime: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntrySource {
    Directory,
    Search,
}

// A cheap provider item: index storage, not a duplicate `FileEntry`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectedEntry {
    pub(crate) source: EntrySource,
    pub(crate) index: usize,
    pub(crate) key: SequenceKey,
}

// The keyed projection consumed by both virtual views, rebuilt only when
// filtering, sorting or source data changes.
#[derive(Clone, Debug, Default)]
pub(crate) struct FileEntryProvider {
    pub(crate) rows: Vec<ProjectedEntry>,
    pub(crate) selection_order: Option<IndexedClickModel<PathBuf>>,
    pub(crate) indices: std::collections::HashMap<SequenceKey, u64>,
    // Widest painted Size / Modified / Type cell, measured with the list cells'
    // typography when the source changes.
    pub(crate) metadata_cell_widths: [f32; 3],
    // Fingerprint of the source fields behind metadata-cell text; filtering and
    // sorting keep the widths, so a query cannot hide a header.
    pub(crate) metadata_fingerprint: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionPolicy {
    show_hidden: bool,
    search_active: bool,
    search_query: String,
    search_scope: SearchScope,
    search_filter_type: SearchFilterType,
    search_filter_date: SearchFilterDate,
    sort_ascending: bool,
    sort_folders_first: bool,
    sort_key: SortKey,
}

impl ProjectionPolicy {
    pub(crate) fn from_model(model: &Model) -> Self {
        Self {
            show_hidden: model.show_hidden,
            search_active: model.search_active,
            search_query: model.search_query.clone(),
            search_scope: model.search_scope,
            search_filter_type: model.search_filter_type,
            search_filter_date: model.search_filter_date,
            sort_ascending: model.sort_ascending,
            sort_folders_first: model.sort_folders_first,
            sort_key: model.sort_key,
        }
    }

    pub(crate) fn recursive(&self) -> bool {
        self.search_active && self.search_scope == SearchScope::Everywhere
    }
}

impl FileEntryProvider {
    pub(crate) fn build(
        source: &[FileEntry],
        source_kind: EntrySource,
        policy: &ProjectionPolicy,
        cached: &Self,
    ) -> Self {
        let query = policy.search_query.to_lowercase();
        let now = SystemTime::now();
        let mut rows: Vec<(usize, &FileEntry)> = source
            .iter()
            .enumerate()
            .filter(|(_, e)| policy.show_hidden || !e.is_hidden)
            .filter(|(_, e)| {
                !policy.search_active || query.is_empty() || e.name.to_lowercase().contains(&query)
            })
            .filter(|(_, e)| !policy.search_active || policy.search_filter_type.matches(e))
            .filter(|(_, e)| !policy.search_active || policy.search_filter_date.matches(e, now))
            .collect();
        let asc = policy.sort_ascending;
        let folders_first = policy.sort_folders_first;
        let key = policy.sort_key;
        // Type labels are built once per row, not once per comparison.
        let kind_labels: std::collections::HashMap<usize, String> = if key == SortKey::Kind {
            rows.iter()
                .map(|(index, entry)| (*index, entry.kind.label(&entry.mime)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };
        rows.sort_by(|(ia, a), (ib, b)| {
            if folders_first {
                let dir_cmp = dir_priority(a).cmp(&dir_priority(b));
                if dir_cmp != Ordering::Equal {
                    return dir_cmp;
                }
            }
            let cmp = match key {
                SortKey::Name => natural_cmp(&a.name, &b.name),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Modified => a.modified.cmp(&b.modified),
                SortKey::Kind => natural_cmp(
                    kind_labels.get(ia).map_or("", String::as_str),
                    kind_labels.get(ib).map_or("", String::as_str),
                ),
            };
            if asc { cmp } else { cmp.reverse() }
        });

        // Inventory-wide widths keep filtering from changing the column layout.
        let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
        (source_kind as u8).hash(&mut fingerprint);
        for entry in source {
            entry.size.hash(&mut fingerprint);
            entry.modified.hash(&mut fingerprint);
            std::mem::discriminant(&entry.kind).hash(&mut fingerprint);
            entry.mime.hash(&mut fingerprint);
        }
        let fingerprint = fingerprint.finish();
        let cell_widths = if cached.metadata_fingerprint == Some(fingerprint) {
            cached.metadata_cell_widths
        } else {
            enzui_widgets::sans_serif_fallback::ensure_sans_serif_fallback();
            let typography = MeasureTypography::new(TextSize::Caption);
            let mut widths = [0.0_f32; 3];
            let mut labels: [std::collections::BTreeSet<String>; 3] = Default::default();
            for entry in source {
                labels[0].insert(services::format_size(entry));
                labels[1].insert(services::format_modified(entry));
                labels[2].insert(entry.kind.label(&entry.mime));
            }
            for (column, values) in labels.into_iter().enumerate() {
                for value in values {
                    widths[column] = widths[column].max(enz_text_measure_width(&value, typography));
                }
            }
            widths
        };
        let projected: Vec<_> = rows
            .into_iter()
            .map(|(index, entry)| ProjectedEntry {
                source: source_kind,
                index,
                key: SequenceKey::new(crate::drag_drop::path_to_id(&entry.path).as_str()),
            })
            .collect();
        FileEntryProvider {
            selection_order: Some(IndexedClickModel::new(
                projected.iter().map(|row| source[row.index].path.clone()),
            )),
            indices: projected
                .iter()
                .enumerate()
                .map(|(index, row)| (row.key.clone(), index as u64))
                .collect(),
            rows: projected,
            metadata_cell_widths: cell_widths,
            metadata_fingerprint: Some(fingerprint),
        }
    }
}

impl ItemProvider<ProjectedEntry> for FileEntryProvider {
    fn len(&self) -> u64 {
        self.rows.len() as u64
    }

    fn item(&self, index: u64) -> Option<ProjectedEntry> {
        self.rows.get(usize::try_from(index).ok()?).cloned()
    }
}

impl KeyedProvider<ProjectedEntry> for FileEntryProvider {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.rows
            .get(usize::try_from(index).ok()?)
            .map(|row| row.key.clone())
    }

    fn index_of(&self, key: &SequenceKey) -> Option<u64> {
        self.indices.get(key).copied()
    }
}

impl TileProvider<ProjectedEntry> for FileEntryProvider {}

// One source stamp's thumbnail lifecycle. `Failed` is kept so a broken image
// falls back to its MIME icon instead of retrying every frame.
#[derive(Clone, Debug)]
pub(crate) enum ThumbnailState {
    Loading(u64),
    Ready {
        mtime: u64,
        handle: iced::widget::image::Handle,
    },
    Failed(u64),
}

impl ThumbnailState {
    pub(crate) fn mtime(&self) -> u64 {
        match self {
            Self::Loading(mtime) | Self::Failed(mtime) | Self::Ready { mtime, .. } => *mtime,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryKind {
    Dir,
    File,
    Symlink,
}

impl EntryKind {
    // Human-readable label for the Type column, from the framework's MIME label
    // table.
    pub(crate) fn label(self, mime: &str) -> String {
        match self {
            EntryKind::Dir => crate::i18n::tr("Folder").to_owned(),
            EntryKind::Symlink => crate::i18n::tr("Link").to_owned(),
            EntryKind::File => {
                if mime.is_empty() {
                    return crate::i18n::tr("File").to_owned();
                }
                enzui_widgets::file_browser::label_for_mime(mime)
            }
        }
    }
}

// Case-insensitive glob matching for the Select items matching dialog, with
// `*`, `?` and literal runs; no character classes.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let name: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    let mut pattern_index = 0;
    let mut name_index = 0;
    let mut star = None;
    let mut star_end = 0;

    while name_index < name.len() {
        match pattern.get(pattern_index) {
            Some('*') => {
                star = Some(pattern_index);
                star_end = name_index;
                pattern_index += 1;
            }
            Some(character) if *character == '?' || *character == name[name_index] => {
                pattern_index += 1;
                name_index += 1;
            }
            _ => {
                let Some(star_index) = star else {
                    return false;
                };
                star_end += 1;
                name_index = star_end;
                pattern_index = star_index + 1;
            }
        }
    }

    pattern[pattern_index..]
        .iter()
        .all(|character| *character == '*')
}
