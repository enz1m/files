// The enzimOS mount service, `org.enzimos.Mount1` on the session bus: the main
// route for drives, phones, network locations and disk images when it is
// installed. Without it Files keeps its GIO, GVfs and udisksctl routes.
//
// Calls may legitimately take a minute (a slow server, a polkit prompt), so
// they run as futures on the iced executor or on a private runtime off the UI
// thread, never with a short D-Bus timeout.

use crate::desktop_storage::{Kind, Location, ServiceItem};
pub(crate) use cloud::{CloudState, add_cloud};
use enzui_core::form::SecretText;
pub(crate) use format::{FileSystem, FormatPhase, FormatState, format};
use iced::futures::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

const NAME: &str = "org.enzimos.Mount1";
const PATH: &str = "/org/enzimos/Mount1";
const INTERFACE: &str = "org.enzimos.Mount1";
const ERROR_PREFIX: &str = "org.enzimos.Mount1.Error.";
// Storage ids from the service carry this prefix, so operations route back.
const ID_PREFIX: &str = "mount1:";
// The service answers within about 45 s, plus any polkit prompt; the API
// asks clients to wait at least 180 s.
const METHOD_TIMEOUT: Duration = Duration::from_secs(300);
// The largest secret the service accepts, in bytes.
const SECRET_LIMIT: usize = 4096;
// Characters of a service message shown to the user.
const MESSAGE_LIMIT: usize = 300;

static AVAILABLE: OnceLock<bool> = OnceLock::new();

// Whether detection found the service. `false` until detection has run, so
// a caller on the UI thread never waits for the bus.
pub(crate) fn detected() -> bool {
    AVAILABLE.get().copied().unwrap_or(false)
}

// Whether the service is running or D-Bus activatable, detected once per
// process. Unit tests always use the fallback routes.
pub(crate) async fn available() -> bool {
    if cfg!(test) {
        return false;
    }
    if let Some(found) = AVAILABLE.get() {
        return *found;
    }
    let found = detect().await;
    *AVAILABLE.get_or_init(|| found)
}

async fn detect() -> bool {
    let Ok(connection) = zbus::Connection::session().await else {
        return false;
    };
    let Ok(bus) = zbus::fdo::DBusProxy::new(&connection).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(NAME) else {
        return false;
    };
    if bus.name_has_owner(name).await.unwrap_or(false) {
        return true;
    }
    bus.list_activatable_names()
        .await
        .is_ok_and(|names| names.iter().any(|name| name.as_str() == NAME))
}

// The service's own id for a storage location it reported.
pub(crate) fn service_id(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX)
}

// The storage id Files uses for a service item.
pub(crate) fn storage_id(service_id: &str) -> String {
    format!("{ID_PREFIX}{service_id}")
}

#[derive(serde::Deserialize)]
struct Snapshot {
    #[serde(default)]
    capabilities: Capabilities,
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Capabilities {
    // The cloud services that can be added; unknown names are skipped.
    cloud: Vec<String>,
}

// The service's inventory: its locations and the cloud services it can add.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Inventory {
    pub(crate) locations: Vec<Location>,
    pub(crate) cloud: Vec<CloudProvider>,
}

// A cloud service the mount service can sign in to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloudProvider {
    GoogleDrive,
    OneDrive,
    Dropbox,
    Nextcloud,
}

impl CloudProvider {
    // In the order the chooser lists them.
    pub(crate) const ALL: [Self; 4] = [
        Self::GoogleDrive,
        Self::OneDrive,
        Self::Dropbox,
        Self::Nextcloud,
    ];

    // The service's name for it.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::GoogleDrive => "google_drive",
            Self::OneDrive => "onedrive",
            Self::Dropbox => "dropbox",
            Self::Nextcloud => "nextcloud",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|provider| provider.key() == key)
    }

    // Product names stay untranslated.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::GoogleDrive => "Google Drive",
            Self::OneDrive => "OneDrive",
            Self::Dropbox => "Dropbox",
            Self::Nextcloud => "Nextcloud",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::GoogleDrive => crate::i18n::tr("Your Google account's files"),
            Self::OneDrive => crate::i18n::tr("Your Microsoft account's files"),
            Self::Dropbox => crate::i18n::tr("Your Dropbox files"),
            Self::Nextcloud => {
                crate::i18n::tr("Files on a Nextcloud server, such as your own or your work's")
            }
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::GoogleDrive => "triangle",
            Self::OneDrive => "cloud",
            Self::Dropbox => "package-open",
            Self::Nextcloud => "server",
        }
    }
}

// One snapshot item. Unknown fields are ignored and missing ones default, as
// the API only ever adds fields.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Item {
    id: String,
    kind: String,
    label: String,
    state: String,
    reason: Option<String>,
    mount_path: Option<PathBuf>,
    read_only: bool,
    can_mount: bool,
    can_unmount: bool,
    can_eject: bool,
    device: Option<PathBuf>,
    uri: Option<String>,
    saved: bool,
    auto_connect: bool,
    mount_at_boot: bool,
    can_mount_at_boot: bool,
    free: Option<u64>,
    capacity: Option<u64>,
    can_format: bool,
    filesystem: Option<String>,
    // What the drive is, such as "16 GB · ACME Stick".
    detail: Option<String>,
    can_power_off: bool,
}

// Maps a `List` or `Changed` snapshot onto the storage model the sidebar and
// Computer views already show. Unknown kinds are generic disks; unknown states
// are neither locked nor empty.
pub(crate) fn inventory(snapshot: &str) -> Result<Inventory, String> {
    let snapshot: Snapshot = serde_json::from_str(snapshot)
        .map_err(|_| crate::i18n::tr("The storage inventory was invalid").to_owned())?;
    let mut cloud: Vec<CloudProvider> = snapshot
        .capabilities
        .cloud
        .iter()
        .filter_map(|key| CloudProvider::from_key(key))
        .collect();
    cloud.dedup();
    Ok(Inventory {
        locations: crate::desktop_storage::normalize(
            snapshot
                .items
                .into_iter()
                .filter(|item| !item.id.is_empty())
                .map(location)
                .collect(),
        ),
        cloud,
    })
}

pub(crate) fn locations(snapshot: &str) -> Result<Vec<Location>, String> {
    inventory(snapshot).map(|inventory| inventory.locations)
}

fn location(item: Item) -> Location {
    let kind = match item.kind.as_str() {
        "usb" | "card" => Kind::Usb,
        "optical" => Kind::Optical,
        "phone" | "ios" | "camera" => Kind::Phone,
        "network" => Kind::Network,
        "cloud" => Kind::Cloud,
        _ => Kind::Disk,
    };
    let locked = item.state == "locked";
    let not_responding = item.state == "not_responding";
    let mount = item.mount_path.filter(|path| path.is_absolute());
    let reason = match item.state.as_str() {
        "failed" => item.reason.map(|reason| bounded(&reason)),
        "not_responding" => Some(item.reason.map_or_else(
            || crate::i18n::tr("Not responding").to_owned(),
            |reason| bounded(&reason),
        )),
        _ => None,
    };
    // A capacity is only meaningful with the free space it bounds.
    let space = match (item.free, item.capacity) {
        (Some(free), Some(capacity)) if capacity > 0 => Some((free.min(capacity), capacity)),
        _ => None,
    };
    Location {
        id: storage_id(&item.id),
        name: item.label,
        kind,
        can_unmount: item.can_unmount && mount.is_some(),
        mount,
        device: item.device.filter(|path| path.is_absolute()),
        can_mount: item.can_mount || locked,
        can_eject: item.can_eject,
        empty_media: item.state == "no_media",
        read_only: item.read_only,
        disk_image: item.kind == "image",
        service: Some(ServiceItem {
            locked,
            not_responding,
            busy: matches!(item.state.as_str(), "mounting" | "unmounting"),
            uri: item.uri,
            reason,
            saved: item.saved,
            auto_connect: item.auto_connect,
            mount_at_boot: item.mount_at_boot,
            can_mount_at_boot: item.can_mount_at_boot,
            space,
            can_format: item.can_format && !item.read_only,
            filesystem: item.filesystem.map(|name| bounded(&name)),
            formatting: item.state == "formatting",
            signing_in: item.state == "signing_in",
            failed: item.state == "failed",
            detail: item
                .detail
                .map(|detail| bounded(&detail))
                .filter(|detail| !detail.is_empty()),
            can_power_off: item.can_power_off,
        }),
    }
}

// Streams the service's inventory into `output` until the receiver goes away,
// reconnecting with backoff when the service or bus fails.
pub(crate) async fn watch(
    mut output: iced::futures::channel::mpsc::Sender<Result<Inventory, String>>,
) {
    let mut backoff = 1;
    loop {
        let error = match watch_once(&mut output, &mut backoff).await {
            Ok(()) => crate::i18n::tr("The mount service stopped").to_owned(),
            Err(Stop) => return,
            Err(Failed(error)) => error,
        };
        if output.send(Err(error)).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

enum WatchError {
    // The subscription was dropped.
    Stop,
    Failed(String),
}
use WatchError::{Failed, Stop};

async fn watch_once(
    output: &mut iced::futures::channel::mpsc::Sender<Result<Inventory, String>>,
    backoff: &mut u64,
) -> Result<(), WatchError> {
    let unavailable = |_| Failed(crate::i18n::tr("The mount service is not available").to_owned());
    let proxy = proxy().await.map_err(unavailable)?;
    // Subscribe before listing so no change falls between the two.
    let mut changes = proxy.receive_signal("Changed").await.map_err(unavailable)?;
    let first: String = proxy.call("List", &()).await.map_err(unavailable)?;
    output.send(inventory(&first)).await.map_err(|_| Stop)?;
    *backoff = 1;
    while let Some(message) = changes.next().await {
        let Ok(snapshot) = message.body().deserialize::<String>() else {
            continue;
        };
        if snapshot.len() > crate::desktop_storage::FRAME_LIMIT {
            return Err(Failed(
                crate::i18n::tr("The storage inventory exceeded its size limit").to_owned(),
            ));
        }
        output.send(inventory(&snapshot)).await.map_err(|_| Stop)?;
    }
    Ok(())
}

async fn proxy() -> zbus::Result<zbus::Proxy<'static>> {
    let connection = zbus::connection::Builder::session()?
        .method_timeout(METHOD_TIMEOUT)
        .build()
        .await?;
    zbus::proxy::Builder::new(&connection)
        .destination(NAME)?
        .path(PATH)?
        .interface(INTERFACE)?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

// The service asked for a user name and password (`NeedsCredentials`).
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(default)]
pub(crate) struct CredentialRequest {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) domain: String,
    pub(crate) can_remember: bool,
    // The previous or remembered password was refused.
    pub(crate) retry: bool,
}

// An SFTP server whose key is not trusted yet, or has changed.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(default)]
pub(crate) struct HostKeyRequest {
    pub(crate) id: String,
    pub(crate) host: String,
    pub(crate) fingerprints: Vec<String>,
    #[serde(skip)]
    pub(crate) changed: bool,
}

// One share of an SMB server.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ShareChoice {
    pub(crate) name: String,
    pub(crate) comment: String,
    pub(crate) uri: String,
}

// A server location without a share (`ChooseShare`).
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(default)]
pub(crate) struct ShareRequest {
    pub(crate) id: String,
    pub(crate) shares: Vec<ShareChoice>,
}

// Why a service call did not complete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ServiceError {
    NeedsPassphrase,
    NeedsCredentials(CredentialRequest),
    HostKey(HostKeyRequest),
    ChooseShare(ShareRequest),
    // The SMB server only speaks SMB1, which needs the person's consent.
    ProtocolMismatch(String),
    // A supplied password or passphrase was refused.
    AuthFailed(String),
    // Files are open on the mount, or another operation is running.
    Busy(String),
    NotAuthorized(String),
    Timeout(String),
    // The user declined a question the operation needed.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsPassphrase => f.write_str(crate::i18n::tr("Passphrase required")),
            Self::NeedsCredentials(_) => f.write_str(crate::i18n::tr("Password required")),
            Self::HostKey(request) => f.write_str(&crate::i18n::format(
                "files-the-identity-of-host-is-not-a7b3e8a0",
                &[("host", request.host.clone())],
            )),
            Self::Cancelled => f.write_str(crate::i18n::tr("Cancelled")),
            Self::ChooseShare(_) => f.write_str(crate::i18n::tr("Choose a share")),
            Self::ProtocolMismatch(message)
            | Self::AuthFailed(message)
            | Self::Busy(message)
            | Self::NotAuthorized(message)
            | Self::Timeout(message)
            | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl ServiceError {
    // The device-operation error the drive actions report.
    pub(crate) fn into_device_error(self) -> crate::devices::DeviceError {
        use crate::devices::DeviceError;
        match self {
            Self::Busy(message) => DeviceError::InUse(message),
            Self::NotAuthorized(_) => DeviceError::NotAuthorized,
            Self::Timeout(_) => DeviceError::Timeout,
            Self::Cancelled => DeviceError::Cancelled,
            other => DeviceError::Failed(other.to_string()),
        }
    }
}

fn classify(error: zbus::Error) -> ServiceError {
    let zbus::Error::MethodError(name, message, _) = error else {
        return ServiceError::Failed(
            crate::i18n::tr("The mount service is not available").to_owned(),
        );
    };
    method_error(name.as_str(), message.unwrap_or_default())
}

// A D-Bus error reply as the prompt or message it calls for.
fn method_error(name: &str, message: String) -> ServiceError {
    let Some(kind) = name.strip_prefix(ERROR_PREFIX) else {
        return ServiceError::Failed(
            crate::i18n::tr("The mount service could not complete the operation").to_owned(),
        );
    };
    let malformed = || {
        ServiceError::Failed(crate::i18n::tr("The storage service answered nonsense").to_owned())
    };
    match kind {
        "NeedsCredentials" => serde_json::from_str(&message)
            .map_or_else(|_| malformed(), ServiceError::NeedsCredentials),
        "HostKeyUnknown" | "HostKeyChanged" => serde_json::from_str(&message).map_or_else(
            |_| malformed(),
            |request: HostKeyRequest| {
                ServiceError::HostKey(HostKeyRequest {
                    changed: kind == "HostKeyChanged",
                    fingerprints: request.fingerprints.iter().map(|f| bounded(f)).collect(),
                    host: bounded(&request.host),
                    ..request
                })
            },
        ),
        "ChooseShare" => serde_json::from_str(&message).map_or_else(
            |_| malformed(),
            |request: ShareRequest| {
                ServiceError::ChooseShare(ShareRequest {
                    shares: request
                        .shares
                        .into_iter()
                        .filter(|share| !share.uri.is_empty())
                        .map(|share| ShareChoice {
                            name: bounded(&share.name),
                            comment: bounded(&share.comment),
                            uri: share.uri,
                        })
                        .collect(),
                    ..request
                })
            },
        ),
        "ProtocolMismatch" => ServiceError::ProtocolMismatch(bounded(&message)),
        "AuthFailed" => ServiceError::AuthFailed(bounded(&message)),
        "Busy" => ServiceError::Busy(bounded(&message)),
        "NotAuthorized" => ServiceError::NotAuthorized(bounded(&message)),
        "Timeout" => ServiceError::Timeout(bounded(&message)),
        _ => ServiceError::Failed(bounded(&message)),
    }
}

// Service text as one bounded line without control characters.
fn bounded(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MESSAGE_LIMIT)
        .collect::<String>()
        .trim()
        .to_owned()
}

async fn call<B, R>(method: &'static str, body: &B) -> Result<R, ServiceError>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let proxy = proxy().await.map_err(classify)?;
    proxy.call(method, body).await.map_err(classify)
}

// Mounts (`unmount` false) and returns the folder, or unmounts without force.
pub(crate) async fn operate(id: String, unmount: bool) -> Result<Option<PathBuf>, ServiceError> {
    if unmount {
        self::unmount(id, false).await.map(|()| None)
    } else {
        mount(id).await.map(Some)
    }
}

// Unmounts; `force` detaches even with open files or a dead server.
pub(crate) async fn unmount(id: String, force: bool) -> Result<(), ServiceError> {
    call::<_, ()>("Unmount", &(id.as_str(), force)).await
}

pub(crate) async fn mount(id: String) -> Result<PathBuf, ServiceError> {
    call::<_, String>("Mount", &(id.as_str(),))
        .await
        .map(PathBuf::from)
}

// Unmounts every volume of the drive, then ejects it (`power_off` false) or
// powers it off.
pub(crate) async fn remove_drive(id: String, power_off: bool) -> Result<(), ServiceError> {
    let method = if power_off { "SafelyRemove" } else { "Eject" };
    call::<_, ()>(method, &(id.as_str(),)).await
}

pub(crate) async fn mount_image(path: PathBuf) -> Result<PathBuf, ServiceError> {
    let Some(path) = path.to_str() else {
        return Err(ServiceError::Failed(
            crate::i18n::tr("The image path is not valid text").to_owned(),
        ));
    };
    call::<_, String>("MountImage", &(path,))
        .await
        .map(PathBuf::from)
}

pub(crate) async fn unlock(id: String, passphrase: SecretText) -> Result<PathBuf, ServiceError> {
    let secret = sealed(passphrase.expose_secret().as_bytes())?;
    drop(passphrase);
    call::<_, String>("Unlock", &(id.as_str(), zbus::zvariant::Fd::from(&secret)))
        .await
        .map(PathBuf::from)
}

pub(crate) async fn mount_with_credentials(
    id: String,
    username: String,
    domain: String,
    password: SecretText,
    remember: bool,
) -> Result<PathBuf, ServiceError> {
    let secret = sealed(password.expose_secret().as_bytes())?;
    drop(password);
    call::<_, String>(
        "MountWithCredentials",
        &(
            id.as_str(),
            username.as_str(),
            domain.as_str(),
            zbus::zvariant::Fd::from(&secret),
            remember,
        ),
    )
    .await
    .map(PathBuf::from)
}

pub(crate) async fn trust_host_key(
    id: String,
    fingerprints: Vec<String>,
) -> Result<(), ServiceError> {
    call::<_, ()>("TrustHostKey", &(id.as_str(), fingerprints)).await
}

// Adds (or finds) the saved network location for `uri` and returns its id.
pub(crate) async fn add_location(uri: String, auto_connect: bool) -> Result<String, ServiceError> {
    let mut options = std::collections::HashMap::new();
    options.insert("save", zbus::zvariant::Value::from(true));
    options.insert("auto_connect", zbus::zvariant::Value::from(auto_connect));
    call::<_, String>("AddLocation", &(uri.as_str(), "", options)).await
}

// One change to a saved network location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocationUpdate {
    Label(String),
    AutoConnect(bool),
    ForgetPassword,
    // Allow SMB1 for a server that speaks nothing newer.
    Smb1,
}

pub(crate) async fn update_location(
    id: String,
    update: LocationUpdate,
) -> Result<(), ServiceError> {
    use zbus::zvariant::Value;
    let mut options = std::collections::HashMap::new();
    match update {
        LocationUpdate::Label(label) => options.insert("label", Value::from(label)),
        LocationUpdate::AutoConnect(enabled) => {
            options.insert("auto_connect", Value::from(enabled))
        }
        LocationUpdate::ForgetPassword => options.insert("forget_password", Value::from(true)),
        LocationUpdate::Smb1 => options.insert("smb_version", Value::from("1.0")),
    };
    call::<_, ()>("UpdateLocation", &(id.as_str(), options)).await
}

// Mounts an SMB or NFS location at startup, or stops doing so. Enabling asks
// for administrator approval; without `password` the remembered one is used.
pub(crate) async fn set_mount_at_boot(
    id: String,
    enabled: bool,
    password: Option<SecretText>,
) -> Result<(), ServiceError> {
    let secret = sealed(
        password
            .as_ref()
            .map_or(&[][..], |p| p.expose_secret().as_bytes()),
    )?;
    drop(password);
    call::<_, ()>(
        "SetMountAtBoot",
        &(id.as_str(), enabled, zbus::zvariant::Fd::from(&secret)),
    )
    .await
}

pub(crate) async fn remove_location(id: String) -> Result<(), ServiceError> {
    call::<_, ()>("RemoveLocation", &(id.as_str(),)).await
}

// What the person typed to log in to a network location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Login {
    pub(crate) username: String,
    pub(crate) domain: String,
    pub(crate) password: SecretText,
}

// The person's choices for a connection, carried through every prompt the
// service raises on the way (password, host key, share, SMB1).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ConnectPlan {
    pub(crate) login: Option<Login>,
    pub(crate) remember: bool,
    pub(crate) auto_connect: bool,
    pub(crate) mount_at_boot: bool,
    // A server location this connection was started from, removed once a
    // share of it is mounted.
    pub(crate) replaces_server: Option<String>,
    // The connection saved a location that was not there before.
    pub(crate) new_location: bool,
}

// Mounts `id` as planned: with the typed password if there is one, then
// mounting at startup when asked. A failure to set up the startup mount does
// not undo the mount; it is returned beside the folder.
pub(crate) async fn mount_planned(
    id: String,
    plan: ConnectPlan,
) -> Result<(PathBuf, Option<ServiceError>), ServiceError> {
    let typed = plan.login.filter(|login| !login.password.is_empty());
    let path = match &typed {
        Some(login) => {
            mount_with_credentials(
                id.clone(),
                login.username.clone(),
                login.domain.clone(),
                login.password.clone(),
                plan.remember,
            )
            .await?
        }
        None => mount(id.clone()).await?,
    };
    if let Some(server) = plan.replaces_server.filter(|server| *server != id) {
        // The bare server entry only led here; a share mount supersedes it.
        let _ = remove_location(server).await;
    }
    let boot = if plan.mount_at_boot {
        // The typed password, when there is one: a password being remembered
        // may not have reached the keyring yet.
        set_mount_at_boot(id, true, typed.map(|login| login.password))
            .await
            .err()
    } else {
        None
    };
    Ok((path, boot))
}

// SMB servers found by mDNS and WS-Discovery; mounting one lists its shares.
pub(crate) async fn discover() -> Result<Vec<crate::network::DiscoveredShare>, ServiceError> {
    #[derive(serde::Deserialize)]
    struct Found {
        uri: String,
    }
    let found: String = call("Discover", &()).await?;
    let found: Vec<Found> = serde_json::from_str(&found).map_err(|_| {
        ServiceError::Failed(crate::i18n::tr("The storage service answered nonsense").to_owned())
    })?;
    let mut servers: Vec<crate::network::DiscoveredShare> = found
        .iter()
        .filter_map(|found| found.uri.strip_prefix("smb://"))
        .filter_map(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty() && !host.starts_with('-'))
        .map(|host| crate::network::DiscoveredShare {
            host: bounded(host),
            share: String::new(),
            needs_auth: false,
        })
        .collect();
    servers.sort_by(|a, b| a.host.cmp(&b.host));
    servers.dedup();
    Ok(servers)
}

// Whether `address` names an SMB share or an NFS export, which the service
// can mount at startup.
pub(crate) fn can_mount_at_boot(address: &str) -> bool {
    let address = address.trim();
    if address.starts_with("nfs://") {
        return true;
    }
    let path = if let Some(rest) = address.strip_prefix("smb://") {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
        rest.split('/').filter(|part| !part.is_empty()).count()
    } else if let Some(rest) = address.strip_prefix("\\\\") {
        rest.split('\\').filter(|part| !part.is_empty()).count()
    } else {
        0
    };
    path >= 2
}

// Runs a service future on a private runtime, for the blocking callers that
// already run on worker threads.
pub(crate) fn block_on<T>(
    future: impl std::future::Future<Output = Result<T, ServiceError>>,
) -> Result<T, ServiceError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            ServiceError::Failed(crate::i18n::tr("The mount service is not available").to_owned())
        })?
        .block_on(future)
}

// The current inventory, for blocking callers.
pub(crate) fn snapshot_blocking() -> Result<Vec<Location>, String> {
    block_on(async {
        let snapshot: String = call("List", &()).await?;
        locations(&snapshot).map_err(ServiceError::Failed)
    })
    .map_err(|error| error.to_string())
}

// A secret in an anonymous memory file sealed against writing, growing and
// shrinking, as the service requires; an empty file means no password.
fn sealed(secret: &[u8]) -> Result<std::os::fd::OwnedFd, ServiceError> {
    use rustix::fs::{MemfdFlags, SealFlags};
    use std::io::{Seek, SeekFrom, Write};
    let failed =
        || ServiceError::Failed(crate::i18n::tr("The password could not be passed on").to_owned());
    if secret.len() > SECRET_LIMIT {
        return Err(failed());
    }
    let fd = rustix::fs::memfd_create(
        "files-mount-secret",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map_err(|_| failed())?;
    let mut file = std::fs::File::from(fd);
    file.write_all(secret).map_err(|_| failed())?;
    file.seek(SeekFrom::Start(0)).map_err(|_| failed())?;
    rustix::fs::fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|_| failed())?;
    Ok(std::os::fd::OwnedFd::from(file))
}

// The passphrase prompt for a locked encrypted drive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnlockState {
    // The storage id of the drive.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) passphrase: SecretText,
    pub(crate) revealed: bool,
    // The service is unlocking the drive.
    pub(crate) busy: bool,
    // Why the last passphrase was refused.
    pub(crate) error: Option<String>,
}

impl UnlockState {
    pub(crate) fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            passphrase: SecretText::new(String::new()),
            revealed: false,
            busy: false,
            error: None,
        }
    }
}

// Fills the attach form so the person can answer `NeedsCredentials` for the
// existing location `storage_id`. The address is shown for reference; the
// answer goes to that location by id, carrying the choices in `carry`.
pub(crate) fn credential_form(
    storage_id: String,
    name: &str,
    uri: Option<&str>,
    request: &CredentialRequest,
    generation: u64,
    purpose: crate::network::TargetPurpose,
    carry: ConnectPlan,
) -> crate::network::AttachState {
    use crate::network::{NetworkProtocol, PasswordRequirement};
    let uri = uri.unwrap_or_default();
    let (scheme, rest) = uri.split_once("://").unwrap_or(("smb", uri));
    let host = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    crate::network::AttachState {
        protocol: match scheme {
            "nfs" => NetworkProtocol::Nfs,
            "sftp" => NetworkProtocol::Sftp,
            "dav" | "davs" => NetworkProtocol::Webdav,
            _ => NetworkProtocol::Smb,
        },
        server: if uri.is_empty() {
            String::new()
        } else {
            format!("{scheme}://{host}")
        },
        domain: request.domain.clone(),
        username: request.username.clone(),
        password_requirement: PasswordRequirement::Required,
        remember_password: carry.remember,
        generation,
        service: true,
        service_target: Some(crate::network::ServiceTarget {
            id: storage_id,
            name: name.to_owned(),
            retry: request.retry,
            can_remember: request.can_remember,
            purpose,
            carry,
        }),
        ..crate::network::AttachState::default()
    }
}

// A command for one mount service location.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocationAction {
    // Mount if needed, then open.
    Open,
    // Ask for the passphrase of a locked encrypted drive.
    Unlock,
    Unmount,
    // Unmount every volume of the drive, then power it off or eject it.
    SafelyRemove,
    // Erase the volume with a new file system.
    Format,
    // Show where a cloud drive's browser sign-in stands.
    ShowSignIn,
    // Stop a cloud drive's browser sign-in.
    CancelSignIn,
    Connect,
    Disconnect,
    // Detach a mount whose server stopped answering.
    ForceDisconnect,
    Rename,
    SetAutoConnect(bool),
    SetMountAtBoot(bool),
    ForgetPassword,
    Remove,
}

impl LocationAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Open => crate::i18n::tr("Open"),
            Self::Unlock => crate::i18n::tr("Unlock…"),
            Self::Unmount => crate::i18n::tr("Unmount"),
            Self::SafelyRemove => crate::i18n::tr("Safely Remove"),
            Self::Format => crate::i18n::tr("Format…"),
            Self::ShowSignIn => crate::i18n::tr("Show Sign-in"),
            Self::CancelSignIn => crate::i18n::tr("Cancel Sign-in"),
            Self::Connect => crate::i18n::tr("Connect"),
            Self::Disconnect => crate::i18n::tr("Disconnect"),
            Self::ForceDisconnect => crate::i18n::tr("Disconnect Anyway"),
            Self::Rename => crate::i18n::tr("Rename…"),
            Self::SetAutoConnect(_) => crate::i18n::tr("Connect at Login"),
            Self::SetMountAtBoot(_) => crate::i18n::tr("Mount at Startup"),
            Self::ForgetPassword => crate::i18n::tr("Forget Password"),
            Self::Remove => crate::i18n::tr("Remove…"),
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Open => "folder-open",
            Self::Unlock => "lock-open",
            Self::Unmount => "arrow-up-from-line",
            Self::SafelyRemove => "usb",
            Self::Format => "eraser",
            Self::ShowSignIn => "globe",
            Self::CancelSignIn => "x",
            Self::Connect => "plug",
            Self::Disconnect | Self::ForceDisconnect => "unplug",
            Self::Rename => "pencil",
            Self::SetAutoConnect(_) => "log-in",
            Self::SetMountAtBoot(_) => "power",
            Self::ForgetPassword => "key-round",
            Self::Remove => "trash-2",
        }
    }

    // Commands that change the connection; the menu puts them first.
    pub(crate) fn connects(self) -> bool {
        matches!(
            self,
            Self::Open
                | Self::Unlock
                | Self::Unmount
                | Self::SafelyRemove
                | Self::ShowSignIn
                | Self::CancelSignIn
                | Self::Connect
                | Self::Disconnect
                | Self::ForceDisconnect
        )
    }
}

// The context menu commands for a mount service location, in menu order;
// toggles carry the value they switch to.
pub(crate) fn location_actions(location: &Location) -> Vec<LocationAction> {
    let Some(service) = location.service.as_ref() else {
        return Vec::new();
    };
    match location.kind {
        Kind::Network => network_actions(location, service),
        Kind::Cloud => cloud_actions(location, service),
        _ => drive_actions(location, service),
    }
}

// Drives open, unmount and come out safely; removable volumes can be
// formatted while idle; a locked drive is unlocked first.
fn drive_actions(location: &Location, service: &ServiceItem) -> Vec<LocationAction> {
    let mut actions = Vec::new();
    if service.formatting || service.busy {
        return actions;
    }
    if service.locked {
        actions.push(LocationAction::Unlock);
    } else if location.mount.is_some() || location.can_mount {
        actions.push(LocationAction::Open);
    }
    if location.can_unmount {
        actions.push(LocationAction::Unmount);
    }
    if service.can_power_off || location.can_eject {
        actions.push(LocationAction::SafelyRemove);
    }
    if service.can_format {
        actions.push(LocationAction::Format);
    }
    actions
}

// Cloud drives keep their account in the service; only connecting and
// removing apply. A sign-in in progress can be shown or cancelled.
fn cloud_actions(location: &Location, service: &ServiceItem) -> Vec<LocationAction> {
    if service.signing_in {
        return vec![LocationAction::ShowSignIn, LocationAction::CancelSignIn];
    }
    let mut actions = connection_actions(location, service);
    actions.push(LocationAction::Remove);
    actions
}

fn connection_actions(location: &Location, service: &ServiceItem) -> Vec<LocationAction> {
    if service.busy {
        Vec::new()
    } else if service.not_responding {
        vec![LocationAction::ForceDisconnect]
    } else if location.mount.is_some() {
        vec![LocationAction::Open, LocationAction::Disconnect]
    } else {
        vec![LocationAction::Connect]
    }
}

fn network_actions(location: &Location, service: &ServiceItem) -> Vec<LocationAction> {
    let mut actions = connection_actions(location, service);
    actions.push(LocationAction::Rename);
    actions.push(LocationAction::SetAutoConnect(!service.auto_connect));
    if service.can_mount_at_boot || service.mount_at_boot {
        actions.push(LocationAction::SetMountAtBoot(!service.mount_at_boot));
    }
    actions.push(LocationAction::ForgetPassword);
    actions.push(LocationAction::Remove);
    actions
}

// A small dialog about one network location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocationDialog {
    Rename {
        id: String,
        name: String,
    },
    // The server location `server` has these shares; picking one connects
    // to it with `plan`.
    ChooseShare {
        server: String,
        name: String,
        shares: Vec<ShareChoice>,
        plan: ConnectPlan,
    },
    Confirm {
        id: String,
        name: String,
        confirmation: Confirmation,
    },
}

// What a confirmation dialog asks before acting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Confirmation {
    // Allow SMB1 for this server, then mount it with `plan`.
    Smb1 { plan: ConnectPlan },
    // Files are open: detach anyway, then finish `then`.
    Force { message: String, then: AfterForce },
    Remove,
}

// What follows a forced unmount.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AfterForce {
    Nothing,
    Eject,
    PowerOff,
}

mod cloud;
mod format;
#[cfg(test)]
mod tests;
