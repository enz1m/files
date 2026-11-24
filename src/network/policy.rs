// Read-only network trust assessment and Settings deep-link handoff.

use iced::window::ActivationToken;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const SYSTEM1_NAME: &str = "org.enzimos.System1";
const SYSTEM1_PATH: &str = "/org/enzimos/System1";
const FIREWALLD_NAME: &str = "org.fedoraproject.FirewallD1";
const SETTINGS_NAME: &str = "dev.enzui.Settings";
const SETTINGS_PATH: &str = "/dev/enzui/Settings";
const SETTINGS_NETWORK_URI: &str = "enzui-settings:network";
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
// Settings may start from D-Bus activation and present its window first.
const SETTINGS_OPEN_TIMEOUT: Duration = Duration::from_secs(30);
const LAN_TYPES: [&str; 2] = ["802-11-wireless", "802-3-ethernet"];

// Read-only assessment of whether local network discovery is restricted by the
// host's network-trust policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum NetworkTrustStatus {
    // At least one active LAN connection is known untrusted under an active
    // firewall policy that blocks multicast discovery.
    Untrusted {
        connection: Option<String>,
    },
    // At least one active LAN connection is explicitly trusted.
    Trusted,
    // Policy could not be determined (other distro, firewalld inactive,
    // sandboxed without system-bus access, or no active LAN connection).
    #[default]
    Unknown,
}

impl NetworkTrustStatus {
    pub(crate) fn discovery_blocked(&self) -> bool {
        matches!(self, Self::Untrusted { .. })
    }

    pub(crate) fn empty_title(&self) -> &'static str {
        match self {
            Self::Untrusted { .. } => crate::i18n::tr("Network discovery is blocked"),
            Self::Trusted | Self::Unknown => crate::i18n::tr("No shares found"),
        }
    }

    pub(crate) fn empty_description(&self) -> String {
        match self {
            Self::Untrusted {
                connection: Some(name),
            } => format!(
                "“{name}” is set to Public, so automatic share discovery is blocked by the \
                 firewall. Switch this network to Trusted in Settings to find local shares, or \
                 connect directly by server address."
            ),
            Self::Untrusted { connection: None } => crate::i18n::tr(
                "This network is set to Public, so automatic share discovery is blocked by the \
                 firewall. Switch this network to Trusted in Settings to find local shares, or \
                 connect directly by server address.",
            )
            .to_owned(),
            Self::Trusted => crate::i18n::tr(
                "No network shares were advertised on this trusted network. You can still \
                 connect directly by server address.",
            )
            .to_owned(),
            Self::Unknown => crate::i18n::tr(
                "No network shares found on this network. Automatic discovery may be unavailable \
                 or restricted by your network or firewall; you can still connect directly by \
                 server address.",
            )
            .to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DiscoveryContext {
    pub(crate) trust: NetworkTrustStatus,
    pub(crate) can_open_network_settings: bool,
}

pub(crate) trait NetworkContextProbe: Send + Sync {
    fn probe(&self) -> DiscoveryContext;
}

pub(crate) type NetworkContextProbeHandle = Arc<dyn NetworkContextProbe>;

pub(crate) trait SettingsOpener: Send + Sync {
    fn open_network_settings(&self, token: Option<ActivationToken>) -> Result<(), String>;
    /// Opens Settings' network share backup set-up with location `id` chosen.
    fn open_backup_share(&self, id: &str, token: Option<ActivationToken>) -> Result<(), String> {
        let _ = (id, token);
        Err(crate::i18n::tr("Could not open Backup Settings.").to_owned())
    }
}

pub(crate) type SettingsOpenerHandle = Arc<dyn SettingsOpener>;

struct DbusNetworkContextProbe;

impl NetworkContextProbe for DbusNetworkContextProbe {
    fn probe(&self) -> DiscoveryContext {
        if cfg!(test) {
            return DiscoveryContext::default();
        }
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return DiscoveryContext::default();
        };
        runtime.block_on(probe_dbus_context())
    }
}

struct DbusSettingsOpener;

impl SettingsOpener for DbusSettingsOpener {
    fn open_network_settings(&self, token: Option<ActivationToken>) -> Result<(), String> {
        if cfg!(test) {
            return Err("Settings is unavailable in tests".to_owned());
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| crate::i18n::tr("Could not open Network Settings.").to_owned())?;
        runtime.block_on(open_network_settings_dbus(token))
    }
    fn open_backup_share(&self, id: &str, token: Option<ActivationToken>) -> Result<(), String> {
        let unavailable = || crate::i18n::tr("Could not open Backup Settings.").to_owned();
        if cfg!(test)
            || id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(unavailable());
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| unavailable())?;
        runtime
            .block_on(open_settings_uri(
                format!("enzui-settings:backups/destination?share={id}"),
                token,
            ))
            .map_err(|()| unavailable())
    }
}

pub(crate) fn default_context_probe() -> NetworkContextProbeHandle {
    Arc::new(DbusNetworkContextProbe)
}

pub(crate) fn default_settings_opener() -> SettingsOpenerHandle {
    Arc::new(DbusSettingsOpener)
}

// Only active LAN connections (`802-11-wireless`, `802-3-ethernet`) determine
// whether local multicast share discovery is allowed or blocked.
pub(crate) fn parse_system1_network_trust(raw: &str) -> NetworkTrustStatus {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(raw) else {
        return NetworkTrustStatus::Unknown;
    };
    let Some(connections) = root
        .get("result")
        .and_then(|result| result.get("connections"))
        .and_then(serde_json::Value::as_array)
    else {
        return NetworkTrustStatus::Unknown;
    };
    let mut active_lan_found = false;
    let mut first_untrusted_name: Option<String> = None;
    for row in connections.iter().take(256) {
        let active = row
            .get("active")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let kind = row
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !active || !LAN_TYPES.contains(&kind) {
            continue;
        }
        active_lan_found = true;
        if row
            .get("trusted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return NetworkTrustStatus::Trusted;
        }
        if first_untrusted_name.is_none() {
            first_untrusted_name = row
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
                .map(|name| name.chars().take(128).collect());
        }
    }
    if active_lan_found {
        NetworkTrustStatus::Untrusted {
            connection: first_untrusted_name,
        }
    } else {
        NetworkTrustStatus::Unknown
    }
}

async fn probe_dbus_context() -> DiscoveryContext {
    let (trust, can_open_network_settings) =
        tokio::join!(probe_network_trust(), detect_settings_service());
    DiscoveryContext {
        trust,
        can_open_network_settings,
    }
}

type FirewallBindings = HashMap<String, HashMap<String, Vec<String>>>;

async fn firewall_bindings(
    connection: &zbus::Connection,
    interface: &str,
    method: &str,
) -> Option<FirewallBindings> {
    let reply = tokio::time::timeout(
        PROBE_TIMEOUT,
        connection.call_method(
            Some(FIREWALLD_NAME),
            "/org/fedoraproject/FirewallD1",
            Some(interface),
            method,
            &(),
        ),
    )
    .await
    .ok()?
    .ok()?;
    reply.body().deserialize().ok()
}

fn active_network_trust(
    zones: &FirewallBindings,
    policies: &FirewallBindings,
) -> NetworkTrustStatus {
    let has_interfaces = |zone: &str| {
        zones
            .get(zone)
            .and_then(|bindings| bindings.get("interfaces"))
            .is_some_and(|interfaces| !interfaces.is_empty())
    };
    if policies.contains_key("enz-mdns-trust") && has_interfaces("enzimos-trusted") {
        NetworkTrustStatus::Trusted
    } else if policies.contains_key("enz-mdns-block") && has_interfaces("public") {
        NetworkTrustStatus::Untrusted { connection: None }
    } else {
        NetworkTrustStatus::Unknown
    }
}

async fn probe_network_trust() -> NetworkTrustStatus {
    let Ok(Ok(connection)) = tokio::time::timeout(PROBE_TIMEOUT, zbus::Connection::system()).await
    else {
        return NetworkTrustStatus::Unknown;
    };
    let Ok(bus) = zbus::fdo::DBusProxy::new(&connection).await else {
        return NetworkTrustStatus::Unknown;
    };
    let Ok(firewall) = zbus::names::BusName::try_from(FIREWALLD_NAME) else {
        return NetworkTrustStatus::Unknown;
    };
    if !matches!(
        tokio::time::timeout(PROBE_TIMEOUT, bus.name_has_owner(firewall)).await,
        Ok(Ok(true))
    ) {
        return NetworkTrustStatus::Unknown;
    }
    let (zones, policies) = tokio::join!(
        firewall_bindings(
            &connection,
            "org.fedoraproject.FirewallD1.zone",
            "getActiveZones"
        ),
        firewall_bindings(
            &connection,
            "org.fedoraproject.FirewallD1.policy",
            "getActivePolicies"
        ),
    );
    let (Some(zones), Some(policies)) = (zones, policies) else {
        return NetworkTrustStatus::Unknown;
    };
    let trust = active_network_trust(&zones, &policies);
    if trust.discovery_blocked()
        && let name @ NetworkTrustStatus::Untrusted {
            connection: Some(_),
        } = probe_system1_trust(&connection).await
    {
        return name;
    }
    trust
}

async fn probe_system1_trust(connection: &zbus::Connection) -> NetworkTrustStatus {
    let body = r#"{"schema":1,"query":"network-trust"}"#;
    let Ok(Ok(reply)) = tokio::time::timeout(
        PROBE_TIMEOUT,
        connection.call_method(
            Some(SYSTEM1_NAME),
            SYSTEM1_PATH,
            Some(SYSTEM1_NAME),
            "Query",
            &(body,),
        ),
    )
    .await
    else {
        return NetworkTrustStatus::Unknown;
    };
    let Ok(text) = reply.body().deserialize::<String>() else {
        return NetworkTrustStatus::Unknown;
    };
    parse_system1_network_trust(&text)
}

async fn detect_settings_service() -> bool {
    let Ok(Ok(connection)) = tokio::time::timeout(PROBE_TIMEOUT, zbus::Connection::session()).await
    else {
        return false;
    };
    let Ok(bus) = zbus::fdo::DBusProxy::new(&connection).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(SETTINGS_NAME) else {
        return false;
    };
    if tokio::time::timeout(PROBE_TIMEOUT, bus.name_has_owner(name))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
    {
        return true;
    }
    tokio::time::timeout(PROBE_TIMEOUT, bus.list_activatable_names())
        .await
        .ok()
        .and_then(Result::ok)
        .is_some_and(|names| names.iter().any(|known| known.as_str() == SETTINGS_NAME))
}

async fn open_settings_uri(uri: String, token: Option<ActivationToken>) -> Result<(), ()> {
    let connection = tokio::time::timeout(SETTINGS_OPEN_TIMEOUT, zbus::Connection::session())
        .await
        .map_err(drop)?
        .map_err(drop)?;
    let mut platform = HashMap::<&str, zbus::zvariant::Value<'_>>::new();
    if let Some(token) = &token {
        if let Some(value) = token.xdg_activation_token() {
            platform.insert("activation-token", value.into());
        }
        if let Some(value) = token.desktop_startup_id() {
            platform.insert("desktop-startup-id", value.into());
        }
    }
    tokio::time::timeout(
        SETTINGS_OPEN_TIMEOUT,
        connection.call_method(
            Some(SETTINGS_NAME),
            SETTINGS_PATH,
            Some("org.freedesktop.Application"),
            "Open",
            &(&[uri.as_str()][..], platform),
        ),
    )
    .await
    .map_err(drop)?
    .map(drop)
    .map_err(drop)
}

async fn open_network_settings_dbus(token: Option<ActivationToken>) -> Result<(), String> {
    let connection = tokio::time::timeout(OPEN_TIMEOUT, zbus::Connection::session())
        .await
        .map_err(|_| crate::i18n::tr("Could not reach Network Settings.").to_owned())?
        .map_err(|_| crate::i18n::tr("Could not reach Network Settings.").to_owned())?;
    let uris = [SETTINGS_NETWORK_URI];
    let mut platform = HashMap::<&str, zbus::zvariant::Value<'_>>::new();
    if let Some(token) = &token {
        if let Some(value) = token.xdg_activation_token() {
            platform.insert("activation-token", value.into());
        }
        if let Some(value) = token.desktop_startup_id() {
            platform.insert("desktop-startup-id", value.into());
        }
    }
    tokio::time::timeout(
        OPEN_TIMEOUT,
        connection.call_method(
            Some(SETTINGS_NAME),
            SETTINGS_PATH,
            Some("org.freedesktop.Application"),
            "Open",
            &(&uris[..], platform),
        ),
    )
    .await
    .map_err(|_| crate::i18n::tr("Network Settings did not respond in time.").to_owned())?
    .map(drop)
    .map_err(|_| crate::i18n::tr("Could not open Network Settings.").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::{StreamExt, executor::block_on};
    use iced_runtime::{Action, task, window};

    #[test]
    fn reports_public_discovery_block_only_when_the_policy_is_active() {
        let public = HashMap::from([(
            "public".to_owned(),
            HashMap::from([("interfaces".to_owned(), vec!["wlan0".to_owned()])]),
        )]);
        let mut policies = HashMap::new();
        assert_eq!(
            active_network_trust(&public, &policies),
            NetworkTrustStatus::Unknown
        );
        policies.insert("enz-mdns-block".to_owned(), HashMap::new());
        let trust = active_network_trust(&public, &policies);
        assert_eq!(trust, NetworkTrustStatus::Untrusted { connection: None });
        assert_eq!(
            crate::network::DiscoveryState::Failed("Could not scan".to_owned()).message_for(&trust),
            Some(trust.empty_description()),
        );
    }

    #[test]
    fn parses_untrusted_active_wifi_connection() {
        let reply = serde_json::json!({
            "schema": 1,
            "result": {
                "trusted_zone": "enzimos-trusted",
                "connections": [
                    {
                        "connection": "11111111-1111-1111-1111-111111111111",
                        "name": "CoffeeShop Wi-Fi",
                        "type": "802-11-wireless",
                        "active": true,
                        "device": "wlp2s0",
                        "zone": null,
                        "trusted": false
                    }
                ]
            }
        })
        .to_string();
        assert_eq!(
            parse_system1_network_trust(&reply),
            NetworkTrustStatus::Untrusted {
                connection: Some("CoffeeShop Wi-Fi".to_owned()),
            },
            "active untrusted Wi-Fi must report Untrusted with connection name"
        );
    }

    #[test]
    fn parses_trusted_active_lan_even_when_untrusted_vpn_or_inactive_profile_exists() {
        let reply = serde_json::json!({
            "schema": 1,
            "result": {
                "trusted_zone": "enzimos-trusted",
                "connections": [
                    {
                        "connection": "11111111-1111-1111-1111-111111111111",
                        "name": "Work VPN",
                        "type": "wireguard",
                        "active": true,
                        "device": "wg0",
                        "zone": null,
                        "trusted": false
                    },
                    {
                        "connection": "22222222-2222-2222-2222-222222222222",
                        "name": "Home Wi-Fi",
                        "type": "802-11-wireless",
                        "active": true,
                        "device": "wlp2s0",
                        "zone": "enzimos-trusted",
                        "trusted": true
                    },
                    {
                        "connection": "33333333-3333-3333-3333-333333333333",
                        "name": "Hotel",
                        "type": "802-11-wireless",
                        "active": false,
                        "device": null,
                        "zone": null,
                        "trusted": false
                    }
                ]
            }
        })
        .to_string();
        assert_eq!(
            parse_system1_network_trust(&reply),
            NetworkTrustStatus::Trusted,
            "an active trusted LAN connection must report Trusted"
        );
    }

    #[test]
    fn returns_unknown_when_no_active_lan_or_malformed_json() {
        let inactive_only = serde_json::json!({
            "schema": 1,
            "result": {
                "trusted_zone": "enzimos-trusted",
                "connections": [
                    {
                        "connection": "11111111-1111-1111-1111-111111111111",
                        "name": "Saved Wi-Fi",
                        "type": "802-11-wireless",
                        "active": false,
                        "device": null,
                        "zone": null,
                        "trusted": false
                    }
                ]
            }
        })
        .to_string();
        assert_eq!(
            parse_system1_network_trust(&inactive_only),
            NetworkTrustStatus::Unknown,
            "inactive connections must not claim the current network is untrusted"
        );
        assert_eq!(
            parse_system1_network_trust("not json"),
            NetworkTrustStatus::Unknown,
            "malformed JSON must fall back to Unknown"
        );
    }

    struct FixedScanner(Vec<crate::network::DiscoveredShare>);

    impl crate::network::ShareScanner for FixedScanner {
        fn scan(&self) -> crate::network::ScanResult {
            Ok(self.0.clone())
        }
    }

    struct FixedProbe {
        context: DiscoveryContext,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl NetworkContextProbe for FixedProbe {
        fn probe(&self) -> DiscoveryContext {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.context.clone()
        }
    }

    struct RecordingOpener {
        tokens: std::sync::Mutex<Vec<Option<ActivationToken>>>,
    }

    impl SettingsOpener for RecordingOpener {
        fn open_network_settings(&self, token: Option<ActivationToken>) -> Result<(), String> {
            self.tokens
                .lock()
                .map_err(|error| error.to_string())?
                .push(token);
            Ok(())
        }
    }

    #[test]
    fn empty_scan_distinguishes_untrusted_trusted_and_unknown_and_opens_settings() {
        let probe = Arc::new(FixedProbe {
            context: DiscoveryContext {
                trust: NetworkTrustStatus::Untrusted {
                    connection: Some("Studio Wi-Fi".to_owned()),
                },
                can_open_network_settings: true,
            },
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let opener = Arc::new(RecordingOpener {
            tokens: std::sync::Mutex::default(),
        });
        let (app, _) = crate::App::new();
        let mut app = app
            .with_scanner(Arc::new(FixedScanner(Vec::new())))
            .with_network_context_probe(Arc::clone(&probe) as NetworkContextProbeHandle)
            .with_settings_opener(Arc::clone(&opener) as SettingsOpenerHandle);

        let task = app.update(crate::Msg::DiscoverNetwork);
        crate::file_clipboard_tests::drive(&mut app, task);

        assert_eq!(
            app.model.network.discovery,
            crate::network::DiscoveryState::Empty,
            "empty scan settles in Empty state"
        );
        assert_eq!(
            app.model.network.context.trust,
            NetworkTrustStatus::Untrusted {
                connection: Some("Studio Wi-Fi".to_owned()),
            },
            "untrusted Wi-Fi context is preserved on empty scan"
        );
        assert!(
            app.model.network.context.can_open_network_settings,
            "Settings deep-link availability is preserved"
        );
        assert_eq!(
            app.model.network.context.trust.empty_title(),
            "Network discovery is blocked",
            "untrusted empty title explains policy block"
        );
        assert!(
            app.model
                .network
                .context
                .trust
                .empty_description()
                .contains("Studio Wi-Fi"),
            "untrusted description names the active public connection"
        );
        assert_eq!(
            probe.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "probe runs once for an empty scan"
        );

        let source = iced::window::Id::unique();
        app.main_window = Some(source);
        for reply in [
            Ok(ActivationToken::for_platform(
                iced::window::ActivationPlatform::Wayland,
                "files-click".into(),
            )),
            Err(iced::window::ActivationError::Unsupported),
        ] {
            let task = app.update(crate::Msg::OpenNetworkSettings);
            let mut actions = task::into_stream(task).unwrap();
            let Some(Action::Window(window::Action::RequestActivationToken(id, response))) =
                block_on(actions.next())
            else {
                panic!("opening Settings must request activation from Files");
            };
            assert_eq!(id, source);
            assert!(response.send(reply).is_ok());
            assert!(matches!(
                block_on(actions.next()),
                Some(Action::Output(crate::Msg::NetworkSettingsOpened(Ok(()))))
            ));
            assert!(block_on(actions.next()).is_none());
        }
        let tokens = opener.tokens.lock().unwrap();
        assert_eq!(tokens.len(), 2);
        assert_eq!(
            tokens[0]
                .as_ref()
                .and_then(ActivationToken::xdg_activation_token),
            Some("files-click")
        );
        assert!(tokens[1].is_none());
        drop(tokens);

        let attach_task = app.update(crate::Msg::ShowAttach);
        crate::file_clipboard_tests::drive(&mut app, attach_task);
        assert!(
            app.model.attach.is_some(),
            "direct Connect to Server remains available on untrusted networks"
        );

        let trusted_probe = Arc::new(FixedProbe {
            context: DiscoveryContext {
                trust: NetworkTrustStatus::Trusted,
                can_open_network_settings: true,
            },
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let (app_trusted, _) = crate::App::new();
        let mut app_trusted = app_trusted
            .with_scanner(Arc::new(FixedScanner(Vec::new())))
            .with_network_context_probe(trusted_probe);
        let task = app_trusted.update(crate::Msg::DiscoverNetwork);
        crate::file_clipboard_tests::drive(&mut app_trusted, task);
        assert_eq!(
            app_trusted.model.network.context.trust,
            NetworkTrustStatus::Trusted,
            "trusted empty scan stays Trusted"
        );
        assert_eq!(
            app_trusted.model.network.context.trust.empty_title(),
            "No shares found",
            "trusted empty scan keeps No shares found title"
        );
        assert!(
            app_trusted
                .model
                .network
                .context
                .trust
                .empty_description()
                .contains("trusted network"),
            "trusted empty scan distinguishes itself from blocked discovery"
        );

        let (app_unknown, _) = crate::App::new();
        let mut app_unknown = app_unknown.with_scanner(Arc::new(FixedScanner(Vec::new())));
        let task = app_unknown.update(crate::Msg::DiscoverNetwork);
        crate::file_clipboard_tests::drive(&mut app_unknown, task);
        assert_eq!(
            app_unknown.model.network.context,
            DiscoveryContext::default(),
            "unknown/other-distro scan defaults to Unknown with no Settings route"
        );
    }

    #[test]
    fn non_empty_scan_skips_trust_probe() {
        let probe = Arc::new(FixedProbe {
            context: DiscoveryContext {
                trust: NetworkTrustStatus::Untrusted { connection: None },
                can_open_network_settings: true,
            },
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let share = crate::network::DiscoveredShare {
            host: "nas.local".to_owned(),
            share: String::new(),
            needs_auth: false,
        };
        let (app, _) = crate::App::new();
        let mut app = app
            .with_scanner(Arc::new(FixedScanner(vec![share])))
            .with_network_context_probe(Arc::clone(&probe) as NetworkContextProbeHandle);

        let task = app.update(crate::Msg::DiscoverNetwork);
        crate::file_clipboard_tests::drive(&mut app, task);

        assert_eq!(
            app.model.network.discovery,
            crate::network::DiscoveryState::Found,
            "discovered shares still transition to Found"
        );
        assert_eq!(
            probe.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "trust probe is skipped when shares are found"
        );
    }
}
