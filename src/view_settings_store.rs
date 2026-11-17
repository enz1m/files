// Missing preference fields keep defaults; refused or newer bodies remain protected from ordinary saves.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use enzui_core::persist::{FileBackend, InMemoryBackend, Persistable, RawByteLimit, Store};

use crate::model::{Model, SortKey, ViewMode};

// Stable format identity written into the atomic-state header.
pub(crate) const FORMAT_ID: &str = "files-app-view-settings";
// Current schema version. v2 adds grid-info and per-folder rows; a v1 body
// loads with those at their defaults.
pub(crate) const VERSION: u32 = 2;
const BODY_V1: &str = "enzui-files-view-settings-v1";
const BODY_V2: &str = "enzui-files-view-settings-v2";

// Wire capacity includes fixed header slack plus every bounded per-folder record.
pub(crate) const HEADER_SLACK_BYTES: u64 = 4 * 1024;
pub(crate) const PER_FOLDER_ROW_BYTES: u64 = ("folder_sort".len() as u64)
    + 3
    + (MAX_FOLDER_PATH_BYTES as u64) * 2
    + (MAX_FIELD_BYTES as u64)
    + ("false".len() as u64)
    + 1;
// `folder_color\t<hex path>\t<dev>\t<ino>\t<secs|->\t<nanos|->\t<key>\n`.
pub(crate) const PER_COLOR_ROW_BYTES: u64 = ("folder_color".len() as u64)
    + 6
    + (MAX_FOLDER_PATH_BYTES as u64) * 2
    + 5 * (MAX_FIELD_BYTES as u64)
    + 1;
// `tab\t<hex path>\n`.
pub(crate) const PER_TAB_ROW_BYTES: u64 = 4 + (MAX_FOLDER_PATH_BYTES as u64) * 2 + 1;
pub(crate) const MAX_BODY_BYTES: u64 = HEADER_SLACK_BYTES
    + PER_TAB_ROW_BYTES * (crate::tabs::MAX_TABS as u64)
    + PER_FOLDER_ROW_BYTES * (MAX_FOLDER_ENTRIES as u64)
    + PER_COLOR_ROW_BYTES * (crate::folder_colors::MAX_FOLDER_COLORS as u64);

// Complete-frame byte limit for every load and save through the production
// [`Store`], so serialization and load refuse the same representation.
// [`RawByteLimit::for_body`] rejects an oversize `MAX_BODY_BYTES` at compile
// time.
pub(crate) const RAW_BYTE_LIMIT: RawByteLimit =
    match RawByteLimit::for_body(FORMAT_ID, VERSION, MAX_BODY_BYTES) {
        Ok(limit) => limit,
        Err(_) => panic!("view-settings body ceiling exceeds framework maximum"),
    };

// Semantic bound on per-folder overrides, enforced on serialize and on
// deserialize, where the frame cap alone would admit many short paths.
pub(crate) const MAX_FOLDER_ENTRIES: usize = 512;

// Byte cap for one folder path. Matches Linux `PATH_MAX`; the hex encoding
// on the wire is 2 * this many bytes.
pub(crate) const MAX_FOLDER_PATH_BYTES: usize = 4096;

// Byte cap for one `key=value` field value, so a hostile document cannot
// allocate a large `String` behind a benign field.
pub(crate) const MAX_FIELD_BYTES: usize = 64;

// Aggregate retained-byte cap: sum of retained path bytes plus the fixed
// key/bool costs, refused before the map insertion that would carry them.
pub(crate) const MAX_RETAINED_STATE_BYTES: usize =
    MAX_FOLDER_ENTRIES * (MAX_FOLDER_PATH_BYTES + 16) + 4 * 1024;

// A cheap `PartialEq` token for everything the record holds. The final field
// is the [`Tracked`](enzui_core::persist::Tracked) revision of
// [`Model::sort_overrides`], so per-folder edits change it in O(1).
pub(crate) type ViewSettingsToken = (
    ViewMode,
    SortKey,
    bool,
    bool,
    bool,
    bool,
    bool,
    [bool; 3],
    [bool; 3],
    u64,
    // Zoom levels, folder-colour revision and tab session hash.
    (usize, usize, u64, u64),
    String,
);

// Every durable choice exposed by View Settings plus per-folder sort memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ViewSettings {
    pub(crate) view_mode: ViewMode,
    pub(crate) default_sort_key: SortKey,
    pub(crate) default_sort_ascending: bool,
    pub(crate) sort_folders_first: bool,
    pub(crate) show_hidden: bool,
    pub(crate) colored_file_icons: bool,
    // Zoom level indices; persisted as their factors, so a ladder
    // change restores the nearest level instead of a shifted one.
    pub(crate) list_zoom: usize,
    pub(crate) grid_zoom: usize,
    // The Open in Terminal command; empty means automatic.
    pub(crate) terminal_command: String,
    // Folder colour tags.
    pub(crate) folder_colors: crate::folder_colors::FolderColors,
    // The tab locations in strip order, and the active one.
    pub(crate) tabs: Vec<PathBuf>,
    pub(crate) active_tab: usize,
    pub(crate) single_click_open: bool,
    pub(crate) visible_columns: [bool; 3],
    pub(crate) grid_info: [bool; 3],
    pub(crate) sort_overrides: BTreeMap<PathBuf, (SortKey, bool)>,
}

impl Default for ViewSettings {
    // The same defaults `Model::new` seeds.
    fn default() -> Self {
        Self {
            view_mode: ViewMode::List,
            default_sort_key: SortKey::Name,
            default_sort_ascending: true,
            sort_folders_first: true,
            show_hidden: false,
            colored_file_icons: true,
            list_zoom: crate::model::list_zoom_ladder().default_index(),
            grid_zoom: crate::model::grid_zoom_ladder().default_index(),
            terminal_command: String::new(),
            folder_colors: crate::folder_colors::FolderColors::default(),
            tabs: Vec::new(),
            active_tab: 0,
            single_click_open: false,
            visible_columns: [true; 3],
            grid_info: [false; 3],
            sort_overrides: BTreeMap::new(),
        }
    }
}

impl ViewSettings {
    // `loaded`, with every field the user changed since `boot` (as `current`
    // holds it) kept instead: a zoom or Ctrl+H during the load must not throw
    // away the saved folder colours, sorts and terminal command.
    pub(crate) fn keep_edits(loaded: &Self, boot: &Self, current: &Self) -> Self {
        let mut merged = loaded.clone();
        macro_rules! keep {
            ($($field:ident),* $(,)?) => {
                $(if current.$field != boot.$field {
                    merged.$field = current.$field.clone();
                })*
            };
        }
        keep!(
            view_mode,
            default_sort_key,
            default_sort_ascending,
            sort_folders_first,
            show_hidden,
            colored_file_icons,
            list_zoom,
            grid_zoom,
            terminal_command,
            folder_colors,
            single_click_open,
            visible_columns,
            grid_info,
            sort_overrides,
        );
        merged
    }

    // Materialises the record from the live model; the
    // [`SaveGate`](enzui_core::persist::SaveGate) runs it only on change.
    pub(crate) fn snapshot(model: &Model) -> Self {
        Self {
            view_mode: model.default_view_mode,
            default_sort_key: model.default_sort_key,
            default_sort_ascending: model.default_sort_ascending,
            sort_folders_first: model.sort_folders_first,
            show_hidden: model.show_hidden,
            colored_file_icons: model.colored_file_icons,
            list_zoom: model.list_zoom,
            grid_zoom: model.grid_zoom,
            terminal_command: model.terminal_command.clone(),
            folder_colors: model.folder_colors.get().clone(),
            tabs: model.session().0,
            active_tab: model.session().1,
            single_click_open: model.single_click_open,
            visible_columns: model.visible_columns,
            grid_info: model.grid_info,
            sort_overrides: model.sort_overrides.get().clone(),
        }
    }

    // [`Self::token`] without the tab session, for the boot-load baseline:
    // browsing before the saved settings arrive must not discard them.
    pub(crate) fn baseline_token(model: &Model) -> ViewSettingsToken {
        let mut token = Self::token(model);
        token.10.3 = 0;
        token
    }

    pub(crate) fn token(model: &Model) -> ViewSettingsToken {
        (
            model.view_mode,
            model.default_sort_key,
            model.default_sort_ascending,
            model.sort_folders_first,
            model.show_hidden,
            model.colored_file_icons,
            model.single_click_open,
            model.visible_columns,
            model.grid_info,
            model.sort_overrides.revision(),
            (
                model.list_zoom,
                model.grid_zoom,
                model.folder_colors.revision(),
                session_hash(model),
            ),
            model.terminal_command.clone(),
        )
    }
}

impl Persistable for ViewSettings {
    fn persist_serialize(&self) -> Result<String, String> {
        use std::fmt::Write as _;

        // Refuse a state that would exceed the frame or aggregate cap.
        if self.sort_overrides.len() > MAX_FOLDER_ENTRIES {
            return Err(crate::i18n::format(
                "files-view-settings-hold-folder-overrides-cap-ce756cb6",
                &[
                    ("arg0", (self.sort_overrides.len()).to_string()),
                    ("MAX_FOLDER_ENTRIES", (MAX_FOLDER_ENTRIES).to_string()),
                ],
            ));
        }
        let mut retained: usize = 0;
        for path in self.sort_overrides.keys() {
            let bytes = path.as_os_str().as_bytes().len();
            if bytes > MAX_FOLDER_PATH_BYTES {
                return Err(crate::i18n::format(
                    "files-folder-override-path-is-bytes-bytes-23ccaf94",
                    &[
                        ("bytes", (bytes).to_string()),
                        ("MAX_FOLDER_PATH_BYTES", (MAX_FOLDER_PATH_BYTES).to_string()),
                    ],
                ));
            }
            retained = retained.saturating_add(bytes).saturating_add(16);
            if retained > MAX_RETAINED_STATE_BYTES {
                return Err(crate::i18n::format(
                    "files-folder-overrides-retain-max-retained-state-01e925f4",
                    &[(
                        "MAX_RETAINED_STATE_BYTES",
                        (MAX_RETAINED_STATE_BYTES).to_string(),
                    )],
                ));
            }
        }

        if !is_terminal_command(&self.terminal_command) {
            return Err(crate::i18n::tr(
                "terminal command holds a control character or exceeds the field cap",
            )
            .into());
        }
        let mut body = format!(
            "{BODY_V2}\n\
             view_mode={}\n\
             default_sort_key={}\n\
             default_sort_ascending={}\n\
             sort_folders_first={}\n\
             show_hidden={}\n\
             colored_file_icons={}\n\
             single_click_open={}\n\
             visible_columns={}\n\
             grid_info={}\n\
             list_zoom={}\n\
             grid_zoom={}\n\
             terminal_command={}\n",
            view_mode_name(self.view_mode),
            sort_key_name(self.default_sort_key),
            bool_name(self.default_sort_ascending),
            bool_name(self.sort_folders_first),
            bool_name(self.show_hidden),
            bool_name(self.colored_file_icons),
            bool_name(self.single_click_open),
            bool_array(self.visible_columns),
            bool_array(self.grid_info),
            crate::model::list_zoom_ladder().factor(self.list_zoom),
            crate::model::grid_zoom_ladder().factor(self.grid_zoom),
            self.terminal_command,
        );
        if self.tabs.len() > crate::tabs::MAX_TABS {
            return Err(crate::i18n::format(
                "files-view-settings-hold-tabs-cap-is-cf45bbdb",
                &[
                    ("arg0", (self.tabs.len()).to_string()),
                    ("arg1", (crate::tabs::MAX_TABS).to_string()),
                ],
            ));
        }
        let _ = writeln!(body, "active_tab={}", self.active_tab);
        for tab in &self.tabs {
            let bytes = tab.as_os_str().as_bytes().len();
            if bytes > MAX_FOLDER_PATH_BYTES {
                return Err(crate::i18n::format(
                    "files-tab-path-is-bytes-bytes-cap-239cd91c",
                    &[
                        ("bytes", (bytes).to_string()),
                        ("MAX_FOLDER_PATH_BYTES", (MAX_FOLDER_PATH_BYTES).to_string()),
                    ],
                ));
            }
            body.push_str("tab\t");
            body.push_str(&hex(tab.as_os_str().as_bytes()));
            body.push('\n');
        }
        for record in self.folder_colors.records() {
            let bytes = record.path.as_os_str().as_bytes().len();
            if bytes > MAX_FOLDER_PATH_BYTES {
                return Err(crate::i18n::format(
                    "files-folder-colour-path-is-bytes-bytes-00012e57",
                    &[
                        ("bytes", (bytes).to_string()),
                        ("MAX_FOLDER_PATH_BYTES", (MAX_FOLDER_PATH_BYTES).to_string()),
                    ],
                ));
            }
            let (secs, nanos) = match record.identity.birth {
                Some((secs, nanos)) => (secs.to_string(), nanos.to_string()),
                None => ("-".to_owned(), "-".to_owned()),
            };
            let _ = writeln!(
                body,
                "folder_color\t{}\t{}\t{}\t{secs}\t{nanos}\t{}",
                hex(record.path.as_os_str().as_bytes()),
                record.identity.dev,
                record.identity.ino,
                record.color.id_str(),
            );
        }
        for (path, (key, ascending)) in &self.sort_overrides {
            // Hex keeps tabs, newlines and non-UTF-8 bytes unambiguous.
            body.push_str("folder_sort\t");
            body.push_str(&hex(path.as_os_str().as_bytes()));
            body.push('\t');
            body.push_str(sort_key_name(*key));
            body.push('\t');
            body.push_str(bool_name(*ascending));
            body.push('\n');
        }
        if body.len() > RAW_BYTE_LIMIT.bytes() as usize {
            return Err(crate::i18n::format(
                "files-serialized-view-settings-body-is-bytes-bf5d3def",
                &[
                    ("arg0", (body.len()).to_string()),
                    ("arg1", (RAW_BYTE_LIMIT.bytes()).to_string()),
                ],
            ));
        }
        Ok(body)
    }

    fn persist_deserialize(body: &str) -> Result<Self, String> {
        // Callers outside the framed load path get the same bound.
        if body.len() > RAW_BYTE_LIMIT.bytes() as usize {
            return Err(crate::i18n::format(
                "files-view-settings-body-is-bytes-frame-49f0e66d",
                &[
                    ("arg0", (body.len()).to_string()),
                    ("arg1", (RAW_BYTE_LIMIT.bytes()).to_string()),
                ],
            ));
        }
        let mut lines = body.lines();
        let schema = lines
            .next()
            .ok_or_else(|| crate::i18n::tr("missing view-settings schema line").to_owned())?;
        if !matches!(schema, BODY_V1 | BODY_V2) {
            return Err(crate::i18n::format(
                "files-unsupported-view-settings-schema-schema-e5bf899e",
                &[("schema", format!("{schema:?}"))],
            ));
        }

        // Fields absent from the body keep their defaults.
        let mut value = Self::default();
        let mut folder_count: usize = 0;
        let mut colors: Vec<crate::folder_colors::FolderColor> = Vec::new();
        let mut retained_bytes: usize = 0;
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if let Some(raw) = line.strip_prefix("tab\t") {
                if value.tabs.len() >= crate::tabs::MAX_TABS {
                    return Err(crate::i18n::tr("view-settings body has too many tabs").into());
                }
                if raw.len() > MAX_FOLDER_PATH_BYTES.saturating_mul(2) {
                    return Err(crate::i18n::tr("tab path beyond cap").into());
                }
                value
                    .tabs
                    .push(PathBuf::from(OsString::from_vec(unhex_bytes(raw)?)));
                continue;
            }
            if let Some(raw) = line.strip_prefix("folder_color\t") {
                if colors.len() >= crate::folder_colors::MAX_FOLDER_COLORS {
                    return Err(
                        crate::i18n::tr("view-settings body has too many folder colours").into(),
                    );
                }
                colors.push(parse_folder_color(raw)?);
                continue;
            }
            if let Some(raw) = line.strip_prefix("folder_sort\t") {
                if folder_count >= MAX_FOLDER_ENTRIES {
                    return Err(crate::i18n::format(
                        "files-view-settings-body-has-max-folder-3567872b",
                        &[("MAX_FOLDER_ENTRIES", (MAX_FOLDER_ENTRIES).to_string())],
                    ));
                }
                folder_count = folder_count.saturating_add(1);
                let mut fields = raw.split('\t');
                let path = fields
                    .next()
                    .ok_or_else(|| crate::i18n::tr("folder_sort path missing"))?;
                let key = fields
                    .next()
                    .ok_or_else(|| crate::i18n::tr("folder_sort key missing"))?;
                let ascending = fields
                    .next()
                    .ok_or_else(|| crate::i18n::tr("folder_sort direction missing"))?;
                if fields.next().is_some() {
                    return Err(crate::i18n::tr("folder_sort row has extra fields").into());
                }
                // Checked on hex length so decoding never allocates an over-cap
                // `Vec`.
                if path.len() > MAX_FOLDER_PATH_BYTES.saturating_mul(2) {
                    return Err(crate::i18n::format(
                        "files-folder-sort-path-hex-length-cap-026f68d0",
                        &[
                            ("arg0", (path.len()).to_string()),
                            (
                                "arg1",
                                (MAX_FOLDER_PATH_BYTES.saturating_mul(2)).to_string(),
                            ),
                        ],
                    ));
                }
                if key.len() > MAX_FIELD_BYTES || ascending.len() > MAX_FIELD_BYTES {
                    return Err(crate::i18n::tr("folder_sort field value beyond cap").into());
                }
                let decoded_bytes = unhex_bytes(path)?;
                if decoded_bytes.len() > MAX_FOLDER_PATH_BYTES {
                    return Err(crate::i18n::format(
                        "files-folder-sort-path-is-bytes-cap-4f3027ba",
                        &[
                            ("arg0", (decoded_bytes.len()).to_string()),
                            ("MAX_FOLDER_PATH_BYTES", (MAX_FOLDER_PATH_BYTES).to_string()),
                        ],
                    ));
                }
                retained_bytes = retained_bytes
                    .saturating_add(decoded_bytes.len())
                    .saturating_add(16);
                if retained_bytes > MAX_RETAINED_STATE_BYTES {
                    return Err(crate::i18n::format(
                        "files-view-settings-retained-bytes-max-retained-d4b9dc99",
                        &[(
                            "MAX_RETAINED_STATE_BYTES",
                            (MAX_RETAINED_STATE_BYTES).to_string(),
                        )],
                    ));
                }
                value.sort_overrides.insert(
                    PathBuf::from(OsString::from_vec(decoded_bytes)),
                    (parse_sort_key(key)?, parse_bool(ascending)?),
                );
                continue;
            }
            let Some((name, raw)) = line.split_once('=') else {
                return Err(crate::i18n::format(
                    "files-malformed-view-settings-line-line-cb8bed9a",
                    &[("line", format!("{line:?}"))],
                ));
            };
            if raw.len() > MAX_FIELD_BYTES {
                return Err(crate::i18n::format(
                    "files-view-settings-field-name-value-is-b38a1903",
                    &[
                        ("name", format!("{name:?}")),
                        ("arg0", (raw.len()).to_string()),
                        ("MAX_FIELD_BYTES", (MAX_FIELD_BYTES).to_string()),
                    ],
                ));
            }
            match name {
                "view_mode" => value.view_mode = parse_view_mode(raw)?,
                "default_sort_key" => value.default_sort_key = parse_sort_key(raw)?,
                "default_sort_ascending" => value.default_sort_ascending = parse_bool(raw)?,
                "sort_folders_first" => value.sort_folders_first = parse_bool(raw)?,
                "show_hidden" => value.show_hidden = parse_bool(raw)?,
                "colored_file_icons" => value.colored_file_icons = parse_bool(raw)?,
                "single_click_open" => value.single_click_open = parse_bool(raw)?,
                "visible_columns" => value.visible_columns = parse_bool_array(raw)?,
                "grid_info" => value.grid_info = parse_bool_array(raw)?,
                "list_zoom" => {
                    value.list_zoom = crate::model::list_zoom_ladder().nearest(parse_factor(raw)?);
                }
                "active_tab" => {
                    value.active_tab = raw.parse().map_err(|_| {
                        crate::i18n::format(
                            "files-invalid-active-tab-raw-43be86ba",
                            &[("raw", format!("{raw:?}"))],
                        )
                    })?;
                }
                "terminal_command" if is_terminal_command(raw) => {
                    raw.clone_into(&mut value.terminal_command);
                }
                "terminal_command" => {
                    return Err(crate::i18n::tr("invalid terminal command").into());
                }
                "grid_zoom" => {
                    value.grid_zoom = crate::model::grid_zoom_ladder().nearest(parse_factor(raw)?);
                }
                // Unknown fields from a newer writer are ignored.
                _ => {}
            }
        }
        value.folder_colors = crate::folder_colors::FolderColors::from_records(colors);
        Ok(value)
    }

    fn persist_default() -> Self {
        Self::default()
    }
}

// Builds the store: a [`FileBackend`] with [`RAW_BYTE_LIMIT`] for a path, or
// an [`InMemoryBackend`] for `None`.
pub(crate) fn store(path: Option<&Path>) -> Store<ViewSettings> {
    match path {
        Some(path) => Store::new(FileBackend::new(path, RAW_BYTE_LIMIT), FORMAT_ID, VERSION),
        None => Store::new(InMemoryBackend::new(), FORMAT_ID, VERSION),
    }
}

fn bool_name(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn session_hash(model: &Model) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    model.session().hash(&mut hasher);
    hasher.finish().max(1)
}

// One `folder_color` row after its tag.
fn parse_folder_color(raw: &str) -> Result<crate::folder_colors::FolderColor, String> {
    let fields: Vec<&str> = raw.split('\t').collect();
    let [path, dev, ino, secs, nanos, key] = fields.as_slice() else {
        return Err(crate::i18n::tr("folder_color row has the wrong number of fields").into());
    };
    if path.len() > MAX_FOLDER_PATH_BYTES.saturating_mul(2)
        || [dev, ino, secs, nanos, key]
            .iter()
            .any(|field| field.len() > MAX_FIELD_BYTES)
    {
        return Err(crate::i18n::tr("folder_color field beyond cap").into());
    }
    let number = |raw: &str| {
        raw.parse::<u64>().map_err(|_| {
            crate::i18n::format(
                "files-invalid-folder-color-number-raw-1dcd9c8e",
                &[("raw", format!("{raw:?}"))],
            )
        })
    };
    let birth = match (*secs, *nanos) {
        ("-", "-") => None,
        (secs, nanos) => Some((
            secs.parse::<i64>().map_err(|_| {
                crate::i18n::format(
                    "files-invalid-folder-color-birth-secs-b2bcca04",
                    &[("secs", format!("{secs:?}"))],
                )
            })?,
            nanos
                .parse::<u32>()
                .ok()
                .filter(|n| *n < 1_000_000_000)
                .ok_or_else(|| {
                    crate::i18n::format(
                        "files-invalid-folder-color-birth-nanos-c43bac8d",
                        &[("nanos", format!("{nanos:?}"))],
                    )
                })?,
        )),
    };
    Ok(crate::folder_colors::FolderColor {
        path: PathBuf::from(OsString::from_vec(unhex_bytes(path)?)),
        identity: crate::folder_colors::FolderIdentity {
            dev: number(dev)?,
            ino: number(ino)?,
            birth,
        },
        color: enzui_theme::WorkspaceAccentKey::from_id_str(key).ok_or_else(|| {
            crate::i18n::format(
                "files-unknown-folder-colour-key-985d6478",
                &[("key", format!("{key:?}"))],
            )
        })?,
    })
}

// A terminal command fits one field line: within the field cap and
// free of control characters (a newline would forge the next field).
pub(crate) fn is_terminal_command(raw: &str) -> bool {
    raw.len() <= MAX_FIELD_BYTES && !raw.chars().any(char::is_control)
}

fn parse_factor(raw: &str) -> Result<f32, String> {
    raw.parse::<f32>()
        .ok()
        .filter(|factor| factor.is_finite())
        .ok_or_else(|| {
            crate::i18n::format(
                "files-invalid-zoom-factor-raw-f5598520",
                &[("raw", format!("{raw:?}"))],
            )
        })
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(crate::i18n::format(
            "files-invalid-boolean-raw-36cba00f",
            &[("raw", format!("{raw:?}"))],
        )),
    }
}

fn bool_array(value: [bool; 3]) -> String {
    value
        .into_iter()
        .map(|v| if v { '1' } else { '0' })
        .collect()
}

fn parse_bool_array(raw: &str) -> Result<[bool; 3], String> {
    let bytes = raw.as_bytes();
    if bytes.len() != 3 || bytes.iter().any(|byte| !matches!(byte, b'0' | b'1')) {
        return Err(crate::i18n::format(
            "files-invalid-three-boolean-array-raw-7178b7bc",
            &[("raw", format!("{raw:?}"))],
        ));
    }
    Ok([bytes[0] == b'1', bytes[1] == b'1', bytes[2] == b'1'])
}

fn view_mode_name(value: ViewMode) -> &'static str {
    match value {
        ViewMode::List => "list",
        ViewMode::Grid => "grid",
    }
}

fn parse_view_mode(raw: &str) -> Result<ViewMode, String> {
    match raw {
        "list" => Ok(ViewMode::List),
        "grid" => Ok(ViewMode::Grid),
        _ => Err(crate::i18n::format(
            "files-invalid-view-mode-raw-672c41d7",
            &[("raw", format!("{raw:?}"))],
        )),
    }
}

fn sort_key_name(value: SortKey) -> &'static str {
    match value {
        SortKey::Name => "name",
        SortKey::Size => "size",
        SortKey::Modified => "modified",
        SortKey::Kind => "kind",
    }
}

fn parse_sort_key(raw: &str) -> Result<SortKey, String> {
    match raw {
        "name" => Ok(SortKey::Name),
        "size" => Ok(SortKey::Size),
        "modified" => Ok(SortKey::Modified),
        "kind" => Ok(SortKey::Kind),
        _ => Err(crate::i18n::format(
            "files-invalid-sort-key-raw-bfa08871",
            &[("raw", format!("{raw:?}"))],
        )),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex_bytes(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err(crate::i18n::tr("odd-length folder path encoding").into());
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|digits| u8::from_str_radix(digits, 16).ok())
                .ok_or_else(|| crate::i18n::tr("invalid folder path encoding").to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests;
