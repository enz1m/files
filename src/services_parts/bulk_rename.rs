use super::{BulkRenameOutcome, publish::marker_sibling, stage};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) fn apply(operations: &[(PathBuf, PathBuf)]) -> BulkRenameOutcome {
    apply_with(operations, stage::rename_paths_no_replace)
}

fn apply_with(
    operations: &[(PathBuf, PathBuf)],
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> BulkRenameOutcome {
    let mut outcome = BulkRenameOutcome::default();
    let mut staged = Vec::new();
    for (index, (from, to)) in operations.iter().enumerate() {
        match reserve_stage(from, &mut rename) {
            Ok(temp) => staged.push((from.clone(), temp, to.clone())),
            Err(error) => {
                // Publishing a partially staged cycle could occupy another source's recovery name.
                for (original, temp, _) in staged {
                    let detail = restore(&temp, &original, &error, &mut rename);
                    outcome.failed.push((original, detail));
                }
                for (original, _) in &operations[index..] {
                    outcome.failed.push((original.clone(), error.to_string()));
                }
                return outcome;
            }
        }
    }
    for (original, temp, destination) in staged {
        match rename(&temp, &destination) {
            Ok(()) => outcome.renamed.push((original, destination)),
            Err(error) => {
                let detail = restore(&temp, &original, &error, &mut rename);
                outcome.failed.push((original, detail));
            }
        }
    }
    outcome
}

fn reserve_stage(
    source: &Path,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    for _ in 0..stage::STAGE_RESERVE_RETRIES {
        let candidate = marker_sibling(&source.with_file_name("rename"), "files-app-rename")?;
        match rename(source, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(crate::i18n::format(
        "files-could-not-reserve-a-unique-staging-d7b94590",
        &[
            ("arg0", source.display().to_string()),
            ("arg1", stage::STAGE_RESERVE_RETRIES.to_string()),
        ],
    )))
}

fn restore(
    stage: &Path,
    original: &Path,
    error: &io::Error,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> String {
    match rename(stage, original) {
        Ok(()) => error.to_string(),
        Err(rollback) => crate::i18n::format(
            "files-bulk-rename-recovery-retained",
            &[
                ("error", error.to_string()),
                ("rollback", rollback.to_string()),
                ("stage", stage.display().to_string()),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {}
