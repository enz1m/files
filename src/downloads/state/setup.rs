// Settings: load, apply, seed, publish, and save.

use super::*;

impl DownloadsState {
    // Loads and fully validates settings before installing them; on any error
    // the previous state is kept and a typed refusal returned.
    pub(crate) fn load_settings_bounded(
        &mut self,
        persistence: &SettingsPersistence,
    ) -> SettingsLoadOutcome {
        let snapshot = match persistence.read_bounded() {
            Ok(snap) => snap,
            Err(refusal) => {
                self.settings_refusal = Some(refusal);
                return SettingsLoadOutcome::Refused(refusal);
            }
        };
        match snapshot {
            CustodySnapshot::Absent => SettingsLoadOutcome::Absent,
            CustodySnapshot::Lost => {
                self.settings_refusal = Some(SettingsRefusal::ReadFailed);
                SettingsLoadOutcome::Lost
            }
            CustodySnapshot::Current {
                bytes, overflow, ..
            } => {
                if overflow {
                    self.settings_refusal = Some(SettingsRefusal::TooLarge);
                    return SettingsLoadOutcome::Refused(SettingsRefusal::TooLarge);
                }
                match parse_bounded_settings(&bytes) {
                    Ok(candidate) => {
                        self.install_candidate(candidate);
                        SettingsLoadOutcome::Applied
                    }
                    Err(refusal) => {
                        self.settings_refusal = Some(refusal);
                        SettingsLoadOutcome::Refused(refusal)
                    }
                }
            }
        }
    }

    // Installs a candidate already validated by `parse_bounded_settings`.
    fn install_candidate(&mut self, candidate: SettingsCandidate) {
        self.settings = candidate.settings;
        self.queue.set_max_active(self.settings.concurrency.get());
        self.settings_draft = DownloadSettingsDraft::from_settings(&self.settings);
        if let Some(uri) = candidate.destination_uri {
            if uri.is_empty() {
                self.last_destination_uri = None;
            } else {
                self.last_destination_uri = Some(uri);
            }
        }
    }

    pub(crate) fn is_settings_refused(&self) -> bool {
        self.settings_refusal.is_some()
    }

    pub(crate) fn apply_settings(&mut self) -> Result<(), String> {
        fn rate(raw: &str, label: &str) -> Result<Option<NonZeroU32>, String> {
            let value: u32 = raw.trim().parse().map_err(|_| {
                crate::i18n::format(
                    "files-label-must-be-a-whole-number-d0615116",
                    &[("label", (label).to_owned())],
                )
            })?;
            if value > MAX_RATE_KIB {
                return Err(crate::i18n::format(
                    "files-label-must-be-at-most-max-b6ecdbcf",
                    &[
                        ("label", (label).to_owned()),
                        ("MAX_RATE_KIB", (MAX_RATE_KIB).to_string()),
                    ],
                ));
            }
            Ok(NonZeroU32::new(value))
        }
        let concurrency: usize = self
            .settings_draft
            .concurrency
            .trim()
            .parse()
            .map_err(|_| {
                crate::i18n::tr("Concurrent downloads must be a whole number").to_owned()
            })?;
        if !(1..=MAX_CONCURRENCY).contains(&concurrency) {
            return Err(crate::i18n::format(
                "files-concurrent-downloads-must-be-between-1-f4eefaff",
                &[("MAX_CONCURRENCY", (MAX_CONCURRENCY).to_string())],
            ));
        }
        let max_download_kib = rate(
            &self.settings_draft.max_download_kib,
            crate::i18n::tr("Download limit"),
        )?;
        let max_upload_kib = rate(
            &self.settings_draft.max_upload_kib,
            crate::i18n::tr("Upload limit"),
        )?;
        // Policy only: no destination is opened, created or probed; authority
        // arrives later from SaveFile.
        let settings = DownloadSettings {
            max_download_kib,
            max_upload_kib,
            concurrency: NonZeroUsize::new(concurrency).ok_or_else(|| {
                crate::i18n::format(
                    "files-concurrent-downloads-must-be-between-1-f4eefaff",
                    &[("MAX_CONCURRENCY", MAX_CONCURRENCY.to_string())],
                )
            })?,
            torrent_policy: self.settings.torrent_policy,
        };
        self.queue.set_max_active(settings.concurrency.get());
        self.settings = settings;
        self.settings_draft.error = None;
        Ok(())
    }

    pub(crate) fn reset_settings_draft(&mut self) {
        self.settings_draft = DownloadSettingsDraft::from_settings(&self.settings);
    }

    // Only the explicit torrent capture mode can select the tracked fixture executable.
    pub(crate) fn seed_torrent_fixture(&mut self, destination: PathBuf) {
        let program = fixture_backend_path();
        self.backend = if crate::exe_resolver::is_executable_file(&program) {
            Aria2Status::Ready
        } else {
            Aria2Status::Absent
        };
        self.backend_program = program;
        self.settings_draft = DownloadSettingsDraft::from_settings(&self.settings);
        self.queue.set_max_active(2);
        let names = [
            "failed.torrent",
            "complete.torrent",
            "running.torrent",
            "second-running.torrent",
            "queued.torrent",
            "held.torrent",
        ];
        let mut backend = enzui_core::portal::backend::MockBackend::new();
        for name in names {
            backend.script(enzui_core::portal::backend::BackendReply {
                response: enzui_core::portal::PortalResponse::Success,
                uris: vec![format!("file://{}", destination.join(name).display())],
            });
        }
        self.destinations = DownloadDestinations::with_backend(Box::new(backend));
        for name in names {
            let source = format!("magnet:?xt=urn:btih:fixture-{name}&dn={name}");
            let _ = self.request_destination(&source);
        }
        let decisions = self.pump_destinations();
        debug_assert!(
            decisions
                .iter()
                .all(|decision| matches!(decision, DestinationDecision::Granted { .. })),
            "capture fixture destinations must pass through broker grants"
        );
    }

    // Capture-only row for the Activity fixture; it has no grant, so it cannot
    // transfer.
    pub(crate) fn seed_activity_fixture(&mut self) {
        let source = "https://example.invalid/linux-image.iso";
        let id = DownloadId(self.next_id);
        self.next_id += 1;
        if self.queue.admit(id) {
            self.names.insert(id.0, display_name(source));
            self.uris.insert(id.0, source.into());
        }
    }

    fn seed_visual_row(&mut self, source: &str) -> Option<DownloadId> {
        let id = DownloadId(self.next_id);
        self.next_id += 1;
        if !self.queue.admit(id) {
            return None;
        }
        self.names.insert(id.0, display_name(source));
        self.uris.insert(id.0, source.into());
        Some(id)
    }

    // Only the explicit torrent capture mode can select the tracked fixture executable.
    // `downloads_dir` is where the completed row says it landed, for its
    // Show in Folder action. `extra` appends that many further completed rows
    // with long names, for the many-items capture.
    pub(crate) fn seed_visual_states(&mut self, downloads_dir: &Path, extra: usize) {
        self.backend = Aria2Status::Absent;
        self.queue.set_max_active(1);
        for index in 0..extra {
            let source = format!(
                "https://example.invalid/Quarterly%20planning%20review%20recording%20with%20\
                 the%20full%20team%20and%20guests%20part%20{index}.mkv"
            );
            let Some(id) = self.seed_visual_row(&source) else {
                continue;
            };
            let total = 120_000_000 + 37_000_000 * index as u64;
            let _ = self.queue.promote();
            let Some(attempt) = self.queue.get(id).map(|download| download.attempt) else {
                continue;
            };
            let _ = self.queue.set_total(id, attempt, Some(total));
            let _ = self.queue.progress(id, attempt, total);
            let _ = self.queue.finish(id, attempt, Phase::Complete);
            self.completed_folders
                .insert(id.0, downloads_dir.to_path_buf());
        }
        let ids: Vec<_> = [
            "paused.iso",
            "running.iso",
            "failed.iso",
            "complete.iso",
            if extra > 0 {
                "Ubuntu%2024.04.1%20Desktop%20amd64%20installer%20image%20with%20extras.iso"
            } else {
                "second-active.iso"
            },
            "queued.iso",
        ]
        .into_iter()
        .filter_map(|name| {
            let source = format!("https://example.invalid/{name}");
            self.seed_visual_row(&source)
        })
        .collect();
        let _ = self.queue.promote();
        let paused_attempt = self
            .queue
            .get(ids[0])
            .expect("capture row was admitted")
            .attempt;
        let _ = self
            .queue
            .set_total(ids[0], paused_attempt, Some(2_000_000_000));
        let _ = self.queue.progress(ids[0], paused_attempt, 730_000_000);
        let _ = self.apply(ids[0], DownloadAction::Pause);
        self.controls.remove(&ids[0].0);
        let _ = self.queue.promote();
        let running_attempt = self
            .queue
            .get(ids[1])
            .expect("capture row was admitted")
            .attempt;
        let _ = self
            .queue
            .set_total(ids[1], running_attempt, Some(4_000_000_000));
        let _ = self.queue.progress(ids[1], running_attempt, 1_400_000_000);
        self.queue.set_max_active(2);
        let _ = self.queue.promote();
        let failed_attempt = self
            .queue
            .get(ids[2])
            .expect("capture row was admitted")
            .attempt;
        let _ = self
            .queue
            .set_total(ids[2], failed_attempt, Some(500_000_000));
        let _ = self.queue.progress(ids[2], failed_attempt, 80_000_000);
        let _ = self.queue.finish(ids[2], failed_attempt, Phase::Failed);
        self.errors.insert(
            ids[2].0,
            TransferError::Transport {
                category: TransportCategory::Other,
                detail: crate::i18n::tr("capture-only transport failure").into(),
            }
            .message(),
        );
        let _ = self.queue.promote();
        let complete_attempt = self
            .queue
            .get(ids[3])
            .expect("capture row was admitted")
            .attempt;
        let _ = self
            .queue
            .set_total(ids[3], complete_attempt, Some(750_000_000));
        let _ = self.queue.progress(ids[3], complete_attempt, 750_000_000);
        let _ = self.queue.finish(ids[3], complete_attempt, Phase::Complete);
        self.completed_folders
            .insert(ids[3].0, downloads_dir.to_path_buf());
        let _ = self.queue.promote();
        let active_attempt = self
            .queue
            .get(ids[4])
            .expect("capture row was admitted")
            .attempt;
        let _ = self
            .queue
            .set_total(ids[4], active_attempt, Some(1_000_000_000));
        let _ = self.queue.progress(ids[4], active_attempt, 260_000_000);
        self.queue.set_max_active(2);
        self.queue.set_max_active(2);
    }

    // Serializes the current settings; `None` if the result would exceed the
    // loader's admission ceiling.
    pub(crate) fn serialize_settings(&self) -> Option<Vec<u8>> {
        use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
        let destination = self
            .last_destination_uri
            .as_deref()
            .and_then(|uri| {
                if !validate_destination_uri_for_mutation(uri) {
                    return None;
                }
                Some(format!(
                    "destination_uri={}\n",
                    utf8_percent_encode(uri, NON_ALPHANUMERIC)
                ))
            })
            .unwrap_or_default();
        let body = format!(
            "{destination}max_download_kib={}\nmax_upload_kib={}\nconcurrency={}\ntorrent_policy={}\n",
            self.settings.max_download_kib.map_or(0, NonZeroU32::get),
            self.settings.max_upload_kib.map_or(0, NonZeroU32::get),
            self.settings.concurrency,
            TorrentPolicy::PERSISTED_DOWNLOAD_ONLY,
        );
        let bytes = body.into_bytes();
        if !validate_settings_for_save(&bytes) {
            return None;
        }
        Some(bytes)
    }

    // Serializes and publishes the current settings, returning the custody
    // outcome for the reducer.
    pub(crate) fn try_publish(
        &self,
        persistence: &mut SettingsPersistence,
    ) -> Result<(), SettingsPublishOutcome> {
        let bytes = self
            .serialize_settings()
            .ok_or(SettingsPublishOutcome::Unchanged)?;
        persistence.publish(&bytes)?;
        Ok(())
    }

    // Publishes canonical bounded defaults, never the current model, returning
    // the custody outcome for the reducer.
    pub(crate) fn try_publish_defaults(
        persistence: &mut SettingsPersistence,
    ) -> Result<(), SettingsPublishOutcome> {
        let defaults = DownloadsState::default();
        let bytes = defaults
            .serialize_settings()
            .ok_or(SettingsPublishOutcome::Unchanged)?;
        persistence.publish(&bytes)?;
        Ok(())
    }

    // Install canonical bounded defaults into the live model.
    pub(crate) fn adopt_defaults(&mut self) {
        let defaults = DownloadsState::default();
        self.settings = defaults.settings;
        self.settings_draft = DownloadSettingsDraft::from_settings(&self.settings);
        self.last_destination_uri = None;
        self.settings_refusal = None;
        self.settings_durability_debt = false;
    }

    // Whether durability debt or a refusal warrants a persistent banner.
    pub(crate) fn has_settings_debt_or_refusal(&self) -> bool {
        self.settings_durability_debt || self.settings_refusal.is_some()
    }
}
