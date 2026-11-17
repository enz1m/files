use super::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) const PACKAGE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub(crate) manifest: Manifest,
    pub(crate) files: BTreeMap<String, Vec<u8>>,
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(crate::i18n::tr("Extension data must be regular files").into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(crate::i18n::tr("Extension data exceeds its size limit").into());
    }
    Ok(bytes)
}

impl Prepared {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let files = enzui_services::extension_package::package::read(
            path,
            &manifest::host(),
            &enzui_services::extension_package::package::PackageLimits::default(),
        )?;
        let out = Self {
            manifest: Manifest::parse(files.get("manifest.json").ok_or("Missing manifest.json")?)?,
            files,
        };
        if let Worker::Native { program, .. } = &out.manifest.worker
            && !out.files.contains_key(program)
        {
            return Err(crate::i18n::tr("The extension worker is missing").into());
        }
        Ok(out)
    }
}

pub(crate) use enzui_services::extension_package::State;

fn state(path: &Path) -> Result<State, String> {
    let bytes = read_file(path, 65536)?;
    serde_json::from_slice(&bytes).map_err(|_| crate::i18n::tr("Invalid extension settings").into())
}
fn private_dir(path: &Path) -> Result<(), String> {
    enzui_core::secure_dir::ensure_private_dir(path)
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn owned_dir(path: &Path) -> Result<(), String> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !m.is_dir() || m.file_type().is_symlink() || m.permissions().mode() & 0o077 != 0 {
        return Err(crate::i18n::tr("The extension directory is not private").into());
    }
    Ok(())
}

pub(crate) fn verify(root: &Path, state: &State) -> Result<(), String> {
    enzui_services::extension_package::verify(root, state).map_err(|error| error.to_string())
}

pub(crate) fn load(root: PathBuf) -> Catalog {
    let mut catalog = Catalog {
        root: Some(root.clone()),
        ..Catalog::default()
    };
    let builtin = Manifest::parse(include_bytes!("../../extensions/bundled/checksums.json"))
        .expect("valid bundled manifest");
    let builtin_root = root.join(&builtin.id);
    let mut saved = State {
        enabled: true,
        ..State::default()
    };
    let mut problem = None;
    if builtin_root.exists() {
        match owned_dir(&builtin_root).and_then(|()| state(&builtin_root.join("state.json"))) {
            Ok(value) => saved = value,
            Err(error) => {
                saved.enabled = false;
                problem = Some(error);
            }
        }
    }
    if !saved.removed {
        catalog.entries.push(Installed {
            id: builtin.id.clone(),
            manifest: Some(builtin),
            state: saved,
            bundled: true,
            problem,
        });
    }
    if !root.exists() {
        return catalog;
    }
    if let Err(error) = owned_dir(&root) {
        catalog.error = Some(error);
        return catalog;
    }
    let items = match fs::read_dir(&root) {
        Ok(items) => items,
        Err(e) => {
            catalog.error = Some(e.to_string());
            return catalog;
        }
    };
    for item in items.take(512) {
        let Ok(item) = item else {
            catalog.error = Some(crate::i18n::tr("Could not read extension store").into());
            break;
        };
        let Some(id) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if id.starts_with('.') || id == "org.enzui.checksums" {
            continue;
        }
        if !manifest::identifier(&id) {
            continue;
        }
        if catalog.entries.len() >= 64 {
            catalog.error =
                Some(crate::i18n::tr("The extension store has too many entries").into());
            break;
        }
        let mut installed = Installed {
            id: id.clone(),
            manifest: None,
            state: State::default(),
            bundled: false,
            problem: None,
        };
        let result = (|| {
            owned_dir(&item.path())?;
            let manifest = Manifest::parse(&read_file(&item.path().join("manifest.json"), 65536)?)?;
            if manifest.id != id {
                return Err(crate::i18n::tr("Extension ID does not match its directory").into());
            }
            installed.manifest = Some(manifest);
            installed.state = state(&item.path().join("state.json"))?;
            verify(&item.path(), &installed.state)?;
            let manifest = installed
                .manifest
                .as_ref()
                .ok_or_else(|| crate::i18n::tr("Missing manifest"))?;
            for (key, value) in &installed.state.settings {
                if !manifest
                    .settings
                    .iter()
                    .any(|s| s.id == *key && s.accepts(value))
                {
                    return Err(crate::i18n::tr("Invalid saved extension settings").into());
                }
            }
            if let Some(error) = manifest.compatibility() {
                return Err(error);
            }
            Ok::<(), String>(())
        })();
        installed.problem = result.err();
        catalog.entries.push(installed);
    }
    catalog.entries.sort_by(|a, b| a.name().cmp(b.name()));
    catalog
}

pub(crate) struct Store {
    root: PathBuf,
    _lock: File,
}
impl Store {
    pub(crate) fn open(root: PathBuf) -> Result<Self, String> {
        private_dir(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock().map_err(|_| {
            crate::i18n::tr("Another Files window is changing extensions; try again")
        })?;
        Ok(Self { root, _lock: lock })
    }
    fn entry(&self, id: &str) -> Result<Installed, String> {
        if !manifest::identifier(id) {
            return Err(crate::i18n::tr("Invalid extension ID").into());
        }
        load(self.root.clone())
            .entries
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| crate::i18n::tr("Extension is no longer installed").into())
    }
    fn save(&self, id: &str, state: &State) -> Result<(), String> {
        if !manifest::identifier(id) {
            return Err(crate::i18n::tr("Invalid extension ID").into());
        }
        let dir = self.root.join(id);
        private_dir(&dir)?;
        publish(
            &dir.join("state.json"),
            &serde_json::to_vec(state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    pub(crate) fn enable(&self, id: &str, enabled: bool) -> Result<(), String> {
        let mut entry = self.entry(id)?;
        if enabled && entry.problem.is_some() {
            return Err(entry.problem.unwrap_or_default());
        }
        entry.state.enabled = enabled;
        self.save(id, &entry.state)
    }
    pub(crate) fn settings(&self, id: &str, values: BTreeMap<String, Value>) -> Result<(), String> {
        let mut entry = self.entry(id)?;
        let manifest = entry
            .manifest
            .as_ref()
            .ok_or_else(|| crate::i18n::tr("Missing extension manifest"))?;
        if values.len() > 16
            || values.iter().any(|(key, value)| {
                !manifest
                    .settings
                    .iter()
                    .any(|s| s.id == *key && s.accepts(value))
            })
        {
            return Err(crate::i18n::tr("Invalid extension setting value").into());
        }
        entry.state.settings = values;
        self.save(id, &entry.state)
    }
    pub(crate) fn remove(&self, id: &str) -> Result<(), String> {
        let mut entry = self.entry(id)?;
        if entry.bundled {
            entry.state.enabled = false;
            entry.state.removed = true;
            entry.state.settings.clear();
            return self.save(id, &entry.state);
        }
        let path = self.root.join(id);
        let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        } else {
            fs::remove_file(path).map_err(|e| e.to_string())
        }
    }
    pub(crate) fn restore(&self) -> Result<(), String> {
        self.save("org.enzui.checksums", &State::default())
    }
    pub(crate) fn install(&self, package: &Prepared) -> Result<(), String> {
        let id = &package.manifest.id;
        package.manifest.validate()?;
        if id.starts_with("org.enzui.") {
            return Err(crate::i18n::tr("Bundled extensions are updated with Files").into());
        }
        let target = self.root.join(id);
        if !target.exists() && load(self.root.clone()).entries.len() >= 64 {
            return Err(crate::i18n::tr(
                "Remove an extension before installing another; the limit is 64",
            )
            .into());
        }
        let mut saved = State::default();
        if target.exists() {
            let old = self.entry(id)?;
            let manifest = old.manifest.ok_or_else(|| {
                crate::i18n::tr("Remove the damaged extension before reinstalling")
            })?;
            let new_version =
                semver::Version::parse(&package.manifest.version).map_err(|e| e.to_string())?;
            let old_version =
                semver::Version::parse(&manifest.version).map_err(|e| e.to_string())?;
            if !new_version.cmp_precedence(&old_version).is_gt() {
                return Err(crate::i18n::tr(
                    "Install a newer extension version, or remove the existing version first",
                )
                .into());
            }
            saved.settings = old
                .state
                .settings
                .into_iter()
                .filter(|(key, value)| {
                    package
                        .manifest
                        .settings
                        .iter()
                        .any(|s| s.id == *key && s.accepts(value))
                })
                .collect();
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let stage = self
            .root
            .join(format!(".install-{}-{stamp}", std::process::id()));
        private_dir(&stage)?;
        let result = (|| {
            for (name, bytes) in &package.files {
                if !manifest::relative(Path::new(name)) || name == "state.json" {
                    return Err(crate::i18n::tr("Unsafe package path").into());
                }
                let path = stage.join(name);
                if let Some(parent) = path.parent() {
                    private_dir(parent)?;
                }
                let executable = matches!(&package.manifest.worker, Worker::Native { program, .. } if program == name);
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(if executable { 0o700 } else { 0o600 })
                    .open(path)
                    .map_err(|e| e.to_string())?;
                file.write_all(bytes).map_err(|e| e.to_string())?;
                file.sync_all().map_err(|e| e.to_string())?;
                saved.digests.insert(name.clone(), hash(bytes));
            }
            publish(
                &stage.join("state.json"),
                &serde_json::to_vec(&saved).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let mut parents = std::collections::BTreeSet::new();
            for name in package.files.keys() {
                let mut parent = stage.join(name).parent().map(Path::to_path_buf);
                while let Some(dir) = parent {
                    if !dir.starts_with(&stage) {
                        break;
                    }
                    parents.insert(dir.clone());
                    parent = dir.parent().map(Path::to_path_buf);
                }
            }
            for dir in parents.iter().rev() {
                File::open(dir)
                    .and_then(|file| file.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            if target.exists() {
                rustix::fs::renameat_with(
                    rustix::fs::CWD,
                    &stage,
                    rustix::fs::CWD,
                    &target,
                    rustix::fs::RenameFlags::EXCHANGE,
                )
                .map_err(|e| e.to_string())?;
            } else {
                crate::services::rename_paths_no_replace(&stage, &target)
                    .map_err(|e| e.to_string())?;
            }
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if stage.exists() {
            let _ = fs::remove_dir_all(stage);
        }
        result
    }
}

fn publish(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut authority =
        enzui_core::state_authority::StateAuthority::open_or_create_tightening(path)
            .map_err(std::io::Error::other)?;
    authority
        .publish(bytes)
        .map(|_| ())
        .map_err(std::io::Error::other)
}
