// Freedesktop cache identity includes URI and mtime; decoded previews share a byte-bounded LRU.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use enzui_core::decode_limits::DecodeLimits;
use enzui_security::secure_dir::SecureRoot;
use enzui_security::tempstage::RootTempStage;
use iced::widget::image::Handle;

use crate::private_data::ThumbnailNestedRoots;

// Maximum encoded image bytes accepted before the static decoder runs.
pub(crate) const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
// Encoded-size ceiling for PDFs, whose parsing can scale with the document.
// Videos have none: preroll deadlines and decoded-pixel limits bound them.
pub(crate) const MAX_PDF_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
// Memory budget for decoded previews, about four hundred 128-pixel previews;
// evicted ones reload from the disk cache.
pub(crate) const PREVIEW_CACHE_BYTES: usize = 24 * 1024 * 1024;
// Freedesktop `normal` thumbnails fit within 128×128.
pub(crate) const NORMAL_SIZE: u32 = 128;
// Maximum active blocking thumbnail workers, sized for video pipelines.
//
// Twelve 1080p posters took 1031/564/443/357 ms at caps 1/2/3/4, peaking at
// 52/105/157/208 MiB worker RSS; a fourth slot saves 86 ms for 51 MiB more.
pub(crate) const MAX_CONCURRENT_DECODES: usize = 3;
// Failure-cache generation; bump when eligibility or decoder support changes so
// old failure markers cannot suppress a newly supported format.
pub(crate) const FAILURE_CACHE_NAMESPACE: &str = "files-app-3";

const MAX_CACHE_BYTES: u64 = DecodeLimits::DEFAULT.max_encoded_bytes;

#[cfg(test)]
type FileReadHook = Box<dyn FnMut(&File)>;

#[cfg(test)]
thread_local! {
    // Deterministic race seams: fire between fstat and read, and between
    // open and read in the cache/source paths.
    static AFTER_FSTAT: std::cell::RefCell<Option<FileReadHook>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_CACHE_OPEN: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_SOURCE_OPEN: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

fn bounded_read(file: &mut File, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pre_meta = file.metadata()?;
    if !pre_meta.file_type().is_file() {
        return Err(crate::i18n::tr("descriptor is not a regular file").into());
    }
    let len = pre_meta.len();
    if len > limit {
        return Err(crate::i18n::format(
            "files-file-is-len-bytes-limit-is-52617382",
            &[("len", (len).to_string()), ("limit", (limit).to_string())],
        )
        .into());
    }
    #[cfg(test)]
    AFTER_FSTAT.with(|cell| {
        if let Some(cb) = cell.borrow_mut().as_mut() {
            cb(file);
        }
    });
    let capacity = usize::try_from(len.saturating_add(1)).unwrap_or(usize::MAX);
    let mut buf = Vec::with_capacity(capacity);
    let sentinel = limit.saturating_add(1);
    let n = file.take(sentinel).read_to_end(&mut buf)? as u64;
    if n > limit {
        return Err(crate::i18n::format(
            "files-file-grew-past-limit-bytes-during-bfa502b7",
            &[("limit", (limit).to_string())],
        )
        .into());
    }
    let post_meta = file.metadata()?;
    if post_meta.len() != len
        || post_meta.mtime() != pre_meta.mtime()
        || post_meta.mtime_nsec() != pre_meta.mtime_nsec()
        || post_meta.ctime() != pre_meta.ctime()
        || post_meta.ctime_nsec() != pre_meta.ctime_nsec()
        || post_meta.ino() != pre_meta.ino()
        || post_meta.dev() != pre_meta.dev()
    {
        return Err(crate::i18n::tr("file changed during read").into());
    }
    if n != len {
        return Err(crate::i18n::format(
            "files-file-size-changed-during-read-expected-57701d19",
            &[("len", (len).to_string()), ("n", (n).to_string())],
        )
        .into());
    }
    Ok(buf)
}

fn open_source_nofollow(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| -> Box<dyn std::error::Error> {
        crate::i18n::format(
            "files-open-source-nofollow-e-81a6a015",
            &[("e", (e).to_string())],
        )
        .into()
    })?;
    let file = File::from(fd);
    if !file.metadata()?.file_type().is_file() {
        return Err(crate::i18n::tr("source is not a regular file").into());
    }
    Ok(file)
}

// Verify that `path` still names the same regular, non-symlink inode as `file`.
fn source_identity_matches(path: &Path, file: &File) -> Result<bool, std::io::Error> {
    let held = file.metadata()?;
    let Ok(current) = std::fs::symlink_metadata(path) else {
        return Ok(false);
    };
    if current.file_type().is_symlink() || !current.file_type().is_file() {
        return Ok(false);
    }
    Ok(current.dev() == held.dev() && current.ino() == held.ino())
}

enum LoadError {
    Refusal(Box<dyn std::error::Error>),
    DecodeFailed(Box<dyn std::error::Error>),
}

impl LoadError {
    fn is_decode_failure(&self) -> bool {
        matches!(self, Self::DecodeFailed(_))
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refusal(e) | Self::DecodeFailed(e) => e.fmt(f),
        }
    }
}

// Identity attached to a request/result, used to reject stale mtime replies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Stamp {
    pub(crate) path: PathBuf,
    pub(crate) mtime: u64,
}

// A completed thumbnail request. Failures are recorded so the same stamp is not
// rescheduled.
#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    pub(crate) stamp: Stamp,
    pub(crate) result: Result<Handle, String>,
}

// A queued request retains exactly the data needed by the blocking worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Request {
    pub(crate) stamp: Stamp,
    pub(crate) mime: String,
}

// Whether this entry is eligible before any task is spawned.
pub(crate) fn eligible(_path: &Path, mime: &str, size: u64) -> bool {
    // Pure scheduling predicate: filesystem checks (including opt-out marker
    // reads) belong to the worker, especially when `path` is a network mount.
    match mime {
        mime if mime.starts_with("image/") => size <= MAX_SOURCE_BYTES,
        "application/pdf" => size <= MAX_PDF_SOURCE_BYTES,
        mime if mime.starts_with("video/") => true,
        _ => false,
    }
}

// Whether `.nomedia`, `.hidden` or `.no-thumbnails` in the containing directory
// opts the file out.
pub(crate) fn opted_out(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    [".nomedia", ".hidden", ".no-thumbnails"]
        .iter()
        .any(|name| parent.join(name).is_file())
}

pub(crate) fn stamp(path: PathBuf, modified: Option<SystemTime>) -> Option<Stamp> {
    let mtime = modified?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Stamp { path, mtime })
}

// Decodes running on workers right now, including ones whose task was
// aborted: a video preroll cannot be interrupted, so promotion counts these
// rather than live task handles and never runs more than
// [`MAX_CONCURRENT_DECODES`] at once.
static RUNNING_DECODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn running_decodes() -> usize {
    RUNNING_DECODES.load(std::sync::atomic::Ordering::Acquire)
}

struct RunningDecode;

impl RunningDecode {
    fn start() -> Self {
        RUNNING_DECODES.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self
    }
}

impl Drop for RunningDecode {
    fn drop(&mut self) {
        RUNNING_DECODES.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

// Loads one thumbnail. `cancelled` turns true once the tile's task is aborted
// (scrolled away, folder left); a decode that has not begun by then is skipped.
pub(crate) fn load(
    request: Request,
    cache_roots: Option<Arc<ThumbnailNestedRoots>>,
    cancelled: &dyn Fn() -> bool,
) -> Outcome {
    if cancelled() {
        return Outcome {
            stamp: request.stamp,
            result: Err(crate::i18n::tr("Cancelled").to_owned()),
        };
    }
    let _running = RunningDecode::start();
    load_with_decoder(request, cache_roots, decode_source)
}

pub(crate) fn load_with_decoder<F>(
    request: Request,
    cache_roots: Option<Arc<ThumbnailNestedRoots>>,
    decoder: F,
) -> Outcome
where
    F: FnOnce(&Path, &str) -> Result<Handle, String>,
{
    let stamp = request.stamp;
    let result =
        load_inner(&stamp, &request.mime, cache_roots.as_deref(), decoder).map_err(|error| {
            if error.is_decode_failure()
                && let Some(roots) = cache_roots.as_deref()
            {
                let _ = write_failure_cache(&stamp, roots);
            }
            error.to_string()
        });
    Outcome { stamp, result }
}

fn load_inner<F>(
    stamp: &Stamp,
    mime: &str,
    cache_roots: Option<&ThumbnailNestedRoots>,
    decoder: F,
) -> Result<Handle, LoadError>
where
    F: FnOnce(&Path, &str) -> Result<Handle, String>,
{
    if opted_out(&stamp.path) {
        return Err(LoadError::Refusal(
            crate::i18n::tr("thumbnailing disabled for this directory").into(),
        ));
    }
    let mut source_file = open_source_nofollow(&stamp.path).map_err(LoadError::Refusal)?;
    #[cfg(test)]
    AFTER_SOURCE_OPEN.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb();
        }
    });
    let source_meta = source_file
        .metadata()
        .map_err(|e| LoadError::Refusal(e.into()))?;
    let source_mtime = source_meta.mtime() as u64;
    if source_mtime != stamp.mtime {
        return Err(LoadError::Refusal(
            crate::i18n::format(
                "files-source-mtime-changed-scheduled-now-source-58742c83",
                &[
                    ("arg0", (stamp.mtime).to_string()),
                    ("source_mtime", (source_mtime).to_string()),
                ],
            )
            .into(),
        ));
    }
    if !eligible(&stamp.path, mime, source_meta.len()) {
        return Err(LoadError::Refusal(
            crate::i18n::tr("source type or size is not eligible for a thumbnail").into(),
        ));
    }
    let uri = file_uri(&stamp.path).map_err(LoadError::Refusal)?;
    if !source_identity_matches(&stamp.path, &source_file)
        .map_err(|e| LoadError::Refusal(e.into()))?
    {
        return Err(LoadError::Refusal(
            crate::i18n::tr("source replaced after open").into(),
        ));
    }
    if let Some(roots) = cache_roots {
        let name = cache_entry_name(&uri);
        if let Some(handle) = read_valid_cache_admitted(
            &roots.normal,
            &name,
            &uri,
            stamp.mtime,
            mime.starts_with("image/"),
        ) {
            if !source_identity_matches(&stamp.path, &source_file)
                .map_err(|e| LoadError::Refusal(e.into()))?
            {
                return Err(LoadError::Refusal(
                    crate::i18n::tr("source replaced during cache read").into(),
                ));
            }
            return thumbnail_handle(handle).map_err(LoadError::Refusal);
        }
        if cache_metadata_matches_admitted(&roots.failure, &name, &uri, stamp.mtime) {
            return Err(LoadError::Refusal(
                crate::i18n::tr("a previous thumbnail attempt failed").into(),
            ));
        }
    }
    let decoded = if mime.starts_with("image/") {
        let bytes = bounded_read(&mut source_file, MAX_SOURCE_BYTES).map_err(LoadError::Refusal)?;
        decode_image_bytes(&bytes).map_err(|e| LoadError::DecodeFailed(e.into()))?
    } else {
        decoder(&stamp.path, mime).map_err(|e| LoadError::DecodeFailed(e.into()))?
    };
    if !source_identity_matches(&stamp.path, &source_file)
        .map_err(|e| LoadError::Refusal(e.into()))?
    {
        return Err(LoadError::Refusal(
            crate::i18n::tr("source replaced before publish").into(),
        ));
    }
    let source_size = match &decoded {
        Handle::Rgba { width, height, .. } => Some((*width, *height)),
        _ => None,
    };
    let handle = thumbnail_handle(decoded).map_err(LoadError::DecodeFailed)?;
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = &handle
    else {
        return Err(LoadError::DecodeFailed(
            crate::i18n::tr("thumbnail conversion did not return RGBA").into(),
        ));
    };
    if let Some(roots) = cache_roots {
        let name = cache_entry_name(&uri);
        let _ = write_cache_admitted(
            &roots.normal,
            &name,
            &uri,
            stamp.mtime,
            source_size,
            *width,
            *height,
            pixels,
        );
    }
    Ok(handle)
}

fn decode_image_bytes(bytes: &[u8]) -> Result<Handle, String> {
    let mut limits = DecodeLimits::DEFAULT;
    limits.max_encoded_bytes = MAX_SOURCE_BYTES;
    enzui_resources::decode_static(bytes, &limits, || false)
        .map(|d| d.handle)
        .map_err(|e| e.to_string())
}

fn decode_source(path: &Path, mime: &str) -> Result<Handle, String> {
    let mut limits = DecodeLimits::DEFAULT;
    let rendered = if mime == "application/pdf" {
        limits.max_encoded_bytes = MAX_PDF_SOURCE_BYTES;
        enzui_services::document_preview::render_pdf_page(path, 0, &limits)
    } else if mime.starts_with("video/") {
        // Encoded film size does not predict preroll memory; the deadline and
        // decoded-pixel limits bound it.
        limits.max_encoded_bytes = u64::MAX;
        enzui_services::document_preview::video_poster_frame(
            path,
            std::time::Duration::ZERO,
            &limits,
        )
    } else {
        return Err(crate::i18n::format(
            "files-unsupported-thumbnail-mime-mime-53bd74a0",
            &[("mime", (mime).to_owned())],
        ));
    }
    .map_err(|error| error.to_string())?;
    Ok(Handle::from_rgba(
        rendered.width(),
        rendered.height(),
        rendered.pixels().to_vec(),
    ))
}

fn thumbnail_handle(handle: Handle) -> Result<Handle, Box<dyn std::error::Error>> {
    let Handle::Rgba {
        width,
        height,
        ref pixels,
        ..
    } = handle
    else {
        return Err(crate::i18n::tr("decoder did not return RGBA").into());
    };
    let Fit::Resize {
        width: target_width,
        height: target_height,
    } = fit_inside(width, height, NORMAL_SIZE)
    else {
        return Ok(handle);
    };
    let rgba = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, pixels.as_ref())
        .ok_or_else(|| crate::i18n::tr("decoder returned an invalid RGBA buffer"))?;
    // Area sampling may alias near the source size, so use it for large reductions only.
    let thumb = if width / target_width >= 3 && height / target_height >= 3 {
        image::imageops::thumbnail(&rgba, target_width, target_height)
    } else {
        image::imageops::resize(
            &rgba,
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
    };
    Ok(Handle::from_rgba(
        target_width,
        target_height,
        thumb.into_raw(),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fit {
    Passthrough,
    Resize { width: u32, height: u32 },
}

fn fit_inside(width: u32, height: u32, bound: u32) -> Fit {
    if width <= bound && height <= bound {
        return Fit::Passthrough;
    }
    if width >= height {
        let scaled =
            (u64::from(height) * u64::from(bound) + u64::from(width) / 2) / u64::from(width);
        Fit::Resize {
            width: bound,
            height: u32::try_from(scaled).unwrap_or(bound).max(1),
        }
    } else {
        let scaled =
            (u64::from(width) * u64::from(bound) + u64::from(height) / 2) / u64::from(height);
        Fit::Resize {
            width: u32::try_from(scaled).unwrap_or(bound).max(1),
            height: bound,
        }
    }
}

fn file_uri(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStrExt;
    const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}');
    let canonical = path.canonicalize()?;
    let encoded = percent_encoding::percent_encode(canonical.as_os_str().as_bytes(), PATH_SEGMENT);
    Ok(format!("file://{encoded}"))
}

pub(crate) fn cache_entry_name(uri: &str) -> OsString {
    OsString::from(format!("{:x}.png", md5::compute(uri.as_bytes())))
}

fn cache_metadata_matches_admitted(root: &SecureRoot, name: &OsStr, uri: &str, mtime: u64) -> bool {
    let Ok(mut file) = root.open_entry_read(name) else {
        return false;
    };
    let Ok(bytes) = bounded_read(&mut file, MAX_CACHE_BYTES) else {
        return false;
    };
    cache_png_metadata_matches(&bytes, uri, mtime)
}

fn read_valid_cache_admitted(
    root: &SecureRoot,
    name: &OsStr,
    uri: &str,
    mtime: u64,
    image: bool,
) -> Option<Handle> {
    let mut file = root.open_entry_read(name).ok()?;
    #[cfg(test)]
    AFTER_CACHE_OPEN.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb();
        }
    });
    let bytes = bounded_read(&mut file, MAX_CACHE_BYTES).ok()?;
    if !cache_png_metadata_matches(&bytes, uri, mtime) {
        return None;
    }
    // An image entry must say what it was made from and still have that
    // shape. Video and PDF entries are trusted as written: other apps share
    // them, and remaking one means decoding the film or the document.
    let source = if image {
        Some(cache_png_source_size(&bytes)?)
    } else {
        None
    };
    let decoded = enzui_resources::decode_static(&bytes, &DecodeLimits::DEFAULT, || false).ok()?;
    match (&decoded.handle, source) {
        (_, None) => Some(decoded.handle),
        (Handle::Rgba { width, height, .. }, Some(source))
            if keeps_aspect((*width, *height), source) =>
        {
            Some(decoded.handle)
        }
        _ => None,
    }
}

// The source size an entry was made from (`Thumb::Image::Width` and `Height`).
// Image entries without it are regenerated once, since some were squashed square.
fn cache_png_source_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .ok()?;
    let info = reader.info();
    let number = |key: &str| {
        info.uncompressed_latin1_text
            .iter()
            .find(|chunk| chunk.keyword == key)
            .and_then(|chunk| chunk.text.parse::<u32>().ok())
            .filter(|value| *value > 0)
    };
    Some((
        number("Thumb::Image::Width")?,
        number("Thumb::Image::Height")?,
    ))
}

// Whether a `thumb`-sized picture is `source` scaled, within the one pixel
// a rounded dimension can be off by.
fn keeps_aspect(thumb: (u32, u32), source: (u32, u32)) -> bool {
    let (tw, th) = (f64::from(thumb.0), f64::from(thumb.1));
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    if tw <= 0.0 || th <= 0.0 {
        return false;
    }
    if sw >= sh {
        (th - tw * sh / sw).abs() <= 1.0
    } else {
        (tw - th * sw / sh).abs() <= 1.0
    }
}

fn cache_png_metadata_matches(bytes: &[u8], uri: &str, mtime: u64) -> bool {
    let Ok(reader) = png::Decoder::new(std::io::Cursor::new(bytes)).read_info() else {
        return false;
    };
    let info = reader.info();
    let text = |key: &str| {
        info.uncompressed_latin1_text
            .iter()
            .find(|chunk| chunk.keyword == key)
            .map(|chunk| chunk.text.as_str())
    };
    text("Thumb::URI") == Some(uri) && text("Thumb::MTime") == Some(&mtime.to_string())
}

#[allow(
    clippy::too_many_arguments,
    reason = "PNG metadata and pixel dimensions are checked together at publication"
)]
fn write_cache_admitted(
    root: &SecureRoot,
    name: &OsStr,
    uri: &str,
    mtime: u64,
    source_size: Option<(u32, u32)>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stage = RootTempStage::create(root, name)?;
    let mut bytes = Vec::with_capacity(rgba.len().saturating_add(4 * 1024));
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_text_chunk("Thumb::URI".into(), uri.into())?;
        encoder.add_text_chunk("Thumb::MTime".into(), mtime.to_string())?;
        if let Some((source_width, source_height)) = source_size {
            encoder.add_text_chunk("Thumb::Image::Width".into(), source_width.to_string())?;
            encoder.add_text_chunk("Thumb::Image::Height".into(), source_height.to_string())?;
        }
        encoder.add_text_chunk("Software".into(), "files-app".into())?;
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
        writer.finish()?;
    }
    stage.write_all(&bytes)?;
    stage.commit()?;
    Ok(())
}

fn write_failure_cache(
    stamp: &Stamp,
    roots: &ThumbnailNestedRoots,
) -> Result<(), Box<dyn std::error::Error>> {
    let uri = file_uri(&stamp.path)?;
    let name = cache_entry_name(&uri);
    write_cache_admitted(
        &roots.failure,
        &name,
        &uri,
        stamp.mtime,
        None,
        1,
        1,
        &[0, 0, 0, 0],
    )
}
