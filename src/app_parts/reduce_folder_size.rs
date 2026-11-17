// Folder-size measurement reducer arms.
//
// The walk itself runs on its own thread ([`crate::folder_size::task`]);
// these arms start and cancel it and fold its reports into the model.

use super::*;
use crate::folder_size::{FolderSize, MeasureEvent, Tally};

impl App {
    pub(crate) fn reduce_measure_folder(&mut self, message: Msg) -> Task<Msg> {
        let Msg::MeasureFolder(path) = message else {
            unreachable!("misrouted message in reduce_measure_folder");
        };
        if path.parent() != Some(self.model.cwd.as_path()) {
            return Task::none();
        }
        let Some((generation, cancel)) = self.folder_measurements.start(&path) else {
            if self.folder_measurements.len() >= folder_size::MAX_RUNNING {
                self.post_status(
                    crate::i18n::tr("Already measuring several folders; wait for one to finish"),
                    ToastTone::Info,
                );
            }
            return Task::none();
        };
        self.model
            .folder_sizes
            .insert(path.clone(), FolderSize::Measuring(Tally::default()));
        folder_size::task(path, generation, cancel)
    }

    pub(crate) fn reduce_cancel_folder_measure(&mut self, message: Msg) -> Task<Msg> {
        let Msg::CancelFolderMeasure(path) = message else {
            unreachable!("misrouted message in reduce_cancel_folder_measure");
        };
        if self.folder_measurements.cancel(&path) {
            self.model.folder_sizes.remove(&path);
        }
        Task::none()
    }

    pub(crate) fn reduce_folder_measure(&mut self, message: Msg) -> Task<Msg> {
        let Msg::FolderMeasure {
            path,
            generation,
            event,
        } = message
        else {
            unreachable!("misrouted message in reduce_folder_measure");
        };
        // A report from a cancelled or replaced walk changes nothing.
        if !self.folder_measurements.is_current(&path, generation) {
            return Task::none();
        }
        match event {
            MeasureEvent::Progress(tally) => {
                self.model
                    .folder_sizes
                    .insert(path, FolderSize::Measuring(tally));
            }
            MeasureEvent::Done(Some(tally)) => {
                self.folder_measurements.finish(&path);
                self.model
                    .folder_sizes
                    .insert(path, FolderSize::Measured(tally));
            }
            MeasureEvent::Done(None) => {
                self.folder_measurements.finish(&path);
                self.model.folder_sizes.remove(&path);
            }
        }
        Task::none()
    }

    // Sizes belong to the folder being shown: leaving it cancels running
    // walks and drops their results.
    pub(crate) fn reconcile_folder_sizes(&mut self) {
        let cwd = self.model.cwd.clone();
        let here = |path: &Path| path.parent() == Some(cwd.as_path());
        self.folder_measurements.retain(here);
        self.model.folder_sizes.retain(|path, _| here(path));
    }
}
