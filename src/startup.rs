// Local launch locations for desktop directory handlers.

use std::{ffi::OsString, path::PathBuf};

pub(crate) fn first_local_directory(
    arguments: impl IntoIterator<Item = OsString>,
) -> Option<PathBuf> {
    arguments.into_iter().find_map(|argument| {
        let path = local_path(argument)?;
        if path.is_dir() {
            Some(path)
        } else if path.is_file() {
            path.parent().map(PathBuf::from)
        } else {
            None
        }
    })
}

pub(crate) fn local_path(argument: OsString) -> Option<PathBuf> {
    let path = match argument.to_str() {
        Some(text) if text.contains("://") || text.starts_with("file:") => {
            // URI query/fragment syntax must not become a literal filename.
            if text.contains(['?', '#']) {
                return None;
            }
            enzui_clipboard::validate_file_uri(text).ok()?
        }
        _ => PathBuf::from(argument),
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    Some(path)
}

pub(crate) fn first_archive(arguments: &[OsString]) -> Option<PathBuf> {
    arguments
        .iter()
        .filter_map(|arg| local_path(arg.clone()))
        .find(|path| path.is_file() && crate::archive::ArchiveFormat::detect(path).is_some())
}

pub(crate) fn first_extension_package(arguments: &[OsString]) -> Option<PathBuf> {
    arguments
        .iter()
        .filter_map(|argument| local_path(argument.clone()))
        .find(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("enzfiles"))
        })
}

#[cfg(test)]
mod tests {}

#[cfg(test)]
mod archive_tests {}
