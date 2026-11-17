// Files' enzui-center functions, behind the `center` feature.
//
// Navigation within what the window can already browse: list the open
// folder, open one of its sub-folders by name, go up, go home. There is no
// "read any path" function — an agent reaches a folder the way a user does,
// so a grant never widens what the app touches. Copying, moving, renaming,
// deleting and opening files are not exposed. Every navigation is an
// ordinary [`Msg`] through [`App::update`]; the listing that follows loads
// asynchronously, and `current_folder` reads it.

use std::path::Path;

#[cfg(not(test))]
use enzui_agent::center::CenterHost;
use enzui_agent::center::{
    AppFunction, AppFunctionRegistry, AppMetadata, CenterFailure, CenterOutcome, CenterTurn,
    ConnectorEvent, ConsentScope, FunctionRisk, FunctionSpec,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::model::EntryKind;

// The app id the center credential is enrolled under.
pub(crate) const CENTER_APP_ID: &str = crate::APP_ID;

pub(crate) const LIST_LIMIT: usize = 200;

pub(crate) const WAITING_CALLS_MAX: usize = 16;

// What an admitted call asks Files to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CenterCall {
    CurrentFolder { offset: usize, limit: usize },
    OpenFolder { name: String },
    GoUp,
    GoHome,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageArgs {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_page_limit")]
    limit: usize,
}

fn default_page_limit() -> usize {
    LIST_LIMIT
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

fn location_result() -> Value {
    enzui_agent::center::object_schema(json!({"folder": {"type": "string"}}), &["folder"])
}

/// The agent catalogue, always in English: an installed app's runtime
/// catalogue must match its packaged (English) manifest byte for byte, and its
/// descriptor hashes (which saved permissions are bound to) must not change
/// with the session's language.
pub(crate) fn registry() -> AppFunctionRegistry<CenterCall> {
    crate::i18n::for_locale(enzui_core::localization::Locale::default()).scope(localized_registry)
}

fn localized_registry() -> AppFunctionRegistry<CenterCall> {
    let once_or_session = vec![ConsentScope::Once, ConsentScope::Session];
    let current = FunctionSpec::new(
        "current_folder",
        crate::i18n::tr("Current folder"),
        crate::i18n::tr(
            "Lists the folder the window shows, as the view orders and filters it: name, \
         kind and size of each entry. Optional offset and limit page through the current \
         ordered view; next_offset is null at the end. Changes to the view may reorder pages.",
        ),
        enzui_agent::center::object_schema(
            json!({
                "offset": {"type": "integer", "minimum": 0, "maximum": 4294967295_u64},
                "limit": {"type": "integer", "minimum": 1, "maximum": LIST_LIMIT}
            }),
            &[],
        ),
        enzui_agent::center::object_schema(
            json!({
                "folder": {"type": "string"},
                "entries": {"type": "array", "items": enzui_agent::center::object_schema(json!({
                    "name": {"type": "string"},
                    "kind": {"type": "string", "enum": ["folder", "file", "link"]},
                    "size": {"type": "integer"}
                }), &["name", "kind", "size"])},
                "truncated": {"type": "boolean"},
                "next_offset": {"type": ["integer", "null"]},
                "total": {"type": "integer"}
            }),
            &["folder", "entries", "truncated", "next_offset", "total"],
        ),
        FunctionRisk::SensitiveRead,
        once_or_session.clone(),
    );
    let open = FunctionSpec::new(
        "open_folder",
        crate::i18n::tr("Open a sub-folder"),
        crate::i18n::tr("Opens the sub-folder of the current folder with this exact name."),
        enzui_agent::center::object_schema(
            json!({"name": {"type": "string", "minLength": 1, "maxLength": 255}}),
            &["name"],
        ),
        location_result(),
        FunctionRisk::Mutating,
        once_or_session.clone(),
    );
    let up = FunctionSpec::new(
        "go_up",
        crate::i18n::tr("Go up"),
        crate::i18n::tr("Opens the parent of the current folder."),
        enzui_agent::center::object_schema(json!({}), &[]),
        location_result(),
        FunctionRisk::Mutating,
        once_or_session.clone(),
    );
    let home = FunctionSpec::new(
        "go_home",
        crate::i18n::tr("Go home"),
        crate::i18n::tr("Opens the home folder."),
        enzui_agent::center::object_schema(json!({}), &[]),
        location_result(),
        FunctionRisk::Mutating,
        once_or_session,
    );
    AppFunctionRegistry::from_functions(vec![
        AppFunction::typed(current, |args: PageArgs| CenterCall::CurrentFolder {
            offset: args.offset,
            limit: args.limit,
        })
        .expect("current_folder spec"),
        AppFunction::typed(open, |args: NameArgs| CenterCall::OpenFolder {
            name: args.name,
        })
        .expect("open_folder spec"),
        AppFunction::new(up, |_| CenterCall::GoUp).expect("go_up spec"),
        AppFunction::new(home, |_| CenterCall::GoHome).expect("go_home spec"),
    ])
    .expect("the files registry is valid")
}

pub(crate) fn metadata() -> AppMetadata {
    // English for the same reason as the registry: it is part of the manifest.
    crate::i18n::for_locale(enzui_core::localization::Locale::default()).scope(|| AppMetadata {
        id: CENTER_APP_ID.into(),
        name: crate::i18n::tr("Files").into(),
        description: crate::i18n::tr("File manager").into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

// `path` as the user would name it: under home as `~/…`, else absolute.
pub(crate) fn display_folder(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

// A plain name of one directory entry: no separator, no `.`/`..`.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

impl App {
    // Connect to enzui-center when this build opted in and a credential is
    // enrolled for [`CENTER_APP_ID`]; otherwise stay unexposed.
    #[cfg(not(test))]
    pub(crate) fn start_center(&mut self) {
        let Some(provision) = enzui_agent::center::CenterProvision::discover(CENTER_APP_ID) else {
            return;
        };
        match CenterHost::spawn(provision, metadata(), registry()) {
            Ok(host) => self.center = Some(host),
            Err(error) => eprintln!("files-app: center connector disabled: {error}"),
        }
    }

    pub(crate) fn center_subscription(&self) -> Subscription<Msg> {
        self.center
            .as_ref()
            .map_or_else(Subscription::none, |host| host.subscription(Msg::Center))
    }

    // Final shutdown: refuse new calls, cancel live ones, join the connector.
    pub(crate) fn shutdown_center(&mut self) {
        if let Some(host) = self.center.take() {
            host.shutdown();
        }
    }

    pub(crate) fn center_event(&mut self, event: ConnectorEvent) -> Task<Msg> {
        let Some(host) = self.center.as_mut() else {
            return Task::none();
        };
        let update = host.receive_with_ends(event);
        self.waiting_center_calls.retain(|(call, _)| {
            !update
                .ended
                .iter()
                .any(|ended| ended.call == call.reference())
        });
        let CenterTurn::Call(call, request) = update.turn else {
            return Task::none();
        };
        // A listing read while a folder is still loading waits for
        // that load. A cold-started worker is asked before its first listing
        // arrives, and a window mid-navigation would otherwise pair the new
        // folder's name with the previous folder's entries.
        if self.dir_load_in_flight && matches!(request, CenterCall::CurrentFolder { .. }) {
            if self.waiting_center_calls.len() >= WAITING_CALLS_MAX {
                host.fail(
                    call,
                    "busy",
                    crate::i18n::tr("too many calls are waiting for the folder to load"),
                );
            } else {
                self.waiting_center_calls.push((call, request));
            }
            return Task::none();
        }
        let (task, outcome) = self.apply_center_call(request);
        if let Some(host) = self.center.as_mut() {
            host.answer(call, outcome);
        }
        task
    }

    pub(crate) fn answer_waiting_center_calls(&mut self) -> Task<Msg> {
        if self.dir_load_in_flight || self.waiting_center_calls.is_empty() {
            return Task::none();
        }
        let waiting = std::mem::take(&mut self.waiting_center_calls);
        let mut tasks = Vec::new();
        for (call, request) in waiting {
            // A call whose deadline passed, or whose connection went away,
            // while it waited is already answered by the host.
            if !self.center.as_ref().is_some_and(|host| host.is_live(&call)) {
                continue;
            }
            let (task, outcome) = self.apply_center_call(request);
            if let Some(host) = self.center.as_mut() {
                host.answer(call, outcome);
            }
            tasks.push(task);
        }
        Task::batch(tasks)
    }

    fn center_location(&self) -> Value {
        json!({ "folder": display_folder(&self.model.cwd, &self.model.home) })
    }

    fn apply_center_call(&mut self, request: CenterCall) -> (Task<Msg>, CenterOutcome) {
        match request {
            CenterCall::CurrentFolder { offset, limit } => {
                if limit == 0 || limit > LIST_LIMIT || offset > u32::MAX as usize {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "invalid_arguments",
                            crate::i18n::tr("offset or page limit is out of bounds"),
                        )),
                    );
                }
                if self.model.screen != Screen::Directory {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "unavailable",
                            crate::i18n::tr("no folder is being shown"),
                        )),
                    );
                }
                let visible = self.model.projected_entries();
                let mut page = json!({
                    "folder": display_folder(&self.model.cwd, &self.model.home),
                    "entries": [],
                    "truncated": false,
                    "next_offset": u64::MAX,
                    "total": visible.len(),
                });
                let budget = enzui_agent::center::protocol::DEFAULT_RESULT_BYTES as usize;
                let mut bytes = serde_json::to_vec(&page).expect("JSON page").len();
                let mut entries = Vec::new();
                for entry in visible.iter().skip(offset).take(limit) {
                    let row = json!({
                        "name": entry.name,
                        "kind": match entry.kind {
                            EntryKind::Dir => "folder",
                            EntryKind::File => "file",
                            EntryKind::Symlink => "link",
                        },
                        "size": entry.size,
                    });
                    let added = serde_json::to_vec(&row).expect("JSON row").len()
                        + usize::from(!entries.is_empty());
                    if bytes + added > budget {
                        break;
                    }
                    bytes += added;
                    entries.push(row);
                }
                if bytes > budget || (offset < visible.len() && entries.is_empty()) {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "result_too_large",
                            crate::i18n::tr("one folder entry exceeds the result budget"),
                        )),
                    );
                }
                let next = offset.saturating_add(entries.len());
                let truncated = next < visible.len();
                page["entries"] = json!(entries);
                page["truncated"] = json!(truncated);
                page["next_offset"] = if truncated { json!(next) } else { Value::Null };
                (Task::none(), Ok(page))
            }
            CenterCall::OpenFolder { name } => {
                if !is_plain_name(&name) {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "invalid_arguments",
                            crate::i18n::tr("a folder name, not a path"),
                        )),
                    );
                }
                let Some(target) = self
                    .model
                    .entries
                    .iter()
                    .find(|entry| entry.name == name && entry.kind == EntryKind::Dir)
                    .map(|entry| entry.path.clone())
                else {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "not_found",
                            crate::i18n::tr("the current folder has no sub-folder with that name"),
                        )),
                    );
                };
                let task = self.update(Msg::NavigateTo(target));
                (task, Ok(self.center_location()))
            }
            CenterCall::GoUp => {
                if self.model.cwd.parent().is_none() {
                    return (
                        Task::none(),
                        Err(CenterFailure::new(
                            "unavailable",
                            crate::i18n::tr("the folder has no parent"),
                        )),
                    );
                }
                let task = self.update(Msg::GoUp);
                (task, Ok(self.center_location()))
            }
            CenterCall::GoHome => {
                let home = self.model.home.clone();
                let task = self.update(Msg::NavigateTo(home));
                (task, Ok(self.center_location()))
            }
        }
    }
}
