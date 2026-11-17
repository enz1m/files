use super::*;
use std::fs;
use std::os::unix::fs::symlink;

// A fake `/sys/block` + `/dev/disk` tree.
struct FakeRoots {
    root: PathBuf,
}

impl FakeRoots {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "files-devices-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("devices")).unwrap();
        fs::create_dir_all(root.join("block")).unwrap();
        fs::create_dir_all(root.join("disk/by-uuid")).unwrap();
        fs::create_dir_all(root.join("disk/by-label")).unwrap();
        Self { root }
    }

    fn drive(&self, name: &str, bus: &str, removable: bool, sectors: u64, parts: &[(&str, u64)]) {
        let real = self.root.join("devices").join(bus).join(name);
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("removable"),
            if removable { "1\n" } else { "0\n" },
        )
        .unwrap();
        fs::write(real.join("size"), format!("{sectors}\n")).unwrap();
        for (part, part_sectors) in parts {
            let dir = real.join(part);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("partition"), "1\n").unwrap();
            fs::write(dir.join("size"), format!("{part_sectors}\n")).unwrap();
        }
        symlink(&real, self.root.join("block").join(name)).unwrap();
    }

    fn uuid(&self, dev: &str, uuid: &str) {
        symlink(
            format!("../../{dev}"),
            self.root.join("disk/by-uuid").join(uuid),
        )
        .unwrap();
    }

    fn label(&self, dev: &str, label: &str) {
        symlink(
            format!("../../{dev}"),
            self.root.join("disk/by-label").join(label),
        )
        .unwrap();
    }

    fn enumerate(&self, mounts: &str) -> Vec<RemovableDevice> {
        enumerate(&Roots {
            sys_block: &self.root.join("block"),
            dev_disk: &self.root.join("disk"),
            mounts,
        })
    }
}

impl Drop for FakeRoots {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> FakeRoots {
    let roots = FakeRoots::new("fixture");
    // Internal NVMe: never listed.
    roots.drive(
        "nvme0n1",
        "pci0000",
        false,
        1_000_000,
        &[("nvme0n1p1", 999_000)],
    );
    // A USB stick (not flagged removable, but hot-plugged) with two partitions.
    roots.drive(
        "sdb",
        "usb1",
        false,
        60_000_000,
        &[("sdb1", 40_000_000), ("sdb2", 20_000_000)],
    );
    roots.uuid("sdb1", "AAAA-1111");
    roots.label("sdb1", "My\\x20Stick");
    // An optical drive with a disc and no partition table.
    roots.drive("sr0", "ata1", true, 1_400_000, &[]);
    roots.label("sr0", "PHOTOS_2024");
    // A card reader with no card inserted.
    roots.drive("sdc", "usb2", true, 0, &[]);
    roots
}

#[test]
fn plans_unmount_every_mounted_partition_before_ejecting_or_powering_off() {
    let roots = fixture();
    let devices =
        roots.enumerate("/dev/sdb1 /media/a vfat rw 0 0\n/dev/sdb2 /media/b ext4 rw 0 0\n");
    let stick = &devices[0];
    let mounted = vec![PathBuf::from("/dev/sdb1"), PathBuf::from("/dev/sdb2")];
    let power_off = plan(DeviceOp::PowerOff, stick, &mounted);
    let rendered: Vec<String> = power_off
        .iter()
        .map(|s| {
            format!(
                "{} {}",
                s.program,
                s.args
                    .iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect();
    assert_eq!(
        rendered,
        vec![
            "udisksctl unmount -b /dev/sdb1",
            "udisksctl unmount -b /dev/sdb2",
            "udisksctl power-off -b /dev/sdb",
        ]
    );
    let eject = plan(DeviceOp::Eject, &devices[2], &[]);
    assert_eq!(eject.len(), 1);
    assert_eq!(eject[0].program, "eject");
    assert_eq!(eject[0].args, vec![OsString::from("/dev/sr0")]);
    assert_eq!(
        plan(DeviceOp::Mount, &devices[1], &[])[0].args,
        vec![
            OsString::from("mount"),
            OsString::from("-b"),
            OsString::from("/dev/sdb2")
        ]
    );
}
