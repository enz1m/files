// Properties dialog snapshot.

use super::*;

// Snapshot of the Properties dialog target, with display strings
// pre-formatted and the nine permission bits as booleans the grid binds to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PropertiesInfo {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) kind_label: String,
    pub(crate) icon: String,
    pub(crate) size_bytes: u64,
    // Preformatted human size; `"\u{2014}"` (an em dash) for directories,
    // whose real size is the recursive walk in [`Self::recursive_size_text`].
    pub(crate) size_text: String,
    pub(crate) modified_text: String,
    pub(crate) accessed_text: String,
    pub(crate) owner: String,
    pub(crate) group: String,
    pub(crate) is_symlink: bool,
    pub(crate) link_target: String,
    // Direct-children summary for a directory ("X files, Y folders"); empty
    // for regular files.
    pub(crate) content_summary: String,
    pub(crate) mime_type: String,
    // Recursive size for directories, computed off the UI thread; empty (with
    // a spinner beside "Total Size") until the walk completes.
    pub(crate) recursive_size_text: String,
    // Nine permission bits: `r`/`w`/`x` for owner / group / other.
    pub(crate) owner_read: bool,
    pub(crate) owner_write: bool,
    pub(crate) owner_exec: bool,
    pub(crate) group_read: bool,
    pub(crate) group_write: bool,
    pub(crate) group_exec: bool,
    pub(crate) other_read: bool,
    pub(crate) other_write: bool,
    pub(crate) other_exec: bool,

    // The nine-bit mask last read from disk via [`Self::set_mode_bits`]. The
    // grid is dirty, enabling `Apply Permissions`, iff
    // `mode_bits() != baseline_mode`.
    pub(crate) baseline_mode: u32,
}

impl PropertiesInfo {
    pub(crate) fn recursive_size_pending(&self) -> bool {
        self.size_text == "\u{2014}" && self.recursive_size_text.is_empty()
    }

    // The nine permission bits as a `chmod(2)` mode mask, without setuid,
    // setgid or sticky, which are preserved separately on apply.
    pub(crate) fn mode_bits(&self) -> u32 {
        let mut m: u32 = 0;
        if self.owner_read {
            m |= 0o400;
        }
        if self.owner_write {
            m |= 0o200;
        }
        if self.owner_exec {
            m |= 0o100;
        }
        if self.group_read {
            m |= 0o040;
        }
        if self.group_write {
            m |= 0o020;
        }
        if self.group_exec {
            m |= 0o010;
        }
        if self.other_read {
            m |= 0o004;
        }
        if self.other_write {
            m |= 0o002;
        }
        if self.other_exec {
            m |= 0o001;
        }
        m
    }

    // Decodes a `stat(2)` mode into the nine bits and records it as the
    // dirty-check baseline. Every disk read goes through here, so `Apply`
    // goes inert after a successful apply.
    pub(crate) fn set_mode_bits(&mut self, mode: u32) {
        self.owner_read = mode & 0o400 != 0;
        self.owner_write = mode & 0o200 != 0;
        self.owner_exec = mode & 0o100 != 0;
        self.group_read = mode & 0o040 != 0;
        self.group_write = mode & 0o020 != 0;
        self.group_exec = mode & 0o010 != 0;
        self.other_read = mode & 0o004 != 0;
        self.other_write = mode & 0o002 != 0;
        self.other_exec = mode & 0o001 != 0;
        self.baseline_mode = mode & 0o777;
    }

    // Nautilus-style symbolic permissions string (`rwxr-xr--`), exactly nine
    // characters, `-` for a missing bit.
    pub(crate) fn symbolic_permissions(&self) -> String {
        let bit = |b: bool, c: char| if b { c } else { '-' };
        let mut s = String::with_capacity(9);
        s.push(bit(self.owner_read, 'r'));
        s.push(bit(self.owner_write, 'w'));
        s.push(bit(self.owner_exec, 'x'));
        s.push(bit(self.group_read, 'r'));
        s.push(bit(self.group_write, 'w'));
        s.push(bit(self.group_exec, 'x'));
        s.push(bit(self.other_read, 'r'));
        s.push(bit(self.other_write, 'w'));
        s.push(bit(self.other_exec, 'x'));
        s
    }

    // Octal mode string — three digits, e.g. `"755"`.
    pub(crate) fn octal_permissions(&self) -> String {
        format!("{:03o}", self.mode_bits())
    }

    // The bit at `(row, col)` where `row` 0/1/2 = owner/group/other and
    // `col` 0/1/2 = read/write/exec. `None` for out-of-range indices.
    pub(crate) fn get_bit(&self, row: usize, col: usize) -> Option<bool> {
        match (row, col) {
            (0, 0) => Some(self.owner_read),
            (0, 1) => Some(self.owner_write),
            (0, 2) => Some(self.owner_exec),
            (1, 0) => Some(self.group_read),
            (1, 1) => Some(self.group_write),
            (1, 2) => Some(self.group_exec),
            (2, 0) => Some(self.other_read),
            (2, 1) => Some(self.other_write),
            (2, 2) => Some(self.other_exec),
            _ => None,
        }
    }

    // Set the bit at `(row, col)`. Returns `true` iff the coordinates were in
    // range; out-of-range is a silent no-op.
    pub(crate) fn set_bit(&mut self, row: usize, col: usize, value: bool) -> bool {
        match (row, col) {
            (0, 0) => self.owner_read = value,
            (0, 1) => self.owner_write = value,
            (0, 2) => self.owner_exec = value,
            (1, 0) => self.group_read = value,
            (1, 1) => self.group_write = value,
            (1, 2) => self.group_exec = value,
            (2, 0) => self.other_read = value,
            (2, 1) => self.other_write = value,
            (2, 2) => self.other_exec = value,
            _ => return false,
        }
        true
    }
}
