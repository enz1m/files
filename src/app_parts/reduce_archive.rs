// Archive extraction and compression reducer arms.
//
// Every arm here only decides and dispatches; the filesystem work runs on
// [`crate::archive::ArchiveWork`]'s bounded owner and returns as
// [`Msg::ArchiveSettled`].

use super::*;
use crate::archive::{ArchiveFormat, ArchiveResult, ArchiveVerb};

impl App {
    pub(crate) fn reduce_extract_archive(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ExtractArchive { archive, dest_dir } = message else {
            unreachable!("misrouted message in reduce_extract_archive");
        };
        self.start_extract_archive(archive, dest_dir, false)
    }

    pub(crate) fn start_extract_archive(
        &mut self,
        archive: PathBuf,
        dest_dir: Option<PathBuf>,
        open_output: bool,
    ) -> Task<Msg> {
        if ArchiveFormat::detect(&archive).is_none() {
            self.post_status(
                crate::i18n::tr("That file is not a supported archive"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        let Some(dest_dir) = dest_dir.or_else(|| archive.parent().map(Path::to_path_buf)) else {
            return Task::none();
        };
        if crate::services::is_virtual(&dest_dir) {
            self.post_status(
                crate::i18n::tr("Cannot extract into a virtual folder"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        let name = archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let activity = self.start_file_activity(
            ActivityKind::Other("Extract"),
            crate::i18n::format("files-extracting-name-8eeacd64", &[("name", (name))]),
            1,
        );
        let now = self.activity_now_ms();
        activity::offer_cancel(&mut self.activity_center, activity, now);
        let task = self.archive_work.extract(activity.key, archive, dest_dir);
        Task::perform(task, move |settlement| Msg::ArchiveSettled {
            activity,
            verb: ArchiveVerb::Extract,
            open_output,
            result: settled_result(settlement),
        })
    }

    pub(crate) fn reduce_begin_extract_archive_to(&mut self, message: Msg) -> Task<Msg> {
        let Msg::BeginExtractArchiveTo(archive) = message else {
            unreachable!("misrouted message in reduce_begin_extract_archive_to");
        };
        if self.picker.is_pending() {
            return Task::none();
        }
        let target = picker::PickerTarget::ExtractArchive;
        self.pending_extract = Some(archive);
        self.picker.start(target, &self.model, self.main_window)
    }

    pub(crate) fn reduce_compress_selection(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CompressSelection(format) = message else {
            unreachable!("misrouted message in reduce_compress_selection");
        };
        let sources = self.model.clipboard_sources();
        if sources.is_empty() {
            self.post_status(
                crate::i18n::tr("Nothing selected to compress"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        if crate::services::is_virtual(&self.model.cwd) {
            self.post_status(
                crate::i18n::tr("Cannot compress in a virtual folder"),
                ToastTone::Warning,
            );
            return Task::none();
        }
        let count = sources.len();
        let activity = self.start_file_activity(
            ActivityKind::Other("Compress"),
            crate::i18n::format(
                "files-compressing-count-e5432aa7",
                &[("count", (count).to_string())],
            ),
            count,
        );
        let now = self.activity_now_ms();
        activity::offer_cancel(&mut self.activity_center, activity, now);
        let task =
            self.archive_work
                .compress(activity.key, sources, self.model.cwd.clone(), format);
        Task::perform(task, move |settlement| Msg::ArchiveSettled {
            activity,
            verb: ArchiveVerb::Compress,
            open_output: false,
            result: settled_result(settlement),
        })
    }

    pub(crate) fn reduce_archive_settled(&mut self, message: Msg) -> Task<Msg> {
        let Msg::ArchiveSettled {
            activity,
            verb,
            open_output,
            result,
        } = message
        else {
            unreachable!("misrouted message in reduce_archive_settled");
        };
        self.archive_work.settled(activity.key);
        let failed_verb = match verb {
            ArchiveVerb::Extract => crate::i18n::tr("Extraction failed"),
            ArchiveVerb::Compress => crate::i18n::tr("Compression failed"),
        };
        let published = match &result {
            ArchiveResult::Published { output, .. } if open_output => Some(output.clone()),
            _ => None,
        };
        let (outcome, detail, done) = match result {
            ArchiveResult::Published { output, detail } => {
                let name = output
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    TerminalOutcome::Completed,
                    crate::i18n::format(
                        "files-detail-into-name-45753a03",
                        &[("detail", (detail)), ("name", (name))],
                    ),
                    1,
                )
            }
            ArchiveResult::Cancelled => (
                TerminalOutcome::Cancelled,
                crate::i18n::tr("Cancelled").to_owned(),
                0,
            ),
            ArchiveResult::Failed(why) => {
                (TerminalOutcome::Failed, format!("{failed_verb}: {why}"), 0)
            }
        };
        self.finish_file_activity(activity, done, outcome, detail.clone());
        self.post_outcome(detail, outcome);
        // Reload on every outcome: a cancel that raced the final rename can
        // still have published, and the listing must show what is on disk.
        if let Some(output) = published {
            self.update(Msg::NavigateTo(output))
        } else {
            self.reload()
        }
    }
}

// Flatten a worker settlement into the reducer's result. A settlement the
// owner refused (busy, shut down) never ran, so it published nothing.
fn settled_result(
    settlement: enzui_core::blocking_task::BlockingTaskSettlement<ArchiveResult>,
) -> ArchiveResult {
    use enzui_core::blocking_task::BlockingTaskError;
    match settlement.into_result() {
        Ok(result) => result,
        Err(BlockingTaskError::Cancelled) => ArchiveResult::Cancelled,
        Err(BlockingTaskError::Busy) => ArchiveResult::Failed(
            crate::i18n::tr("too many archive operations are running").to_owned(),
        ),
        Err(error) => ArchiveResult::Failed(error.to_string()),
    }
}
