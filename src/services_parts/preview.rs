// Preview content.

use super::*;

pub(crate) const PREVIEW_TEXT_BYTES: usize = 64 * 1024;

// Loads what a preview window paints for `path`, classified by `mime`.
//
// Images decode under the framework's limits; text reads the first
// [`PREVIEW_TEXT_BYTES`] lossily; PDFs open with page navigation; local media
// uses the framework playback worker. Unreadable supported sources are errors.
//
// Blocking; callers run it inside `Task::perform`.
#[must_use]
pub(crate) fn read_preview_content(path: &Path, mime: &str) -> crate::preview::PreviewContent {
    use crate::preview::PreviewContent;
    if mime.starts_with("image/") {
        use enzui_core::decode_limits::DecodeLimits;

        let max = usize::try_from(crate::thumbnail::MAX_SOURCE_BYTES).unwrap_or(usize::MAX - 1);
        let Some(bytes) = read_head_bytes(path, max.saturating_add(1)) else {
            return PreviewContent::Error(
                crate::i18n::tr("Could not read image preview").to_owned(),
            );
        };
        if bytes.len() > max {
            return PreviewContent::Error(
                crate::i18n::tr("Image exceeds preview byte limit").to_owned(),
            );
        }
        return match enzui_resources::decode_static(&bytes, &DecodeLimits::DEFAULT, || false) {
            Ok(decoded) => PreviewContent::Image(decoded.handle),
            Err(error) => PreviewContent::Error(crate::i18n::format(
                "files-could-not-decode-image-error-6f1c9cd1",
                &[("error", (error).to_string())],
            )),
        };
    }
    if is_textual_mime(mime) {
        return match read_head_bytes(path, PREVIEW_TEXT_BYTES) {
            Some(bytes) => PreviewContent::Text(String::from_utf8_lossy(&bytes).into_owned()),
            None => {
                PreviewContent::Error(crate::i18n::tr("Could not read text preview").to_owned())
            }
        };
    }
    if mime == "application/pdf" {
        return match crate::preview_pdf::render(path, 0) {
            Ok(page) => PreviewContent::Pdf {
                page,
                pending: None,
                error: None,
            },
            Err(error) => PreviewContent::Error(error),
        };
    }
    if mime.starts_with("audio/") || mime.starts_with("video/") {
        return crate::preview::MediaPreview::open(path, mime.starts_with("audio/"));
    }
    PreviewContent::Unsupported
}

#[must_use]
pub(crate) fn is_previewable_mime(mime: &str) -> bool {
    mime.starts_with("image/")
        || is_textual_mime(mime)
        || mime == "application/pdf"
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
}

// Whether a mime type previews as text: `text/*` plus common UTF-8
// `application/*` types such as JSON, XML and scripts.
fn is_textual_mime(mime: &str) -> bool {
    if mime.starts_with("text/") {
        return true;
    }
    matches!(
        mime,
        "application/json"
            | "application/xml"
            | "application/javascript"
            | "application/x-shellscript"
            | "application/x-desktop"
            | "application/toml"
            | "application/x-yaml"
            | "application/yaml"
    )
}

// Reads up to `cap` bytes from the head of `path`; `None` on any I/O error.
fn read_head_bytes(path: &Path, cap: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = super::open_regular_file(path).ok()?;
    let mut buf = Vec::new();
    file.take(cap as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

// Whether `path` is a virtual sentinel; `Screen` drives routing.

pub(crate) fn is_virtual(path: &Path) -> bool {
    path.to_string_lossy().starts_with("files-app://")
}
