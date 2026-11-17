// Demo stand-ins for the enzimOS mount and history services.
//
// `FILES_APP_DEMO_SERVICES=1` starts, inside Files, small fakes of
// `org.enzimos.Mount1` and `org.enzimos.History1` on the session bus, so
// every screen built on them can be tried and captured where the real
// services are not running (the capture sandbox has a private bus). Files
// talks to them through its ordinary D-Bus client code; only the history
// client moves to the session bus, since a demo cannot own a system name.
//
// The fake mount service lists a USB stick, an encrypted drive, a saved
// network share that asks for a password, a server that stopped answering
// and a cloud drive. Mounted folders are real directories under the runtime
// directory with a few sample files. The fake history service keeps four
// earlier versions of every file in the home and older views of every
// folder, with two files deleted since. Nothing in it reaches a real
// service, server or account.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use zbus::zvariant::OwnedFd;

const MOUNT_NAME: &str = "org.enzimos.Mount1";
const MOUNT_PATH: &str = "/org/enzimos/Mount1";
const HISTORY_NAME: &str = "org.enzimos.History1";
const HISTORY_PATH: &str = "/org/enzimos/History1";

static ENABLED: OnceLock<bool> = OnceLock::new();

// Whether the demo services were asked for. Never in tests.
pub(crate) fn enabled() -> bool {
    !cfg!(test)
        && *ENABLED.get_or_init(|| {
            std::env::var("FILES_APP_DEMO_SERVICES").is_ok_and(|value| value == "1")
        })
}

// Starts the demo services and waits (briefly) until they own their names,
// so service detection finds them. Does nothing unless enabled.
pub(crate) fn start() {
    if !enabled() {
        return;
    }
    let (ready, started) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("files-demo-services".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                let _ = ready.send(false);
                return;
            };
            runtime.block_on(async move {
                match serve().await {
                    Ok(connection) => {
                        let _ = ready.send(true);
                        // Keep serving for the life of the process.
                        let _keep = connection;
                        std::future::pending::<()>().await;
                    }
                    Err(error) => {
                        eprintln!("files-app: demo services: {error}");
                        let _ = ready.send(false);
                    }
                }
            });
        });
    if spawned.is_ok() {
        let _ = started.recv_timeout(Duration::from_secs(5));
    }
}

async fn serve() -> zbus::Result<zbus::Connection> {
    let root = std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join(format!("files-demo-{}", std::process::id()));
    let home = crate::services::home_dir();
    seed_home(&home);
    let state = Arc::new(Mutex::new(MountState::new(&root)));
    let connection = zbus::connection::Builder::session()?
        .name(MOUNT_NAME)?
        .name(HISTORY_NAME)?
        .serve_at(
            MOUNT_PATH,
            Mount {
                state: Arc::clone(&state),
            },
        )?
        .serve_at(HISTORY_PATH, History::new(home))?
        .build()
        .await?;
    Ok(connection)
}

// A text file whose earlier versions read differently, for the preview.
fn seed_home(home: &Path) {
    let documents = home.join("Documents");
    let notes = documents.join("Project notes.txt");
    if std::fs::create_dir_all(&documents).is_ok() && !notes.exists() {
        let _ = std::fs::write(&notes, NOTES.join("\n"));
    }
}

const NOTES: [&str; 14] = [
    "Project notes",
    "=============",
    "",
    "Launch moved to the second week of December.",
    "Design review with the whole team on Thursday.",
    "",
    "Open questions",
    "- Which file systems do we test on USB sticks?",
    "- Can we keep earlier versions for a month?",
    "- Who signs off the onboarding copy?",
    "",
    "Next steps",
    "- Send the draft to Mia and Jonas",
    "- Book the room for the review",
];

// ---------------------------------------------------------------------------
// org.enzimos.Mount1

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.enzimos.Mount1.Error")]
enum MountError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NeedsCredentials(String),
    AuthFailed(String),
    HostKeyUnknown(String),
    ChooseShare(String),
    ProtocolMismatch(String),
    Timeout(String),
    Busy(String),
    NotFound(String),
    Invalid(String),
}

#[derive(Clone, Debug, Default)]
struct DemoItem {
    id: String,
    kind: &'static str,
    label: String,
    detail: String,
    state: &'static str,
    reason: Option<String>,
    mount: Option<PathBuf>,
    uri: Option<String>,
    device: Option<String>,
    saved: bool,
    auto_connect: bool,
    mount_at_boot: bool,
    free: Option<u64>,
    capacity: Option<u64>,
    can_format: bool,
    filesystem: Option<String>,
    // Demo behaviour, never reported.
    needs_password: bool,
    host_key: bool,
    smb1: bool,
    server_only: bool,
    passphrase: bool,
}

impl DemoItem {
    fn json(&self) -> serde_json::Value {
        let mounted = self.state == "mounted";
        let network = matches!(self.kind, "network" | "cloud");
        serde_json::json!({
            "id": self.id,
            "kind": self.kind,
            "label": self.label,
            "detail": self.detail,
            "state": self.state,
            "reason": self.reason,
            "mount_path": if mounted { self.mount.clone() } else { None },
            "read_only": false,
            "can_mount": matches!(self.state, "unmounted" | "failed"),
            "can_unmount": mounted,
            "can_eject": false,
            "can_power_off": self.kind == "usb",
            "device": self.device,
            "uri": self.uri,
            "saved": self.saved,
            "auto_connect": self.auto_connect,
            "mount_at_boot": self.mount_at_boot,
            "can_mount_at_boot": network && self.kind == "network" && !self.server_only
                && self.uri.as_deref().is_some_and(|uri| uri.starts_with("smb://") || uri.starts_with("nfs://")),
            "free": if mounted || !network { self.free } else { None },
            "capacity": if mounted || !network { self.capacity } else { None },
            "can_format": self.can_format,
            "filesystem": self.filesystem,
        })
    }
}

struct MountState {
    root: PathBuf,
    generation: u64,
    next: u32,
    items: Vec<DemoItem>,
}

const GB: u64 = 1_000_000_000;

impl MountState {
    fn new(root: &Path) -> Self {
        let mut state = Self {
            root: root.to_path_buf(),
            generation: 1,
            next: 1,
            items: Vec::new(),
        };
        let stick = DemoItem {
            id: "block:sdb1".into(),
            kind: "usb",
            label: "SANDISK".into(),
            detail: "32 GB · SanDisk Ultra Fit".into(),
            state: "mounted",
            device: Some("/dev/sdb1".into()),
            free: Some(21_400_000_000),
            capacity: Some(31_900_000_000),
            can_format: true,
            filesystem: Some("exfat".into()),
            ..DemoItem::default()
        };
        state.attach(stick, &["Holiday photos/", "Slides.pdf", "Notes.txt"]);
        state.items.push(DemoItem {
            id: "block:sdc1".into(),
            kind: "usb",
            label: "Travel Drive".into(),
            detail: "1 TB · Encrypted".into(),
            state: "locked",
            device: Some("/dev/sdc1".into()),
            capacity: Some(1_000 * GB),
            filesystem: Some("crypto_LUKS".into()),
            passphrase: true,
            ..DemoItem::default()
        });
        state.items.push(DemoItem {
            id: "net-team".into(),
            kind: "network",
            label: "Team on nas.local".into(),
            state: "unmounted",
            uri: Some("smb://nas.local/team".into()),
            saved: true,
            auto_connect: true,
            free: Some(812 * GB),
            capacity: Some(2_000 * GB),
            needs_password: true,
            ..DemoItem::default()
        });
        state.items.push(DemoItem {
            id: "net-media".into(),
            kind: "network",
            label: "Media on old-nas.local".into(),
            state: "not_responding",
            reason: Some("The server “old-nas.local” stopped answering".into()),
            uri: Some("smb://old-nas.local/media".into()),
            saved: true,
            ..DemoItem::default()
        });
        let cloud = DemoItem {
            id: "cloud-onedrive".into(),
            kind: "cloud",
            label: "OneDrive".into(),
            state: "mounted",
            uri: Some("onedrive:".into()),
            saved: true,
            auto_connect: true,
            free: Some(912 * GB),
            capacity: Some(1_000 * GB),
            ..DemoItem::default()
        };
        state.attach(cloud, &["Documents/", "Shared/", "Budget 2027.ods"]);
        state
    }

    // Adds `item` with a real folder holding `files` (a trailing `/` makes a folder).
    fn attach(&mut self, mut item: DemoItem, files: &[&str]) {
        let folder = self.root.join(&item.label);
        if std::fs::create_dir_all(&folder).is_ok() {
            for name in files {
                let path = folder.join(name.trim_end_matches('/'));
                let _ = if name.ends_with('/') {
                    std::fs::create_dir_all(&path)
                } else {
                    std::fs::write(&path, format!("{name}\n"))
                };
            }
        }
        item.mount = Some(folder);
        self.items.push(item);
    }

    fn snapshot(&self) -> String {
        serde_json::json!({
            "version": 1,
            "generation": self.generation,
            "capabilities": {
                "drives": true, "smb": true, "nfs": true, "sftp": true, "webdav_ftp": true,
                "mtp": true, "ios": true, "smb_browse": true, "keyring": true,
                "cloud": ["google_drive", "onedrive", "dropbox", "nextcloud"],
            },
            "items": self.items.iter().map(DemoItem::json).collect::<Vec<_>>(),
        })
        .to_string()
    }

    fn item(&mut self, id: &str) -> Result<&mut DemoItem, MountError> {
        self.items
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| MountError::NotFound("That location is no longer there".into()))
    }

    // The folder a location mounts on, with a few files the first time.
    fn folder_for(&self, label: &str) -> PathBuf {
        let folder = self.root.join(label);
        let fresh = !folder.exists();
        let _ = std::fs::create_dir_all(&folder);
        if fresh {
            for name in ["Projects", "Shared photos"] {
                let _ = std::fs::create_dir_all(folder.join(name));
            }
            for name in ["Budget 2027.ods", "Meeting notes.md", "Welcome.txt"] {
                let _ = std::fs::write(folder.join(name), format!("{name}\n"));
            }
        }
        folder
    }
}

struct Mount {
    state: Arc<Mutex<MountState>>,
}

fn read_secret(fd: &OwnedFd) -> String {
    use std::io::Read;
    let mut text = String::new();
    use std::os::fd::AsFd;
    if let Ok(duplicate) = fd.as_fd().try_clone_to_owned() {
        let mut file = std::fs::File::from(duplicate);
        let _ = file.read_to_string(&mut text);
    }
    text
}

fn bool_option(options: &HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<bool> {
    options
        .get(key)
        .and_then(|value| bool::try_from(value).ok())
}

fn text_option(options: &HashMap<String, zbus::zvariant::OwnedValue>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok().map(str::to_owned))
}

impl Mount {
    fn lock(&self) -> std::sync::MutexGuard<'_, MountState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn changed(&self, connection: &zbus::Connection) {
        let snapshot = {
            let mut state = self.lock();
            state.generation += 1;
            state.snapshot()
        };
        let _ = connection
            .emit_signal(
                None::<()>,
                MOUNT_PATH,
                MOUNT_NAME,
                "Changed",
                &(snapshot.as_str(),),
            )
            .await;
    }

    async fn set_state(&self, connection: &zbus::Connection, id: &str, state: &'static str) {
        if let Ok(item) = self.lock().item(id) {
            item.state = state;
        }
        self.changed(connection).await;
    }

    // Mounts `id` after the checks a real mount would raise.
    async fn mount_checked(
        &self,
        connection: &zbus::Connection,
        id: &str,
        password: Option<String>,
        remember: bool,
    ) -> Result<String, MountError> {
        let item = self.lock().item(id)?.clone();
        if item.state == "mounted"
            && let Some(path) = &item.mount
        {
            return Ok(path.to_string_lossy().into_owned());
        }
        if item.state == "not_responding" {
            return Err(MountError::Timeout(format!(
                "The server “{}” did not answer",
                host_of(item.uri.as_deref().unwrap_or_default())
            )));
        }
        if item.state == "locked" {
            return Err(MountError::Invalid("Unlock the drive first".into()));
        }
        if item.server_only {
            let host = host_of(item.uri.as_deref().unwrap_or_default());
            let shares: Vec<_> = [
                ("team", "Shared team folder"),
                ("photos", "Photo library"),
                ("scans", ""),
            ]
            .iter()
            .map(|(name, comment)| {
                serde_json::json!({"name": name, "comment": comment, "uri": format!("smb://{host}/{name}")})
            })
            .collect();
            return Err(MountError::ChooseShare(
                serde_json::json!({"id": id, "shares": shares}).to_string(),
            ));
        }
        if item.smb1 {
            return Err(MountError::ProtocolMismatch(format!(
                "“{}” only supports SMB1, an old version of the file sharing protocol",
                host_of(item.uri.as_deref().unwrap_or_default())
            )));
        }
        if item.host_key {
            return Err(MountError::HostKeyUnknown(
                serde_json::json!({
                    "id": id,
                    "host": host_of(item.uri.as_deref().unwrap_or_default()),
                    "fingerprints": ["SHA256:4Lx9m2Qp7vT0cK3yJd8Wf1sHn6bRz5aEo2uGtV0iYkM"],
                })
                .to_string(),
            ));
        }
        if item.needs_password {
            match password.as_deref() {
                None | Some("") => {
                    return Err(MountError::NeedsCredentials(
                        serde_json::json!({
                            "id": id, "username": "alex", "domain": "",
                            "can_remember": true, "retry": false,
                        })
                        .to_string(),
                    ));
                }
                Some("wrong") => {
                    return Err(MountError::AuthFailed(
                        "The user name or password was not accepted".into(),
                    ));
                }
                Some(_) => {}
            }
        }
        self.set_state(connection, id, "mounting").await;
        tokio::time::sleep(Duration::from_millis(900)).await;
        let path = {
            let mut state = self.lock();
            let label = state.item(id)?.label.clone();
            let folder = state.folder_for(&label);
            let item = state.item(id)?;
            item.state = "mounted";
            item.reason = None;
            item.mount = Some(folder.clone());
            if remember {
                item.needs_password = false;
                item.saved = true;
            }
            folder
        };
        self.changed(connection).await;
        Ok(path.to_string_lossy().into_owned())
    }
}

fn host_of(uri: &str) -> String {
    let rest = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    rest.split(['/', ':']).next().unwrap_or_default().to_owned()
}

#[zbus::interface(name = "org.enzimos.Mount1")]
impl Mount {
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    fn list(&self) -> String {
        self.lock().snapshot()
    }

    async fn mount(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
    ) -> Result<String, MountError> {
        self.mount_checked(connection, &id, None, false).await
    }

    async fn mount_with_credentials(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        _username: String,
        _domain: String,
        secret: OwnedFd,
        remember: bool,
    ) -> Result<String, MountError> {
        let password = read_secret(&secret);
        self.mount_checked(connection, &id, Some(password), remember)
            .await
    }

    async fn unlock(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        passphrase: OwnedFd,
    ) -> Result<String, MountError> {
        tokio::time::sleep(Duration::from_millis(700)).await;
        if read_secret(&passphrase) == "wrong" {
            return Err(MountError::AuthFailed(
                "The passphrase was not accepted".into(),
            ));
        }
        let path = {
            let mut state = self.lock();
            let label = state.item(&id)?.label.clone();
            let folder = state.folder_for(&label);
            for name in ["Backups", "Tax 2026.pdf"] {
                let _ = std::fs::write(folder.join(name), name);
            }
            let item = state.item(&id)?;
            item.state = "mounted";
            item.mount = Some(folder.clone());
            item.free = Some(640 * GB);
            item.filesystem = Some("ext4".into());
            folder
        };
        self.changed(connection).await;
        Ok(path.to_string_lossy().into_owned())
    }

    async fn unmount(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        _force: bool,
    ) -> Result<(), MountError> {
        self.set_state(connection, &id, "unmounting").await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        {
            let mut state = self.lock();
            let item = state.item(&id)?;
            item.state = if item.passphrase {
                "locked"
            } else {
                "unmounted"
            };
            item.reason = None;
        }
        self.changed(connection).await;
        Ok(())
    }

    async fn eject(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
    ) -> Result<(), MountError> {
        self.lock().items.retain(|item| item.id != id);
        self.changed(connection).await;
        Ok(())
    }

    async fn safely_remove(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
    ) -> Result<(), MountError> {
        self.eject(connection, id).await
    }

    async fn add_location(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        uri: String,
        label: String,
        options: HashMap<String, zbus::zvariant::OwnedValue>,
    ) -> Result<String, MountError> {
        let uri = normalize_address(&uri).ok_or_else(|| {
            MountError::Invalid(format!("“{uri}” is not an address Files can connect to"))
        })?;
        let auto_connect = bool_option(&options, "auto_connect").unwrap_or(false);
        let save = bool_option(&options, "save").unwrap_or(false) || auto_connect;
        let id = {
            let mut state = self.lock();
            if let Some(item) = state
                .items
                .iter_mut()
                .find(|item| item.uri.as_deref() == Some(&uri))
            {
                item.saved |= save;
                item.auto_connect |= auto_connect;
                item.id.clone()
            } else {
                let host = host_of(&uri);
                let rest = uri.split_once("://").map_or("", |(_, rest)| rest);
                let share = rest.split('/').nth(1).unwrap_or_default().to_owned();
                let label = if !label.is_empty() {
                    label
                } else if share.is_empty() {
                    host.clone()
                } else {
                    format!("{share} on {host}")
                };
                let id = format!("net-{}", state.next);
                state.next += 1;
                state.items.push(DemoItem {
                    id: id.clone(),
                    kind: "network",
                    label,
                    state: "unmounted",
                    uri: Some(uri.clone()),
                    saved: save,
                    auto_connect,
                    free: Some(1_200 * GB),
                    capacity: Some(4_000 * GB),
                    server_only: uri.starts_with("smb://") && share.is_empty(),
                    smb1: host.starts_with("old"),
                    host_key: uri.starts_with("sftp://"),
                    needs_password: uri.starts_with("smb://") && !share.is_empty(),
                    ..DemoItem::default()
                });
                id
            }
        };
        self.changed(connection).await;
        Ok(id)
    }

    async fn update_location(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        options: HashMap<String, zbus::zvariant::OwnedValue>,
    ) -> Result<(), MountError> {
        {
            let mut state = self.lock();
            let item = state.item(&id)?;
            if let Some(label) = text_option(&options, "label") {
                item.label = label;
            }
            if let Some(auto) = bool_option(&options, "auto_connect") {
                item.auto_connect = auto;
                item.saved |= auto;
            }
            if bool_option(&options, "forget_password") == Some(true) {
                item.needs_password = true;
            }
            if text_option(&options, "smb_version").as_deref() == Some("1.0") {
                item.smb1 = false;
            }
        }
        self.changed(connection).await;
        Ok(())
    }

    async fn set_mount_at_boot(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        enabled: bool,
        secret: OwnedFd,
    ) -> Result<(), MountError> {
        let has_password = !read_secret(&secret).is_empty();
        {
            let mut state = self.lock();
            let item = state.item(&id)?;
            if enabled && item.needs_password && !has_password {
                return Err(MountError::NeedsCredentials(
                    serde_json::json!({
                        "id": id, "username": "alex", "domain": "",
                        "can_remember": true, "retry": false,
                    })
                    .to_string(),
                ));
            }
            item.mount_at_boot = enabled;
            if enabled {
                item.saved = true;
                item.auto_connect = true;
            }
        }
        // The administrator approval a real service asks for.
        tokio::time::sleep(Duration::from_millis(400)).await;
        self.changed(connection).await;
        Ok(())
    }

    async fn remove_location(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
    ) -> Result<(), MountError> {
        self.lock().item(&id)?;
        self.lock().items.retain(|item| item.id != id);
        self.changed(connection).await;
        Ok(())
    }

    fn trust_host_key(&self, id: String, _fingerprints: Vec<String>) -> Result<(), MountError> {
        self.lock().item(&id)?.host_key = false;
        Ok(())
    }

    async fn discover(&self) -> String {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        serde_json::json!([
            {"name": "nas", "uri": "smb://nas.local", "source": "mdns"},
            {"name": "studio-pc", "uri": "smb://studio-pc.local", "source": "wsd"},
        ])
        .to_string()
    }

    fn list_shares(
        &self,
        uri: String,
        _username: String,
        _domain: String,
        _secret: OwnedFd,
    ) -> String {
        let host = host_of(&uri);
        serde_json::json!([
            {"name": "team", "comment": "Shared team folder", "uri": format!("smb://{host}/team")},
            {"name": "photos", "comment": "Photo library", "uri": format!("smb://{host}/photos")},
        ])
        .to_string()
    }

    async fn mount_image(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        path: String,
    ) -> Result<String, MountError> {
        let label = Path::new(&path).file_stem().map_or_else(
            || "Disk image".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let folder = {
            let mut state = self.lock();
            let id = format!("image-{}", state.next);
            state.next += 1;
            let item = DemoItem {
                id,
                kind: "image",
                label: label.clone(),
                state: "mounted",
                ..DemoItem::default()
            };
            state.attach(item, &["README.txt"]);
            state.folder_for(&label)
        };
        self.changed(connection).await;
        Ok(folder.to_string_lossy().into_owned())
    }

    async fn add_cloud(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        provider: String,
        server: String,
    ) -> Result<String, MountError> {
        let label = match provider.as_str() {
            "google_drive" => "Google Drive".to_owned(),
            "onedrive" => "OneDrive".to_owned(),
            "dropbox" => "Dropbox".to_owned(),
            "nextcloud" => {
                let host = server
                    .trim()
                    .trim_start_matches("https://")
                    .trim_end_matches('/');
                if !host.contains('.') {
                    return Err(MountError::Invalid(format!(
                        "No Nextcloud server answers at “{}”. Check the address and try again.",
                        server.trim()
                    )));
                }
                format!("Nextcloud on {host}")
            }
            _ => {
                return Err(MountError::Invalid(
                    "That cloud service is not offered".into(),
                ));
            }
        };
        let id = {
            let mut state = self.lock();
            if state.items.iter().any(|item| item.state == "signing_in") {
                return Err(MountError::Busy(
                    "Another sign-in is still open in your browser".into(),
                ));
            }
            let id = format!("cloud-{}", state.next);
            state.next += 1;
            state.items.push(DemoItem {
                id: id.clone(),
                kind: "cloud",
                label,
                state: "signing_in",
                uri: Some(format!("{provider}:")),
                free: Some(14 * GB),
                capacity: Some(15 * GB),
                ..DemoItem::default()
            });
            id
        };
        self.changed(connection).await;
        // The person finishes signing in a little later.
        let state = Arc::clone(&self.state);
        let connection = connection.clone();
        let finished = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(6)).await;
            let mount = Mount { state };
            {
                let mut state = mount.lock();
                let Ok(label) = state.item(&finished).map(|item| item.label.clone()) else {
                    return;
                };
                let folder = state.folder_for(&label);
                let _ = std::fs::create_dir_all(folder.join("Documents"));
                if let Ok(item) = state.item(&finished) {
                    item.state = "mounted";
                    item.saved = true;
                    item.auto_connect = true;
                    item.mount = Some(folder);
                }
            }
            mount.changed(&connection).await;
        });
        Ok(id)
    }

    async fn format(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        id: String,
        options: HashMap<String, zbus::zvariant::OwnedValue>,
    ) -> Result<String, MountError> {
        let kind = text_option(&options, "type").unwrap_or_else(|| "exfat".into());
        let label = text_option(&options, "label").unwrap_or_default();
        {
            let mut state = self.lock();
            let item = state.item(&id)?;
            if !item.can_format {
                return Err(MountError::Invalid("This drive cannot be formatted".into()));
            }
            item.state = "formatting";
        }
        self.changed(connection).await;
        tokio::time::sleep(Duration::from_secs(4)).await;
        {
            let mut state = self.lock();
            let root = state.root.clone();
            let item = state.item(&id)?;
            if let Some(old) = item.mount.take() {
                let _ = std::fs::remove_dir_all(old);
            }
            item.label = if label.is_empty() {
                "Untitled".into()
            } else {
                label
            };
            item.filesystem = Some(kind);
            item.state = "unmounted";
            item.free = item.capacity.map(|capacity| capacity - capacity / 200);
            let _ = std::fs::create_dir_all(root.join(&item.label));
        }
        self.changed(connection).await;
        Ok(id)
    }
}

// An address in the forms the real service accepts, as a URI.
fn normalize_address(address: &str) -> Option<String> {
    let address = address.trim();
    if address.contains("://") {
        return Some(address.trim_end_matches('/').to_owned());
    }
    if let Some(rest) = address.strip_prefix("\\\\") {
        return Some(format!("smb://{}", rest.replace('\\', "/")));
    }
    if let Some((user_host, path)) = address.split_once(":/") {
        return Some(format!("sftp://{user_host}/{path}"));
    }
    (!address.is_empty()).then(|| format!("smb://{address}"))
}

// ---------------------------------------------------------------------------
// org.enzimos.History1

struct History {
    home: PathBuf,
    excluded: Mutex<Vec<String>>,
    // (number, taken) newest first.
    snapshots: Vec<(u32, i64)>,
}

impl History {
    fn new(home: PathBuf) -> Self {
        let now = crate::history::now();
        // On the hour, as the real service keeps them.
        let hour = now - now.rem_euclid(3600);
        Self {
            excluded: Mutex::new(vec![home.join("Videos").to_string_lossy().into_owned()]),
            home,
            snapshots: vec![
                (52, hour),
                (47, hour - 3 * 3600),
                (31, hour - 26 * 3600),
                (24, hour - 3 * 86_400 - 5 * 3600),
                (12, hour - 9 * 86_400 - 2 * 3600),
                (4, hour - 33 * 86_400),
            ],
        }
    }

    fn inside(&self, path: &str) -> fdo::Result<PathBuf> {
        let path = PathBuf::from(path);
        if path.starts_with(&self.home) {
            Ok(path)
        } else {
            Err(fdo::Error::Failed("path-not-allowed".into()))
        }
    }

    // How many lines an older version of a text file keeps.
    fn lines_kept(&self, number: u32, total: usize) -> usize {
        let position = self
            .snapshots
            .iter()
            .position(|(n, _)| *n == number)
            .unwrap_or(0);
        total.saturating_sub(position * 3).max(3.min(total))
    }
}

use zbus::fdo;

// Files deleted since the older snapshots, shown in a folder as it was.
const DELETED: [(&str, u64); 2] = [("Old budget.ods", 48_210), ("Meeting notes.txt", 2_310)];

fn is_text(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "txt" | "md" | "csv" | "rs" | "toml"
        )
    })
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

type HistoryVersion = (u32, i64, u64, i64, bool);
type HistoryEntry = (String, String, u64, i64, bool);

#[zbus::interface(name = "org.enzimos.History1")]
impl History {
    fn snapshots(&self) -> Vec<(u32, i64)> {
        self.snapshots.clone()
    }

    fn versions(&self, path: String) -> fdo::Result<Vec<HistoryVersion>> {
        let path = self.inside(&path)?;
        let meta = std::fs::metadata(&path).map_err(|_| fdo::Error::Failed("not-found".into()))?;
        let size = meta.len();
        // The newest snapshot matches the file now; three older ones differ.
        Ok(self
            .snapshots
            .iter()
            .take(5)
            .enumerate()
            .filter(|(index, _)| *index != 3)
            .map(|(index, &(number, taken))| {
                let step = u64::try_from(index).unwrap_or(0);
                let older = size.saturating_sub(size / 9 * step);
                (number, taken, older, taken - 1_200, index == 0)
            })
            .collect())
    }

    fn folder(&self, path: String, number: u32) -> fdo::Result<Vec<HistoryEntry>> {
        let path = self.inside(&path)?;
        let newest = self.snapshots.first().map(|(n, _)| *n) == Some(number);
        let taken = self
            .snapshots
            .iter()
            .find(|(n, _)| *n == number)
            .map(|(_, t)| *t)
            .ok_or_else(|| fdo::Error::Failed("not-found".into()))?;
        let mut items = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let kind = if meta.is_dir() { "d" } else { "f" };
                items.push((
                    name,
                    kind.to_owned(),
                    meta.len(),
                    mtime(&meta).min(taken),
                    true,
                ));
            }
        }
        if !newest {
            for (name, size) in DELETED {
                items.push((name.to_owned(), "f".to_owned(), size, taken - 86_400, false));
            }
        }
        items.sort_by(|a, b| {
            (b.1 == "d")
                .cmp(&(a.1 == "d"))
                .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        Ok(items)
    }

    fn open(&self, path: String, number: u32) -> fdo::Result<OwnedFd> {
        let path = self.inside(&path)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) if is_text(&path) => {
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<&str> = text.lines().collect();
                let kept = self.lines_kept(number, lines.len());
                lines[..kept].join("\n").into_bytes()
            }
            Ok(bytes) => bytes,
            // A file deleted since: a short stand-in.
            Err(_) => b"Meeting notes\n\n- Agree on the release date\n- Review the backups page\n"
                .to_vec(),
        };
        use std::io::{Seek, SeekFrom, Write};
        let fd = rustix::fs::memfd_create("files-demo-version", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&bytes)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok(OwnedFd::from(std::os::fd::OwnedFd::from(file)))
    }

    fn rules(&self) -> (bool, Vec<String>) {
        (
            true,
            self.excluded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
    }

    fn set_excluded(&self, path: String, excluded: bool) -> fdo::Result<()> {
        let path = self.inside(&path)?;
        let text = path.to_string_lossy().into_owned();
        let mut list = self
            .excluded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        list.retain(|p| *p != text);
        if excluded {
            list.push(text);
        }
        Ok(())
    }

    async fn forget(&self, paths: Vec<String>) -> fdo::Result<(u64, u32, bool)> {
        // Where the real service asks for the person's password.
        tokio::time::sleep(Duration::from_millis(800)).await;
        Ok((u64::try_from(paths.len()).unwrap_or(0) * 4, 4, true))
    }

    async fn back_up_now(&self) -> fdo::Result<()> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(())
    }
}
