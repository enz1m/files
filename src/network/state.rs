// Network UI state: backend health, the attach flow, discovery, and the
// app network state.

use super::{
    DiscoveredShare, DiscoveryContext, MountCredentials, Mounter, NetworkProtocol,
    NetworkTrustStatus, PasswordRequirement, ScanResult,
};
use enzui_core::form::SecretText;

// The health of the network backend (gvfs / gio), shown as "Backend Status".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackendStatus {
    // The backend responded and is ready to mount.
    Ready,
    // The backend is enumerating / mounting.
    Working,
    // The backend is unavailable; set when a discovery scan fails.
    Unavailable,
}

impl BackendStatus {
    pub(crate) fn status_line(self) -> String {
        let state = match self {
            BackendStatus::Ready => crate::i18n::tr("Ready"),
            BackendStatus::Working => crate::i18n::tr("Working"),
            BackendStatus::Unavailable => crate::i18n::tr("Unavailable"),
        };
        crate::i18n::format(
            "files-backend-status-state-6068ce81",
            &[("state", (state).to_owned())],
        )
    }
}

// The Attach-network-location dialog state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttachState {
    pub(crate) protocol: NetworkProtocol,
    // The server host / address.
    pub(crate) server: String,
    // The share / path on the server.
    pub(crate) share: String,
    // The optional domain / workgroup (SMB only).
    pub(crate) domain: String,
    // The username.
    pub(crate) username: String,
    // The password, a wipe-on-drop [`enzui_core::form::SecretText`]: closing
    // the dialog drops it, and it is redacted in `Debug`/`Display`.
    //
    // Rendered by borrowing [`SecretText::expose_secret`] for one view pass; never
    // logged or placed in a URI or argv.
    pub(crate) password: SecretText,
    // Whether the password field is revealed; masked by default.
    pub(crate) password_revealed: bool,
    // Whether to keep the password in the keyring; off by default.
    pub(crate) remember_password: bool,
    // Whether the server requires a password (drives the field label).
    pub(crate) password_requirement: PasswordRequirement,
    // Dialog-session id threaded into every [`MountCredentials`], so a backend can
    // refuse callbacks from a closed or reopened dialog.
    pub(crate) generation: u64,
    // Discovered shares from the last Discover run.
    pub(crate) discovered: Vec<DiscoveredShare>,
    // The backend health.
    pub(crate) backend: BackendStatus,
    // The outcome of the last Discover run; an empty network and a broken scanner
    // are distinct states.
    pub(crate) discovery: DiscoveryState,
    pub(crate) context: DiscoveryContext,
    // Set when the form answers the mount service's password request for an
    // existing location, which is then mounted by id rather than by address.
    pub(crate) service_target: Option<ServiceTarget>,
    // The enzimOS mount service connects: the server field also takes a full
    // address, and the location options below apply.
    pub(crate) service: bool,
    // Reconnect when the person logs in (saves the location).
    pub(crate) auto_connect: bool,
    // Also mount at startup, before login (SMB shares and NFS only).
    pub(crate) mount_at_boot: bool,
}

// The mount service location a credential form answers for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ServiceTarget {
    // The storage id of the location.
    pub(crate) id: String,
    // What the sign-in dialog calls the location.
    pub(crate) name: String,
    // The last password was refused.
    pub(crate) retry: bool,
    // The service can keep the password in the keyring.
    pub(crate) can_remember: bool,
    pub(crate) purpose: TargetPurpose,
    // The choices of the connection that asked, kept for the retry.
    pub(crate) carry: crate::mount_service::ConnectPlan,
}

// What the password typed into a credential form is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetPurpose {
    Mount,
    // Turning on mount at startup for a location with no remembered password.
    MountAtBoot,
}

// The visible outcome of the last discovery scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiscoveryState {
    // Discover has not been pressed yet.
    Idle,
    // The scan ran and found no shares.
    Empty,
    // The scan ran and found the shares in [`AttachState::discovered`].
    Found,
    // The scan could not run; the string is the reason, shown to the user.
    Failed(String),
}

// State owned by the Network tab, separate from the attach dialog so closing the
// form keeps the LAN results.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NetworkState {
    pub(crate) discovered: Vec<DiscoveredShare>,
    pub(crate) discovery: DiscoveryState,
    pub(crate) context: DiscoveryContext,
    pub(crate) backend: BackendStatus,
    pub(crate) pending: bool,
    pub(crate) pending_mount: bool,
    pub(crate) operation_error: Option<String>,
}

impl Default for NetworkState {
    fn default() -> Self {
        Self {
            discovered: Vec::new(),
            discovery: DiscoveryState::Idle,
            context: DiscoveryContext::default(),
            backend: BackendStatus::Ready,
            pending: false,
            pending_mount: false,
            operation_error: None,
        }
    }
}

impl NetworkState {
    pub(crate) fn begin_scan(&mut self) {
        self.pending = true;
        self.pending_mount = false;
        self.backend = BackendStatus::Working;
        self.discovery = DiscoveryState::Idle;
        self.context = DiscoveryContext::default();
        self.operation_error = None;
    }

    pub(crate) fn finish_scan(&mut self, result: ScanResult, context: DiscoveryContext) {
        self.pending = false;
        self.pending_mount = false;
        self.context = context;
        match result {
            Ok(shares) if shares.is_empty() => {
                self.discovered.clear();
                self.discovery = DiscoveryState::Empty;
                self.backend = BackendStatus::Ready;
            }
            Ok(shares) => {
                self.discovered = super::unique_shares(shares);
                self.discovery = DiscoveryState::Found;
                self.backend = BackendStatus::Ready;
            }
            Err(reason) => {
                let is_truncation = reason.preserve_previous;
                if !is_truncation {
                    // A backend failure clears hosts that are genuinely gone.
                    self.discovered.clear();
                }
                self.discovery = DiscoveryState::Failed(reason.message);
                self.backend = if is_truncation {
                    BackendStatus::Ready
                } else {
                    BackendStatus::Unavailable
                };
            }
        }
    }
}

impl DiscoveryState {
    // The line for a scan that found nothing or failed; `None` when idle or listing.
    pub(crate) fn message_for(&self, trust: &NetworkTrustStatus) -> Option<String> {
        match self {
            DiscoveryState::Idle | DiscoveryState::Found => None,
            DiscoveryState::Empty => Some(trust.empty_description()),
            DiscoveryState::Failed(_) if trust.discovery_blocked() => {
                Some(trust.empty_description())
            }
            DiscoveryState::Failed(reason) => Some(reason.clone()),
        }
    }

    pub(crate) fn is_failure(&self) -> bool {
        matches!(self, DiscoveryState::Failed(_))
    }
}

impl Default for AttachState {
    fn default() -> Self {
        Self {
            protocol: NetworkProtocol::Smb,
            server: String::new(),
            share: String::new(),
            domain: String::new(),
            username: String::new(),
            password: SecretText::new(String::new()),
            password_revealed: false,
            remember_password: false,
            password_requirement: PasswordRequirement::Optional,
            generation: 0,
            discovered: Vec::new(),
            backend: BackendStatus::Ready,
            discovery: DiscoveryState::Idle,
            context: DiscoveryContext::default(),
            service_target: None,
            service: false,
            auto_connect: false,
            mount_at_boot: false,
        }
    }
}

impl AttachState {
    // The `scheme://[user@]server/share` URI to mount; `None` when server or share
    // is empty.
    pub(crate) fn mount_uri(&self) -> Option<String> {
        let server = self.server.trim();
        let share = self.share.trim();
        if server.is_empty() || share.is_empty() {
            return None;
        }
        let auth = super::uri_user_part(&self.username);
        let share = super::uri_share_part(share);
        Some(format!(
            "{}://{auth}{server}/{share}",
            self.protocol.scheme()
        ))
    }

    // The address the mount service connects to: a full address typed into
    // the server field as is, otherwise one composed from the form.
    pub(crate) fn service_address(&self) -> Option<String> {
        let server = self.server.trim();
        if server.is_empty() {
            return None;
        }
        if server.contains("://") || server.starts_with('\\') || server.contains(":/") {
            return Some(server.to_owned());
        }
        let share = self.share.trim().trim_matches('/');
        let user = self.username.trim();
        // `DOMAIN\user` and `user@domain` go to the service as the user name,
        // never into the address.
        let auth = if user.is_empty() || user.contains(['\\', '@', ';', '/', ':']) {
            String::new()
        } else {
            format!("{user}@")
        };
        let path = if share.is_empty() {
            String::new()
        } else {
            format!("/{share}")
        };
        Some(format!("{}://{auth}{server}{path}", self.protocol.scheme()))
    }

    // Whether the address can be mounted at startup.
    pub(crate) fn can_mount_at_boot(&self) -> bool {
        self.service_address()
            .is_some_and(|address| crate::mount_service::can_mount_at_boot(&address))
    }

    // Whether the form can be submitted, ignoring backend support; see
    // [`AttachState::backend_can_carry_password`].
    pub(crate) fn can_attach(&self) -> bool {
        if self.mount_uri().is_none() {
            return false;
        }
        self.password_requirement.allows_empty() || !self.password.expose_secret().trim().is_empty()
    }

    // Whether a typed password can reach `mounter`.
    //
    // An empty password always can. Otherwise
    // [`Mounter::supports_authenticated_mount`] must accept the current protocol,
    // or the dialog disables the password field rather than drop it.
    pub(crate) fn backend_can_carry_password(&self, mounter: &dyn Mounter) -> bool {
        if self.password.is_empty() {
            return true;
        }
        mounter.supports_authenticated_mount(self.protocol)
    }

    // Why the Attach button is disabled, or `None` when the form can submit:
    // `incomplete`, `password_required` or `backend_lacks_credentials`.
    pub(crate) fn attach_blocker(&self, mounter: &dyn Mounter) -> Option<&'static str> {
        if self.mount_uri().is_none() {
            return Some(crate::i18n::tr("Enter a server and share"));
        }
        if !self.password_requirement.allows_empty()
            && self.password.expose_secret().trim().is_empty()
        {
            return Some(crate::i18n::tr("Password required"));
        }
        if !self.backend_can_carry_password(mounter) {
            return Some(crate::i18n::tr(
                "Authenticated mounts unavailable: this build has no \
                 in-process GIO password-callback provider. Clear the \
                 password to mount anonymously, or install a backend \
                 that supports authenticated mounts.",
            ));
        }
        None
    }

    // Replaces the password with a fresh [`SecretText`], wiping the previous
    // allocation once its last handle drops.
    pub(crate) fn set_password(&mut self, plaintext: String) {
        self.password = SecretText::new(plaintext);
    }

    // Builds the typed credential handoff for the current form; the password
    // stays one shared [`SecretText`] allocation.
    pub(crate) fn mount_credentials(&self) -> MountCredentials {
        MountCredentials::new(
            self.generation,
            self.protocol,
            self.server.clone(),
            self.share.clone(),
            self.domain.clone(),
            self.username.clone(),
            self.password.clone(),
        )
        .remembering(self.remember_password)
    }

    pub(crate) fn adopt(&mut self, share: &DiscoveredShare) {
        self.protocol = NetworkProtocol::Smb;
        self.server.clone_from(&share.host);
        self.share.clone_from(&share.share);
        self.password_requirement = share.password_requirement();
    }
}
