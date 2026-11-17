use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) const API: u32 = files_extension_sdk::API;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub(crate) schema: Option<String>,
    pub(crate) api: u32,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) author: String,
    pub(crate) description: String,
    pub(crate) worker: Worker,
    #[serde(default)]
    pub(crate) permissions: Vec<Permission>,
    pub(crate) actions: Vec<Action>,
    #[serde(default)]
    pub(crate) settings: Vec<Setting>,
}

pub(crate) use enzui_services::extension_package::WorkerSpec as Worker;
pub(crate) use enzui_services::extension_package::{SettingKind, SettingSpec as Setting};
pub(crate) use enzui_services::extension_package::{identifier, plain_relative as relative};

pub(super) fn host() -> enzui_services::extension_package::Host {
    enzui_services::extension_package::Host {
        api: API..=API,
        permissions: &["clipboard", "navigate"],
        builtins: &["checksum"],
        reserved_prefixes: &["org.enzui."],
        reserved_names: &[],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Permission {
    Clipboard,
    Navigate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Action {
    pub(crate) id: String,
    pub(crate) label: String,
    #[serde(default = "default_icon")]
    pub(crate) icon: String,
    pub(crate) scope: Scope,
    #[serde(default)]
    pub(crate) mime: Vec<String>,
    #[serde(default = "one")]
    pub(crate) min_selection: usize,
    #[serde(default = "one")]
    pub(crate) max_selection: usize,
}
fn default_icon() -> String {
    "puzzle".into()
}
fn one() -> usize {
    1
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Scope {
    Files,
    Folders,
    Selection,
    Background,
}

fn text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

impl Manifest {
    pub(super) fn shared(&self) -> enzui_services::extension_package::Manifest<Vec<Action>> {
        enzui_services::extension_package::Manifest {
            schema: self.schema.clone(),
            api: self.api,
            id: self.id.clone(),
            version: self.version.clone(),
            name: self.name.clone(),
            author: self.author.clone(),
            description: self.description.clone(),
            worker: self.worker.clone(),
            permissions: self
                .permissions
                .iter()
                .map(|permission| match permission {
                    Permission::Clipboard => "clipboard".into(),
                    Permission::Navigate => "navigate".into(),
                })
                .collect(),
            settings: self.settings.clone(),
            contributes: self.actions.clone(),
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 65536 {
            return Err(crate::i18n::tr("The extension manifest is too large").into());
        }
        let manifest: Self = serde_json::from_slice(bytes).map_err(|e| {
            crate::i18n::format(
                "files-invalid-extension-manifest-e-6c3314e4",
                &[("e", (e).to_string())],
            )
        })?;
        manifest.validate()?;
        Ok(manifest)
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.api == 0
            || self.version.len() > 128
            || self.schema.as_ref().is_some_and(|s| !text(s, 512))
            || !identifier(&self.id)
            || !text(&self.name, 80)
            || !text(&self.author, 120)
            || !text(&self.description, 512)
            || semver::Version::parse(&self.version).is_err()
        {
            return Err(crate::i18n::tr(
                "Invalid extension identity, description or semantic version",
            )
            .into());
        }
        match &self.worker {
            Worker::Native { program, target }
                if relative(Path::new(program))
                    && program.len() <= 240
                    && text(target, 64)
                    && program != "manifest.json"
                    && program != "state.json" => {}
            Worker::Builtin { id } if id == "checksum" => {}
            _ => return Err(crate::i18n::tr("Invalid extension worker").into()),
        }
        if self.permissions.len() > 2
            || self.permissions.iter().collect::<BTreeSet<_>>().len() != self.permissions.len()
            || self.actions.is_empty()
            || self.actions.len() > 16
            || self.settings.len() > 16
        {
            return Err(crate::i18n::tr(
                "Invalid extension contribution count or duplicate permission",
            )
            .into());
        }
        if matches!(self.worker, Worker::Builtin { .. })
            && self
                .actions
                .iter()
                .any(|a| a.id != "sha256" || a.scope != Scope::Files)
        {
            return Err(crate::i18n::tr(
                "The checksum worker only supports its file checksum action",
            )
            .into());
        }
        let mut ids = BTreeSet::new();
        for a in &self.actions {
            if !identifier(&a.id)
                || !ids.insert(&a.id)
                || !text(&a.label, 80)
                || !identifier(&a.icon)
                || a.max_selection > 128
                || a.min_selection > a.max_selection
                || (a.scope != Scope::Background && a.min_selection == 0)
                || a.mime.len() > 16
            {
                return Err(crate::i18n::tr("Invalid or duplicate extension action").into());
            }
            for mime in &a.mime {
                let Some((major, minor)) = mime.split_once('/') else {
                    return Err(crate::i18n::tr("Invalid action MIME filter").into());
                };
                if !text(mime, 100)
                    || !identifier(major)
                    || !(minor == "*"
                        || minor
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b".-+_".contains(&b)))
                    || minor.is_empty()
                {
                    return Err(crate::i18n::tr("Invalid action MIME filter").into());
                }
            }
        }
        ids.clear();
        for setting in &self.settings {
            if !identifier(&setting.id)
                || !ids.insert(&setting.id)
                || !text(&setting.label, 80)
                || !setting.accepts(&setting.default_value())
            {
                return Err(crate::i18n::tr("Invalid or duplicate extension setting").into());
            }
            if let SettingKind::Choice { choices, .. } = &setting.value
                && (choices.is_empty()
                    || choices.len() > 16
                    || choices.iter().any(|s| !text(s, 80))
                    || choices.iter().collect::<BTreeSet<_>>().len() != choices.len())
            {
                return Err(crate::i18n::tr("Invalid setting choices").into());
            }
        }
        Ok(())
    }
    pub(crate) fn compatibility(&self) -> Option<String> {
        if self.api != API {
            return Some(crate::i18n::format(
                "files-requires-extension-api-this-app-supports-c844b752",
                &[("arg0", (self.api).to_string()), ("API", (API).to_string())],
            ));
        }
        if let Worker::Native { target, .. } = &self.worker
            && target != &format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        {
            return Some(crate::i18n::format(
                "files-built-for-target-install-a-version-4085577e",
                &[
                    ("target", (target).clone()),
                    ("arg0", (std::env::consts::OS).to_owned()),
                    ("arg1", (std::env::consts::ARCH).to_owned()),
                ],
            ));
        }
        None
    }
}
