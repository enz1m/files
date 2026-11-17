// Versioned file-action extensions, package management and native worker protocol.
mod manifest;
mod runtime;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use manifest::{Action, Manifest, Permission, Scope, SettingKind, Worker};
pub(crate) use runtime::{Response, helper_mode, request, run};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
pub(crate) use store::{Prepared, State, Store, load};

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug)]
pub(crate) struct Installed {
    pub(crate) id: String,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) state: State,
    pub(crate) bundled: bool,
    pub(crate) problem: Option<String>,
}
impl Installed {
    pub(crate) fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }
    pub(crate) fn display_name(&self) -> String {
        self.display_text(self.name())
    }
    pub(crate) fn display_text(&self, source: &str) -> String {
        if self.bundled {
            crate::i18n::text(source)
        } else {
            source.to_owned()
        }
    }
    pub(crate) fn active(&self) -> bool {
        self.state.enabled
            && self.problem.is_none()
            && self
                .manifest
                .as_ref()
                .is_some_and(|m| m.compatibility().is_none())
    }
    pub(crate) fn settings(&self) -> BTreeMap<String, Value> {
        self.manifest
            .as_ref()
            .map_or_else(BTreeMap::new, |manifest| {
                enzui_services::extension_package::resolve(&manifest.settings, &self.state.settings)
            })
    }
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Catalog {
    pub(crate) root: Option<PathBuf>,
    pub(crate) entries: Vec<Installed>,
    pub(crate) error: Option<String>,
    pub(crate) pending: Option<Arc<Prepared>>,
    pub(crate) remove: Option<String>,
    pub(crate) busy: bool,
    pub(crate) running: bool,
    pub(crate) expanded: Option<String>,
    pub(crate) drafts: BTreeMap<String, enzui_services::extension_package::SettingsDraft>,
}
impl Catalog {
    #[cfg(not(test))]
    pub(crate) fn root_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("files-app/extensions"))
    }
    pub(crate) fn actions<'a>(
        &'a self,
        entries: &[crate::model::FileEntry],
        background: bool,
    ) -> Vec<(&'a Installed, &'a Action)> {
        self.entries
            .iter()
            .filter(|e| e.active())
            .flat_map(|entry| {
                entry
                    .manifest
                    .as_ref()
                    .into_iter()
                    .flat_map(move |m| m.actions.iter().map(move |a| (entry, a)))
            })
            .filter(|(_, a)| matches_action(a, entries, background))
            .take(32)
            .collect()
    }
}
fn matches_action(action: &Action, entries: &[crate::model::FileEntry], background: bool) -> bool {
    if background {
        return action.scope == Scope::Background;
    }
    if action.scope == Scope::Background
        || !(action.min_selection..=action.max_selection).contains(&entries.len())
    {
        return false;
    }
    entries.iter().all(|entry| {
        let dir = entry.kind == crate::model::EntryKind::Dir;
        (action.scope == Scope::Selection
            || (action.scope == Scope::Files && !dir)
            || (action.scope == Scope::Folders && dir))
            && (action.mime.is_empty()
                || action.mime.iter().any(|m| {
                    m == &entry.mime
                        || m.strip_suffix('*')
                            .is_some_and(|prefix| entry.mime.starts_with(prefix))
                }))
    })
}

#[derive(Clone, Debug)]
pub(crate) enum Change {
    Install(Arc<Prepared>),
    Enable(String, bool),
    Settings(String, BTreeMap<String, Value>),
    Remove(String),
    Restore,
}
pub(crate) fn change(root: PathBuf, action: Change) -> Result<Catalog, String> {
    let store = Store::open(root.clone())?;
    match action {
        Change::Install(p) => store.install(&p)?,
        Change::Enable(id, value) => store.enable(&id, value)?,
        Change::Settings(id, settings) => store.settings(&id, settings)?,
        Change::Remove(id) => store.remove(&id)?,
        Change::Restore => store.restore()?,
    }
    Ok(load(root))
}
