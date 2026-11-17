// Drop, helper ownership, and persistence roots.

use super::*;

impl Drop for App {
    fn drop(&mut self) {
        if let Some(cancel) = &self.extension_cancel {
            cancel.cancel(enzui_core::cancellation::CancelCause::Shutdown);
        }
        self.cancel_all_launches();
        self.cancel_mime_query();
        self.cancel_all_mime_writes();
        self.cancel_all_unmounts();
    }
}

// One owned short desktop helper: its cancellation token and abort handle.
// Cancelling tears down the process group; aborting only suppresses the
// message, so both are released together.
pub(crate) struct HelperOwner {
    pub(crate) cancel: CancelToken,
    pub(crate) handle: Option<iced::task::Handle>,
}
// Persistence roots, snapshotted once at production boot.
//
// Tests build them from literal sandbox paths (`PersistenceRoots::disarmed`
// and its variants) because mutating `XDG_*` races every test in the
// threaded test binary.
pub(crate) struct PersistenceRoots {
    pub(crate) pin_mode: PinPersistenceMode,
    // Held authority over the bookmarks file, or `None` when the path
    // could not be resolved/provisioned/opened.
    pub(crate) pin_authority: Option<enzui_core::state_authority::StateAuthority>,
    // Resolved bookmarks path; `None` when unresolvable or disarmed.
    pub(crate) pin_path: Option<std::path::PathBuf>,
    // Boot refusal when the path or parent could not be provisioned or
    // opened; becomes `model.pins_refused`.
    pub(crate) pin_boot_refusal: Option<services::PinRefusal>,
    // Bounded feature-state persistence mode.
    pub(crate) features_persistence: features::FeaturesPersistence,
    // Bounded download settings persistence mode.
    pub(crate) downloads_settings: downloads::SettingsPersistence,
    // The Trash data root (`$XDG_DATA_HOME`), or a unique `/tmp` sandbox.
    pub(crate) home_data_dir: std::path::PathBuf,
    // `$XDG_STATE_HOME/files-app/placement.state`, or `None` when disarmed.
    pub(crate) placement_path: Option<std::path::PathBuf>,

    // `$XDG_CONFIG_HOME/files-app/view-settings`, or `None` when disarmed,
    // which selects an in-memory backend.
    pub(crate) view_settings_path: Option<std::path::PathBuf>,
    // `$XDG_CONFIG_HOME/files-app/shortcuts`, or `None` when disarmed, which
    // selects an in-memory backend.
    pub(crate) shortcuts_path: Option<std::path::PathBuf>,
    // Descriptor-bound home-Trash authority captured at boot.
    pub(crate) home_trash: private_data::HomeTrashAuthority,
    // Descriptor-bound durable-thumbnail authority captured at boot.
    pub(crate) thumbnail_cache: private_data::ThumbnailCacheAuthority,
    // Loads view settings off-thread through a
    // [`enzui_core::persist::LoadController`], publishing defaults until
    // [`Msg::ViewSettingsLoaded`] applies the result. `false` loads
    // synchronously so fixtures can assert model state right after
    // construction; production always sets `true`.
    pub(crate) async_view_settings_boot: bool,
}

impl PersistenceRoots {
    // Snapshots the real XDG-derived roots; called once at boot.
    //
    // The home Trash and shared thumbnail cache are opened through
    // [`SecureRoot`](enzui_security::secure_dir::SecureRoot), bound to
    // their verified `(dev, ino)`. A missing or failing root makes the
    // matching authority `Unavailable`, and every entry point using it
    // short-circuits.
    pub(crate) fn production() -> Self {
        let private_roots = private_data::AppPrivateRoots::from_env();
        let (home_trash, thumbnail_cache) = compute_private_authorities(&private_roots);
        // Derived from the retained authority, not the environment, so an
        // unavailable authority yields an empty path nothing dispatches on.
        let home_data_dir = home_trash.data_home().unwrap_or_default();
        // A failure becomes a typed refusal the sidebar surfaces, blocking
        // mutations instead of silently defaulting.
        let (pin_path, pin_authority, pin_boot_refusal) = match services::bookmarks_path() {
            Some(p) => {
                let parent_ok = p.parent().is_some_and(|parent| {
                    enzui_core::secure_dir::ensure_private_dir(parent).is_ok()
                });
                if !parent_ok {
                    (
                        Some(p),
                        None,
                        Some(services::PinRefusal::Io(
                            services::PinIoCategory::DirProvisionFailed,
                        )),
                    )
                } else {
                    match enzui_core::state_authority::StateAuthority::open_or_create_tightening(&p)
                    {
                        Ok(auth) => (Some(p), Some(auth), None),
                        Err(e) => {
                            let refusal = services::pin_open_refusal(&p, &e);
                            (Some(p), None, Some(refusal))
                        }
                    }
                }
            }
            None => (None, None, Some(services::PinRefusal::Unavailable)),
        };
        Self {
            pin_mode: PinPersistenceMode::Production,
            pin_authority,
            pin_path,
            pin_boot_refusal,
            features_persistence: {
                match features::state_path() {
                    Some(path) => {
                        let parent_ok = path.parent().is_some_and(|parent| {
                            enzui_core::secure_dir::ensure_private_dir(parent).is_ok()
                        });
                        if !parent_ok {
                            features::FeaturesPersistence::Unavailable {
                                path: Some(path),
                                refusal: features::FeaturesRefusal::AuthorityUnavailable,
                            }
                        } else {
                            match enzui_core::state_authority::StateAuthority::open_or_create_tightening(
                                &path,
                            ) {
                                Ok(auth) => features::FeaturesPersistence::Held(auth),
                                Err(_) => features::FeaturesPersistence::Unavailable {
                                    path: Some(path),
                                    refusal: features::FeaturesRefusal::AuthorityUnavailable,
                                },
                            }
                        }
                    }
                    None => features::FeaturesPersistence::Unavailable {
                        path: None,
                        refusal: features::FeaturesRefusal::AuthorityUnavailable,
                    },
                }
            },
            downloads_settings: {
                match dirs::config_dir().map(|p| p.join("files-app").join("downloads.conf")) {
                    Some(path) => {
                        let parent_ok = path.parent().is_some_and(|parent| {
                            enzui_core::secure_dir::ensure_private_dir(parent).is_ok()
                        });
                        if !parent_ok {
                            downloads::SettingsPersistence::Unavailable {
                                path: Some(path),
                                refusal: downloads::SettingsRefusal::AuthorityUnavailable,
                            }
                        } else {
                            match enzui_core::state_authority::StateAuthority::open_or_create_tightening(
                                &path,
                            ) {
                                Ok(auth) => downloads::SettingsPersistence::Held(auth),
                                Err(_) => downloads::SettingsPersistence::Unavailable {
                                    path: Some(path),
                                    refusal: downloads::SettingsRefusal::AuthorityUnavailable,
                                },
                            }
                        }
                    }
                    None => downloads::SettingsPersistence::Unavailable {
                        path: None,
                        refusal: downloads::SettingsRefusal::AuthorityUnavailable,
                    },
                }
            },
            home_data_dir,
            placement_path: enzui_core::window_placement::default_state_path(PLACEMENT_APP)
                .expect("the static placement XDG application is valid"),
            view_settings_path: dirs::config_dir()
                .map(|p| p.join("files-app").join("view-settings")),
            shortcuts_path: dirs::config_dir().map(|p| p.join("files-app").join("shortcuts")),
            home_trash,
            thumbnail_cache,
            async_view_settings_boot: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn disarmed() -> Self {
        let nanos = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        };
        let home_data_dir = std::env::temp_dir().join(format!(
            "files-app-test-trash-{}-{}",
            std::process::id(),
            nanos()
        ));
        let thumbnail_cache_root =
            std::env::temp_dir().join(format!("files-app-test-cache-{}", std::process::id()));
        // Same private-directory capability as production, so trash and
        // thumbnail ops receive a real authority.
        let sandbox_roots = private_data::AppPrivateRoots::new(
            Some(home_data_dir.clone()),
            Some(thumbnail_cache_root),
        );
        let (home_trash, thumbnail_cache) = compute_private_authorities(&sandbox_roots);
        Self {
            pin_mode: PinPersistenceMode::TestDisarmed,
            pin_authority: None,
            pin_path: None,
            pin_boot_refusal: None,
            features_persistence: features::FeaturesPersistence::Disarmed,
            downloads_settings: downloads::SettingsPersistence::Disarmed,
            home_data_dir,
            placement_path: None,
            view_settings_path: None,
            shortcuts_path: None,
            home_trash,
            thumbnail_cache,
            async_view_settings_boot: false,
        }
    }
}
