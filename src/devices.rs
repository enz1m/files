// Enumeration reads kernel metadata; mutations use bounded helpers under desktop authorization.

pub(crate) mod phones;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

// One filesystem on a removable drive, or the drive itself when it carries
// no partition table (optical discs, superfloppies).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RemovableDevice {
    // Stable identity: the filesystem UUID when it has one, otherwise the
    // kernel device name.
    pub(crate) id: String,
    // The block device holding the filesystem, e.g. `/dev/sdb1`.
    pub(crate) device: PathBuf,
    // The whole drive, e.g. `/dev/sdb`, which Eject and Safely Remove act on.
    pub(crate) drive: PathBuf,
    // The filesystem label, or a size-based name when it has none.
    pub(crate) label: String,
    pub(crate) size: u64,
    // Where it is mounted now, if anywhere.
    pub(crate) mount: Option<PathBuf>,
    // The drive has ejectable media (an optical tray or a card reader).
    pub(crate) ejectable: bool,
    // The drive is hot-plugged (USB) and can be powered off.
    pub(crate) can_power_off: bool,
}

impl RemovableDevice {
    // The operations this device offers now, in presentation order.
    pub(crate) fn operations(&self) -> Vec<DeviceOp> {
        let mut ops = Vec::new();
        ops.push(if self.mount.is_some() {
            DeviceOp::Unmount
        } else {
            DeviceOp::Mount
        });
        if self.ejectable {
            ops.push(DeviceOp::Eject);
        }
        if self.can_power_off {
            ops.push(DeviceOp::PowerOff);
        }
        ops
    }
}

// What the user asked a removable device to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum DeviceOp {
    Mount,
    Unmount,
    Eject,
    PowerOff,
}

impl DeviceOp {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Mount => crate::i18n::tr("Mount"),
            Self::Unmount => crate::i18n::tr("Unmount"),
            Self::Eject => crate::i18n::tr("Eject"),
            Self::PowerOff => crate::i18n::tr("Safely Remove"),
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Mount => "hard-drive-download",
            Self::Unmount => "arrow-up-from-line",
            Self::Eject => "disc",
            Self::PowerOff => "power",
        }
    }

    // Eject and Safely Remove end the device's session for every partition on
    // the drive, so they are confirmed first.
    pub(crate) fn needs_confirmation(self) -> bool {
        matches!(self, Self::Eject | Self::PowerOff)
    }
}

// Why a device operation did not complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeviceError {
    HelperMissing(&'static str),
    // A file on the device is still open.
    Busy,
    // The mount service refused because files are open; the message names
    // the programs holding them.
    InUse(String),
    // The session's authorization policy refused, or the prompt was declined.
    NotAuthorized,
    Timeout,
    Cancelled,
    // Any other refusal, with one bounded helper line.
    Failed(String),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HelperMissing(program) => write!(f, "{program} is not installed"),
            Self::Busy => f.write_str(crate::i18n::tr(
                "the device is in use; close open files and try again",
            )),
            Self::NotAuthorized => f.write_str(crate::i18n::tr("not authorized")),
            Self::Timeout => f.write_str(crate::i18n::tr("the device did not respond in time")),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(line) | Self::InUse(line) => f.write_str(line),
        }
    }
}

// Classify a helper's rejection from its bounded stderr line.
pub(crate) fn classify_rejection(line: &str) -> DeviceError {
    let lower = line.to_ascii_lowercase();
    if lower.contains("target is busy")
        || lower.contains("device is busy")
        || lower.contains("in use")
    {
        DeviceError::Busy
    } else if lower.contains("not authorized")
        || lower.contains("notauthorized")
        || lower.contains("dismissed")
    {
        DeviceError::NotAuthorized
    } else {
        DeviceError::Failed(line.to_owned())
    }
}

impl From<crate::services::HelperFailure> for DeviceError {
    fn from(failure: crate::services::HelperFailure) -> Self {
        use crate::services::HelperFailure;
        use enzui_services::process_transport::ProcessErrorKind;
        match failure {
            HelperFailure::NotFound { program } => Self::HelperMissing(program),
            HelperFailure::Transport { ref error, .. } => match error.kind() {
                ProcessErrorKind::Deadline => Self::Timeout,
                ProcessErrorKind::Cancelled(_) => Self::Cancelled,
                _ => Self::Failed(failure.into_line()),
            },
            nonzero @ HelperFailure::NonZero { .. } => classify_rejection(&nonzero.into_line()),
        }
    }
}

// One helper invocation of a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Step {
    pub(crate) program: &'static str,
    pub(crate) args: Vec<OsString>,
}

fn step(program: &'static str, args: &[&str], device: &Path) -> Step {
    let mut argv: Vec<OsString> = args.iter().map(OsString::from).collect();
    argv.push(device.as_os_str().to_os_string());
    Step {
        program,
        args: argv,
    }
}

// The helper steps for `op` on `device`, run in order and stopped at the
// first failure. `mounted_on_drive` lists every mounted filesystem on the same
// drive, which Eject and Safely Remove unmount first.
pub(crate) fn plan(
    op: DeviceOp,
    device: &RemovableDevice,
    mounted_on_drive: &[PathBuf],
) -> Vec<Step> {
    let unmount_all = || {
        mounted_on_drive
            .iter()
            .map(|dev| step("udisksctl", &["unmount", "-b"], dev))
            .collect::<Vec<_>>()
    };
    match op {
        DeviceOp::Mount => vec![step("udisksctl", &["mount", "-b"], &device.device)],
        DeviceOp::Unmount => vec![step("udisksctl", &["unmount", "-b"], &device.device)],
        DeviceOp::Eject => {
            let mut steps = unmount_all();
            steps.push(step("eject", &[], &device.drive));
            steps
        }
        DeviceOp::PowerOff => {
            let mut steps = unmount_all();
            steps.push(step("udisksctl", &["power-off", "-b"], &device.drive));
            steps
        }
    }
}

// The mount point `udisksctl mount` reports: `Mounted /dev/sdb1 at /path.`
//
// The sentence's own period is one character; a volume labelled `backup.`
// mounts at `/run/media/u/backup.`, so only one is taken off, and where the
// version printed no period the path that exists wins.
pub(crate) fn parse_mounted_at(stdout: &str) -> Option<PathBuf> {
    let line = stdout.lines().find(|line| line.starts_with("Mounted "))?;
    let (_, at) = line.split_once(" at ")?;
    let at = at.trim();
    let without_period = at.strip_suffix('.').unwrap_or(at);
    let path = if !Path::new(without_period).is_dir() && Path::new(at).is_dir() {
        at
    } else {
        without_period
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

// Run `op` through the bounded helper runner. A successful Mount returns the
// new mount point.
pub(crate) fn perform(
    op: DeviceOp,
    device: &RemovableDevice,
    mounted_on_drive: &[PathBuf],
    service: &crate::services::UnmountService,
    cancel: &enzui_core::cancellation::CancelToken,
) -> Result<Option<PathBuf>, DeviceError> {
    let mut mounted_at = None;
    for step in plan(op, device, mounted_on_drive) {
        let output = service.run_helper(step.program, step.args, cancel)?;
        if op == DeviceOp::Mount {
            mounted_at = parse_mounted_at(&String::from_utf8_lossy(&output.stdout));
        }
    }
    Ok(mounted_at)
}

pub(crate) struct Roots<'a> {
    pub(crate) sys_block: &'a Path,
    pub(crate) dev_disk: &'a Path,
    pub(crate) mounts: &'a str,
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
}

// Device name → link name for every symlink in a `/dev/disk/by-*` directory.
fn links_by_target(dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if let (Ok(target), Some(name)) =
            (std::fs::read_link(entry.path()), entry.file_name().to_str())
            && let Some(dev) = target.file_name().and_then(|n| n.to_str())
        {
            out.insert(dev.to_owned(), name.to_owned());
        }
    }
    out
}

// Undo udev's `\xNN` escaping in a `by-label` link name.
fn unescape_label(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(hex) = raw.get(i + 2..i + 4)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Every removable filesystem under `roots`, drives in name order.
pub(crate) fn enumerate(roots: &Roots<'_>) -> Vec<RemovableDevice> {
    let uuids = links_by_target(&roots.dev_disk.join("by-uuid"));
    let labels = links_by_target(&roots.dev_disk.join("by-label"));
    let mounts: HashMap<String, PathBuf> = roots
        .mounts
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let source = cols.next()?;
            let target = cols.next()?;
            let dev = source.strip_prefix("/dev/")?;
            Some((
                dev.to_owned(),
                PathBuf::from(crate::services::decode_mount_field(target)),
            ))
        })
        .collect();

    let mut drives: Vec<String> = std::fs::read_dir(roots.sys_block)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| {
            !["loop", "ram", "zram", "dm-", "md", "nbd"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .collect();
    drives.sort();

    let mut out = Vec::new();
    for name in drives {
        let sys = roots.sys_block.join(&name);
        let removable = read_trimmed(&sys.join("removable")).as_deref() == Some("1");
        let usb = std::fs::read_link(&sys)
            .ok()
            .is_some_and(|link| link.to_string_lossy().contains("/usb"));
        let optical = name.starts_with("sr");
        if !(removable || usb || optical) {
            continue;
        }
        let mut filesystems: Vec<String> = std::fs::read_dir(&sys)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().join("partition").is_file())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect();
        filesystems.sort();
        let whole_disk = filesystems.is_empty();
        if whole_disk {
            filesystems.push(name.clone());
        }
        for fs in filesystems {
            let fs_sys = if whole_disk {
                sys.clone()
            } else {
                sys.join(&fs)
            };
            let size = read_trimmed(&fs_sys.join("size"))
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                .saturating_mul(512);
            if size == 0 {
                // An empty tray or a card reader with no card.
                continue;
            }
            let label = labels.get(&fs).map_or_else(
                || {
                    crate::i18n::format(
                        "files-volume-550d7167",
                        &[("arg0", (crate::services::format_bytes(size)))],
                    )
                },
                |raw| unescape_label(raw),
            );
            out.push(RemovableDevice {
                id: uuids
                    .get(&fs)
                    .map_or_else(|| format!("dev:{fs}"), |uuid| format!("uuid:{uuid}")),
                device: PathBuf::from("/dev").join(&fs),
                drive: PathBuf::from("/dev").join(&name),
                label,
                size,
                mount: mounts.get(&fs).cloned(),
                ejectable: optical || removable,
                can_power_off: usb,
            });
        }
    }
    out
}

pub(crate) fn enumerate_system() -> Vec<RemovableDevice> {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    enumerate(&Roots {
        sys_block: Path::new("/sys/block"),
        dev_disk: Path::new("/dev/disk"),
        mounts: &mounts,
    })
}

// The bounded owner removable-media enumeration and operations run on.
pub(crate) struct DeviceWork {
    owner: enzui_core::blocking_task::BlockingTaskOwner,
    generation: u64,
}

impl std::fmt::Debug for DeviceWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceWork").finish_non_exhaustive()
    }
}

// One worker: device operations on one session serialize naturally, and a
// short queue refuses a burst as `Busy` instead of stacking helpers.
const DEVICE_WORKERS: usize = 1;
const DEVICE_QUEUE: usize = 4;

impl DeviceWork {
    pub(crate) fn new() -> Self {
        use std::num::NonZeroUsize;
        let queue = NonZeroUsize::new(DEVICE_QUEUE).expect("nonzero");
        let owner = enzui_core::blocking_task::BlockingTaskOwner::new(
            "files-devices",
            NonZeroUsize::new(DEVICE_WORKERS).expect("nonzero"),
            queue,
        )
        .unwrap_or_else(|_| enzui_core::blocking_task::BlockingTaskOwner::unavailable(queue));
        Self {
            owner,
            generation: 0,
        }
    }

    fn next(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(crate) fn enumerate(
        &mut self,
    ) -> enzui_core::blocking_task::BlockingTask<Vec<RemovableDevice>> {
        let generation = self.next();
        self.owner.submit(generation, |_| enumerate_system())
    }

    pub(crate) fn perform(
        &mut self,
        op: DeviceOp,
        device: RemovableDevice,
        mounted_on_drive: Vec<PathBuf>,
    ) -> enzui_core::blocking_task::BlockingTask<Result<Option<PathBuf>, DeviceError>> {
        let generation = self.next();
        self.owner.submit(generation, move |_| {
            let service = crate::services::UnmountService::for_devices();
            let cancel = enzui_core::cancellation::CancelToken::new();
            perform(op, &device, &mounted_on_drive, &service, &cancel)
        })
    }
}

#[cfg(test)]
mod tests;
