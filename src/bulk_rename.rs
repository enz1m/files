// Preview and execution share one plan; any collision blocks the entire batch.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BulkRenameMode {
    // Substring find + replace across the whole file name.
    FindReplace,
    // A name template with one `[N]` counter and an `[Original Name]` token.
    Numbering,
}

impl BulkRenameMode {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            BulkRenameMode::FindReplace => "Find & Replace",
            BulkRenameMode::Numbering => "Numbering",
        }
    }
}

// The modes in radio display order.
//
// `enz_radio_group` selects by index, so this array is the only mapping from
// `on_select(index)` to a mode.
pub(crate) const BULK_RENAME_MODES: [BulkRenameMode; 2] =
    [BulkRenameMode::FindReplace, BulkRenameMode::Numbering];

// The mode labels, at the same indices as [`BULK_RENAME_MODES`].
pub(crate) const BULK_RENAME_MODE_LABELS: [&str; 2] =
    [BULK_RENAME_MODES[0].label(), BULK_RENAME_MODES[1].label()];

pub(crate) const PREVIEW_LABELS: [&str; 2] = ["Original:", "New:"];

pub(crate) const RENAME_TO_LABEL: &str = "Rename to:";

pub(crate) const DEFAULT_TEMPLATE: &str = "[Original Name] [N]";

// The template field's help subtitle, naming exactly the tokens
// [`expand_template`] accepts.
//
// Width tokens such as `[NN]` are not tokens (padding is the `pad` stepper), so
// the help does not advertise them.
pub(crate) const TEMPLATE_TOKENS_HELP: &str = "Tokens: [Original Name] and [N]";
// Default counter start; `0` is accepted and means zero.
pub(crate) const DEFAULT_START: u32 = 1;

pub(crate) const DEFAULT_STEP: u32 = 1;

pub(crate) const DEFAULT_PAD: u32 = 3;
// Padding wider than this is refused by the stepper; a name is not a bignum.
pub(crate) const MAX_PAD: u32 = 8;

// Why a proposed name is or is not going to be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenameStatus {
    // The rule produced the current name; skipped without blocking the batch.
    Unchanged,
    // Will be renamed.
    Ready,
    // The rule produced an empty name.
    Empty,
    // The proposed name contains a path separator or a NUL.
    Illegal,
    // Another target in this batch wants the same name.
    CollidesInBatch,
    // A non-target file in this directory already has this name.
    CollidesOnDisk,
}

impl RenameStatus {
    pub(crate) const fn will_rename(self) -> bool {
        matches!(self, RenameStatus::Ready)
    }

    //
    // [`RenameStatus::Unchanged`] does not block: renaming only the matching subset
    // of a selection is normal.
    pub(crate) const fn blocks(self) -> bool {
        matches!(
            self,
            RenameStatus::Empty
                | RenameStatus::Illegal
                | RenameStatus::CollidesInBatch
                | RenameStatus::CollidesOnDisk
        )
    }

    // The badge text shown beside a row, or `None` for a row that needs none.
    pub(crate) fn notice(self) -> Option<&'static str> {
        match self {
            RenameStatus::Unchanged => Some(crate::i18n::tr("Unchanged")),
            RenameStatus::Ready => None,
            RenameStatus::Empty => Some(crate::i18n::tr("Name would be empty")),
            RenameStatus::Illegal => Some(crate::i18n::tr("Illegal character")),
            RenameStatus::CollidesInBatch => Some(crate::i18n::tr("Two files want this name")),
            RenameStatus::CollidesOnDisk => Some(crate::i18n::tr("A file already has this name")),
        }
    }
}

// One previewed rename.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenameRow {
    // The file as it is now.
    pub(crate) original: PathBuf,
    // `original`'s file name, for the `Original:` line.
    pub(crate) original_name: String,
    // The proposed file name, for the `New:` line.
    pub(crate) new_name: String,
    pub(crate) status: RenameStatus,
}

impl RenameRow {
    pub(crate) fn new_path(&self) -> PathBuf {
        match self.original.parent() {
            Some(parent) => parent.join(&self.new_name),
            None => PathBuf::from(&self.new_name),
        }
    }
}

// The complete plan: one row per target, in target order, used by both preview
// and executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenamePlan {
    pub(crate) rows: Vec<RenameRow>,
}

impl RenamePlan {
    // The `(from, to)` pairs to perform, in plan order; blocked and unchanged rows
    // are absent.
    pub(crate) fn operations(&self) -> Vec<(PathBuf, PathBuf)> {
        self.rows
            .iter()
            .filter(|r| r.status.will_rename())
            .map(|r| (r.original.clone(), r.new_path()))
            .collect()
    }

    pub(crate) fn rename_count(&self) -> usize {
        self.rows.iter().filter(|r| r.status.will_rename()).count()
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.rows.iter().any(|r| r.status.blocks())
    }
}

// The bulk-rename dialog's state.
#[derive(Clone, Debug)]
pub(crate) struct BulkRename {
    // Snapshot of the targets, taken when the dialog opened, in display order.
    pub(crate) targets: Vec<PathBuf>,
    // Non-target file names already in the directory, snapshotted with `targets`
    // so [`plan`](Self::plan) stays pure.
    // Every other entry in the targets' own folders, as full paths: search
    // results can come from many folders, and a name only clashes with its
    // siblings.
    pub(crate) occupied: BTreeSet<PathBuf>,
    pub(crate) mode: BulkRenameMode,
    pub(crate) find: String,
    pub(crate) replace: String,
    pub(crate) template: String,
    pub(crate) start: u32,
    pub(crate) step: u32,
    pub(crate) pad: u32,
    // The `Rename to:` text, used only with exactly one target; pre-filled with
    // its current name.
    pub(crate) direct: String,
}

impl BulkRename {
    // Open on `targets`, with `occupied` naming everything else in the folder.
    pub(crate) fn new(targets: Vec<PathBuf>, occupied: BTreeSet<PathBuf>) -> Self {
        let direct = match targets.as_slice() {
            [one] => file_name_of(one),
            _ => String::new(),
        };
        Self {
            targets,
            occupied,
            mode: BulkRenameMode::FindReplace,
            find: String::new(),
            replace: String::new(),
            template: DEFAULT_TEMPLATE.to_owned(),
            start: DEFAULT_START,
            step: DEFAULT_STEP,
            pad: DEFAULT_PAD,
            direct,
        }
    }

    // Restores every rule field, and a single target's `Rename to:` text, to its
    // opening default. `targets` and `occupied` are unchanged.
    pub(crate) fn reset(&mut self) {
        self.mode = BulkRenameMode::FindReplace;
        self.find.clear();
        self.replace.clear();
        DEFAULT_TEMPLATE.clone_into(&mut self.template);
        self.start = DEFAULT_START;
        self.step = DEFAULT_STEP;
        self.pad = DEFAULT_PAD;
        self.direct = match self.targets.as_slice() {
            [one] => file_name_of(one),
            _ => String::new(),
        };
    }

    // Whether this is the one-file `Rename to:` dialog rather than the rule dialog.
    //
    // Both the view and `propose` read this, so they agree on which dialog is shown.
    pub(crate) fn is_single(&self) -> bool {
        self.targets.len() == 1
    }

    // The dialog title: `Rename` for one file, `Rename Multiple Files` for several.
    pub(crate) fn title(&self) -> &'static str {
        if self.is_single() {
            crate::i18n::tr("Rename")
        } else {
            crate::i18n::tr("Rename Multiple Files")
        }
    }

    // The index of the active mode in [`BULK_RENAME_MODES`].
    pub(crate) fn mode_index(&self) -> usize {
        BULK_RENAME_MODES
            .iter()
            .position(|m| *m == self.mode)
            .unwrap_or(0)
    }

    // The single planner: the preview renders this and apply performs it.
    pub(crate) fn plan(&self) -> RenamePlan {
        let proposed: Vec<String> = self
            .targets
            .iter()
            .enumerate()
            .map(|(i, path)| self.propose(i, path))
            .collect();

        // Only changed names can collide; an unchanged file would collide with
        // itself. Destinations are full paths, so the same new name in two
        // folders is no clash.
        let destination = |path: &Path, name: &str| path.with_file_name(name);
        let mut wanted: HashMap<PathBuf, usize> = HashMap::new();
        for (i, name) in proposed.iter().enumerate() {
            let current = file_name_of(&self.targets[i]);
            if *name != current {
                *wanted
                    .entry(destination(&self.targets[i], name))
                    .or_insert(0) += 1;
            }
        }

        let rows = self
            .targets
            .iter()
            .zip(proposed.iter())
            .map(|(path, new_name)| {
                let original_name = file_name_of(path);
                let status = if *new_name == original_name {
                    RenameStatus::Unchanged
                } else if new_name.is_empty() {
                    RenameStatus::Empty
                } else if new_name.contains('/') || new_name.contains('\0') {
                    RenameStatus::Illegal
                } else if wanted
                    .get(&destination(path, new_name))
                    .copied()
                    .unwrap_or(0)
                    > 1
                {
                    RenameStatus::CollidesInBatch
                } else if self.occupied.contains(&destination(path, new_name)) {
                    RenameStatus::CollidesOnDisk
                } else {
                    RenameStatus::Ready
                };
                RenameRow {
                    original: path.clone(),
                    original_name,
                    new_name: new_name.clone(),
                    status,
                }
            })
            .collect();

        RenamePlan { rows }
    }

    pub(crate) fn can_apply(&self) -> bool {
        let plan = self.plan();
        !plan.is_blocked() && plan.rename_count() > 0
    }

    pub(crate) fn summary(&self) -> String {
        let plan = self.plan();
        let n = plan.rename_count();
        let total = self.targets.len();
        if plan.is_blocked() {
            return crate::i18n::format(
                "files-total-selected-resolve-the-conflicts-below-26532d7d",
                &[("total", (total).to_string())],
            );
        }
        match n {
            0 => crate::i18n::format(
                "files-total-selected-no-name-would-change-102adb58",
                &[("total", (total).to_string())],
            ),
            1 => crate::i18n::format(
                "files-total-selected-1-will-be-renamed-5aa0f67f",
                &[("total", (total).to_string())],
            ),
            _ => crate::i18n::format(
                "files-total-selected-n-will-be-renamed-be97a3b0",
                &[("total", (total).to_string()), ("n", (n).to_string())],
            ),
        }
    }

    // Applies the active rule to target `i`, returning the proposed file name.
    //
    // A single target returns [`Self::direct`] trimmed, so an all-space name
    // classifies as [`RenameStatus::Empty`].
    fn propose(&self, i: usize, path: &Path) -> String {
        if self.is_single() {
            return self.direct.trim().to_owned();
        }
        if path.file_name().and_then(|name| name.to_str()).is_none() {
            // A rule over a name that is not UTF-8 would write replacement
            // characters in place of its bytes; such a file keeps its name.
            return file_name_of(path);
        }
        match self.mode {
            BulkRenameMode::FindReplace => {
                let name = file_name_of(path);
                if self.find.is_empty() {
                    return name;
                }
                name.replace(&self.find, &self.replace)
            }
            BulkRenameMode::Numbering => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
                // u64, so a large step cannot wrap into a duplicate.
                let counter = u64::from(self.start) + (i as u64) * u64::from(self.step);
                let mut name = expand_template(&self.template, &stem, counter, self.pad);
                if let Some(ext) = ext.filter(|e| !e.is_empty()) {
                    name.push('.');
                    name.push_str(&ext);
                }
                name
            }
        }
    }
}

// `path`'s file name, or the empty string.
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// Expands `[Original Name]` and `[N]` in `template`, case-insensitively.
//
// An unknown bracketed token is copied verbatim, so a typo shows in the preview.
pub(crate) fn expand_template(template: &str, stem: &str, counter: u64, pad: u32) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(template.len() + stem.len() + 8);
    let mut rest = template;
    while let Some(open) = rest.find('[') {
        out.push_str(rest.get(..open).expect("index follows a UTF-8 boundary"));
        let after = rest
            .get(open + 1..)
            .expect("index follows a UTF-8 boundary");
        let Some(close) = after.find(']') else {
            // Unterminated bracket: copy the remainder verbatim.
            out.push_str(rest.get(open..).expect("index follows a UTF-8 boundary"));
            return out;
        };
        let token = after.get(..close).expect("index follows a UTF-8 boundary");
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "original name" => out.push_str(stem),
            "n" => {
                let width = pad as usize;
                let _ = write!(out, "{counter:0width$}");
            }
            _ => {
                // Unknown token — verbatim, brackets included.
                out.push('[');
                out.push_str(token);
                out.push(']');
            }
        }
        rest = after
            .get(close + 1..)
            .expect("index follows a UTF-8 boundary");
    }
    out.push_str(rest);
    out
}
