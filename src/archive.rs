// Cancellable archive operations dispatched through a bounded blocking owner.
//
// Extraction uses the framework's safe archive service. Compression streams
// into a private partial file and publishes by no-replace rename.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use enzui_core::operation_activity::{ActivityKey, ActivityProgress};
use enzui_services::archive::ExtractProgress;

pub(crate) use enzui_services::archive::ArchiveFormat;

#[derive(Clone, Debug, Default)]
pub(crate) struct CompressionProgress(Arc<CompressionCounters>);

#[derive(Debug, Default)]
struct CompressionCounters {
    total: AtomicU64,
    read: AtomicU64,
}

impl CompressionProgress {
    fn activity(&self) -> ActivityProgress {
        let total = self.0.total.load(Ordering::Relaxed);
        ActivityProgress::Bytes {
            done: self.0.read.load(Ordering::Relaxed).min(total),
            total: (total > 0).then_some(total),
        }
    }

    fn record(&self, bytes: usize) {
        self.0.read.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

enum ArchiveProgress {
    Extract(ExtractProgress),
    Compress(CompressionProgress),
}

impl ArchiveProgress {
    fn activity(&self) -> ActivityProgress {
        match self {
            Self::Extract(progress) => progress.activity(),
            Self::Compress(progress) => progress.activity(),
        }
    }
}

// Bounds applied while an archive is read or written.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) max_entries: u64,
    pub(crate) max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 200_000,
            max_total_bytes: 64 * 1024 * 1024 * 1024,
        }
    }
}

// Why an archive operation did not publish.
#[derive(Debug)]
pub(crate) enum ArchiveError {
    Cancelled,
    Unsupported,
    LimitExceeded(&'static str),
    Corrupt(String),
    Io(io::Error),
    Extraction(enzui_services::archive::ArchiveError),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extraction(error) => std::fmt::Display::fmt(error, f),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Unsupported => write!(f, "this archive format is not supported"),
            Self::LimitExceeded(what) => write!(f, "the archive exceeds the {what} limit"),
            Self::Corrupt(detail) => write!(f, "cannot read the archive ({detail})"),
            Self::Io(error) => {
                use enzui_core::io_class::IoFailureCategory;
                f.write_str(match IoFailureCategory::classify(error) {
                    IoFailureCategory::Permission => crate::i18n::tr("permission denied"),
                    IoFailureCategory::NotFound | IoFailureCategory::NotADirectory => {
                        crate::i18n::tr("a file or folder was not found")
                    }
                    IoFailureCategory::StorageExhausted => {
                        crate::i18n::tr("no storage space remaining")
                    }
                    IoFailureCategory::Unknown => crate::i18n::tr("a filesystem error occurred"),
                })
            }
        }
    }
}

impl From<io::Error> for ArchiveError {
    fn from(error: io::Error) -> Self {
        if is_cancel(&error) {
            Self::Cancelled
        } else {
            Self::Io(error)
        }
    }
}

// What a completed extraction published.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ExtractReport {
    pub(crate) output: PathBuf,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) skipped: u64,
}

// What a completed compression published.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct CompressReport {
    pub(crate) output: PathBuf,
    pub(crate) entries: u64,
    pub(crate) skipped: u64,
}

const CHUNK: usize = 64 * 1024;

// The payload of the `io::Error` a cancelled reader returns, so the
// cancellation survives a library that wraps the reader's error.
#[derive(Debug)]
struct CancelMarker;

impl std::fmt::Display for CancelMarker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(crate::i18n::tr("archive operation cancelled"))
    }
}

impl std::error::Error for CancelMarker {}

fn cancelled_error() -> io::Error {
    io::Error::other(CancelMarker)
}

fn is_cancel(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + std::marker::Send + std::marker::Sync + 'static>::is::<CancelMarker>)
}

// The staging or partial sibling name for `name` in `dir`.
fn hidden_sibling(dir: &Path, name: &str, role: &str) -> PathBuf {
    let name = match name.char_indices().nth(96) {
        Some((cut, _)) => name.get(..cut).expect("index follows a UTF-8 boundary"),
        None => name,
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    dir.join(format!(".{name}.{role}-{}-{nanos}", std::process::id()))
}

// Rename `staged` to a free name in `dir` derived from `base`/`ext`, never
// replacing an existing entry. A name taken between suggestion and rename
// is retried with the next suggestion.
fn publish_no_replace(
    staged: &Path,
    dir: &Path,
    base: &str,
    ext: Option<&str>,
) -> io::Result<PathBuf> {
    for _ in 0..64 {
        let target = dir.join(crate::services::suggest_unique_name(dir, base, ext));
        match crate::services::rename_paths_no_replace(staged, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        crate::i18n::tr("no free output name"),
    ))
}

// Copies `reader` into `writer` in bounded chunks, charging every byte to
// `total` and checking cancellation between chunks.
fn bounded_copy(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    total: &mut u64,
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
    progress: &CompressionProgress,
) -> Result<u64, ArchiveError> {
    let mut buffer = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        if cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(copied),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted && !is_cancel(&error) => {
                continue;
            }
            Err(error) => return Err(corrupt_or_io(error)),
        };
        *total = total.saturating_add(read as u64);
        if *total > limits.max_total_bytes {
            return Err(ArchiveError::LimitExceeded(crate::i18n::tr("total size")));
        }
        writer.write_all(&buffer[..read])?;
        progress.record(read);
        copied += read as u64;
    }
}

fn corrupt_or_io(error: io::Error) -> ArchiveError {
    match error.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput | io::ErrorKind::UnexpectedEof => {
            ArchiveError::Corrupt(error.to_string())
        }
        _ => ArchiveError::from(error),
    }
}

// Extract into a unique folder named after the archive, without replacing existing files.
pub(crate) fn extract(
    archive: &Path,
    dest_dir: &Path,
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
    progress: &ExtractProgress,
) -> Result<ExtractReport, ArchiveError> {
    use enzui_services::archive as shared;
    let mut request = shared::ExtractRequest::new(archive, dest_dir);
    request.limits = shared::Limits {
        max_entries: limits.max_entries,
        max_output_bytes: limits.max_total_bytes,
        max_file_size: limits.max_total_bytes,
    };
    let report = shared::extract(&request, progress, cancelled).map_err(|error| match error {
        shared::ArchiveError::Cancelled => ArchiveError::Cancelled,
        shared::ArchiveError::Unsupported => ArchiveError::Unsupported,
        shared::ArchiveError::LimitExceeded(shared::Limit::Entries) => {
            ArchiveError::LimitExceeded(crate::i18n::tr("entry count"))
        }
        shared::ArchiveError::LimitExceeded(_) => {
            ArchiveError::LimitExceeded(crate::i18n::tr("total size"))
        }
        shared::ArchiveError::Corrupt(detail) => ArchiveError::Corrupt(detail),
        error => ArchiveError::Extraction(error),
    })?;
    Ok(ExtractReport {
        output: report.output,
        files: report.files,
        bytes: report.bytes,
        skipped: report.skipped.total(),
    })
}

struct Source {
    path: PathBuf,
    name: PathBuf,
}

// Every entry below `sources`, depth first, without following symlinks.
fn walk(
    sources: &[PathBuf],
    skip: &Path,
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(Source, fs::Metadata)>, ArchiveError> {
    let mut out = Vec::new();
    let mut stack: Vec<Source> = sources
        .iter()
        .rev()
        .filter_map(|path| {
            path.file_name().map(|name| Source {
                path: path.clone(),
                name: PathBuf::from(name),
            })
        })
        .collect();
    while let Some(source) = stack.pop() {
        if cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        if source.path == skip {
            continue;
        }
        let metadata = fs::symlink_metadata(&source.path)?;
        if out.len() as u64 >= limits.max_entries {
            return Err(ArchiveError::LimitExceeded(crate::i18n::tr("entry count")));
        }
        if metadata.is_dir() {
            let mut children: Vec<_> = fs::read_dir(&source.path)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|entry| entry.file_name())
                .collect();
            children.sort();
            for child in children.into_iter().rev() {
                stack.push(Source {
                    path: source.path.join(&child),
                    name: source.name.join(&child),
                });
            }
        }
        out.push((source, metadata));
    }
    Ok(out)
}

// A reader that fails with the cancellation error once `cancelled` is true.
struct CancelRead<'a, R> {
    inner: R,
    cancelled: &'a dyn Fn() -> bool,
    progress: &'a CompressionProgress,
}

impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(cancelled_error());
        }
        let read = self.inner.read(buf)?;
        self.progress.record(read);
        Ok(read)
    }
}

fn archive_name(name: &Path) -> Result<String, ArchiveError> {
    name.to_str().map(str::to_owned).ok_or_else(|| {
        ArchiveError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            crate::i18n::tr("a name is not valid UTF-8"),
        ))
    })
}

fn write_zip(
    file: File,
    entries: &[(Source, fs::Metadata)],
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
    skipped: &mut u64,
    progress: &CompressionProgress,
) -> Result<File, ArchiveError> {
    use std::os::unix::fs::PermissionsExt;
    use zip::write::SimpleFileOptions;
    let zip_error = |error: zip::result::ZipError| match error {
        zip::result::ZipError::Io(error) => ArchiveError::from(error),
        other => ArchiveError::Corrupt(other.to_string()),
    };
    let mut zip = zip::ZipWriter::new(file);
    let mut total = 0u64;
    for (source, metadata) in entries {
        let name = archive_name(&source.name)?;
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(metadata.permissions().mode() & 0o777)
            .large_file(metadata.len() >= u64::from(u32::MAX));
        if metadata.is_dir() {
            zip.add_directory(name, options).map_err(zip_error)?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source.path)?;
            zip.add_symlink(name, archive_name(&target)?, options)
                .map_err(zip_error)?;
        } else if metadata.is_file() {
            zip.start_file(name, options).map_err(zip_error)?;
            let mut reader = File::open(&source.path)?;
            bounded_copy(
                &mut reader,
                &mut zip,
                &mut total,
                limits,
                cancelled,
                progress,
            )?;
        } else {
            *skipped += 1;
        }
    }
    zip.finish().map_err(zip_error)
}

fn write_tar_gz(
    file: File,
    entries: &[(Source, fs::Metadata)],
    cancelled: &dyn Fn() -> bool,
    skipped: &mut u64,
    progress: &CompressionProgress,
) -> Result<File, ArchiveError> {
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    tar.follow_symlinks(false);
    for (source, metadata) in entries {
        if cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let file_type = metadata.file_type();
        if file_type.is_dir() || file_type.is_symlink() {
            tar.append_path_with_name(&source.path, &source.name)?;
        } else if file_type.is_file() {
            let mut header = tar::Header::new_gnu();
            header.set_metadata(metadata);
            let reader = CancelRead {
                inner: File::open(&source.path)?,
                cancelled,
                progress,
            };
            tar.append_data(&mut header, &source.name, reader)?;
        } else {
            *skipped += 1;
        }
    }
    Ok(tar.into_inner()?.finish()?)
}

// Compress `sources` into a new archive in `dest_dir`.
//
// One source names the archive after itself (without a file extension);
// several are named `Archive`. The name is made unique in `dest_dir`.
pub(crate) fn compress(
    sources: &[PathBuf],
    dest_dir: &Path,
    format: ArchiveFormat,
    limits: &Limits,
    cancelled: &dyn Fn() -> bool,
    progress: &CompressionProgress,
) -> Result<CompressReport, ArchiveError> {
    if !matches!(format, ArchiveFormat::Zip | ArchiveFormat::TarGz) {
        return Err(ArchiveError::Unsupported);
    }
    let base = match sources {
        [one] => {
            let is_dir = fs::symlink_metadata(one).is_ok_and(|m| m.is_dir());
            let name = one
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Archive");
            match (is_dir, Path::new(name).file_stem().and_then(|s| s.to_str())) {
                (false, Some(stem)) if !stem.is_empty() => stem.to_owned(),
                _ => name.to_owned(),
            }
        }
        _ => "Archive".to_owned(),
    };
    let partial = hidden_sibling(dest_dir, &base, "partial");
    let file = File::create_new(&partial)?;

    let mut skipped = 0u64;
    let result = (|| -> Result<(PathBuf, u64), ArchiveError> {
        let entries = walk(sources, &partial, limits, cancelled)?;
        let total = entries
            .iter()
            .filter(|(_, metadata)| metadata.is_file())
            .fold(0u64, |sum, (_, metadata)| {
                sum.saturating_add(metadata.len())
            });
        progress.0.total.store(total, Ordering::Relaxed);
        let file = match format {
            ArchiveFormat::Zip => {
                write_zip(file, &entries, limits, cancelled, &mut skipped, progress)?
            }
            ArchiveFormat::TarGz => {
                write_tar_gz(file, &entries, cancelled, &mut skipped, progress)?
            }
            _ => unreachable!("refused above"),
        };
        file.sync_all()?;
        if cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let extension = if format == ArchiveFormat::Zip {
            "zip"
        } else {
            "tar.gz"
        };
        let output = publish_no_replace(&partial, dest_dir, &base, Some(extension))?;
        Ok((output, entries.len() as u64 - skipped))
    })();

    match result {
        Ok((output, entries)) => Ok(CompressReport {
            output,
            entries,
            skipped,
        }),
        Err(error) => {
            let _ = fs::remove_file(&partial);
            Err(error)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveVerb {
    Extract,
    Compress,
}

// The settled outcome of one archive operation, as the reducer sees it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArchiveResult {
    // The output exists at `output`; `detail` is the user-facing summary.
    Published { output: PathBuf, detail: String },
    Cancelled,
    Failed(String),
}

fn skipped_suffix(skipped: u64) -> String {
    if skipped == 0 {
        String::new()
    } else {
        crate::i18n::format(
            "files-skipped-21ed54a6",
            &[("arg0", (crate::i18n::count("files-unsafe-count", skipped)))],
        )
    }
}

fn settle(result: Result<(PathBuf, String), ArchiveError>) -> ArchiveResult {
    match result {
        Ok((output, detail)) => ArchiveResult::Published { output, detail },
        Err(ArchiveError::Cancelled) => ArchiveResult::Cancelled,
        Err(error) => ArchiveResult::Failed(error.to_string()),
    }
}

// The bounded owner every archive operation runs on, and the cancellation
// handle of each one still in flight, keyed by its Activity Center row.
pub(crate) struct ArchiveWork {
    owner: enzui_core::blocking_task::BlockingTaskOwner,
    active: std::collections::BTreeMap<ActivityKey, ArchiveOperation>,
    generation: u64,
}

struct ArchiveOperation {
    cancellation: enzui_core::blocking_task::BlockingTaskCancellation<ArchiveResult>,
    progress: ArchiveProgress,
}

impl std::fmt::Debug for ArchiveWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveWork")
            .field("in_flight", &self.active.len())
            .finish()
    }
}

// Archive work is long and I/O bound; two workers let a second operation
// start while one runs, and the short queue refuses a burst as `Busy`.
const ARCHIVE_WORKERS: usize = 2;
const ARCHIVE_QUEUE: usize = 4;

impl ArchiveWork {
    pub(crate) fn new() -> Self {
        use std::num::NonZeroUsize;
        let queue = NonZeroUsize::new(ARCHIVE_QUEUE).expect("nonzero");
        let owner = enzui_core::blocking_task::BlockingTaskOwner::new(
            "files-archive",
            NonZeroUsize::new(ARCHIVE_WORKERS).expect("nonzero"),
            queue,
        )
        .unwrap_or_else(|_| enzui_core::blocking_task::BlockingTaskOwner::unavailable(queue));
        Self {
            owner,
            active: std::collections::BTreeMap::new(),
            generation: 0,
        }
    }

    pub(crate) fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    pub(crate) fn measurements(
        &self,
    ) -> impl Iterator<Item = (ActivityKey, ActivityProgress)> + '_ {
        self.active
            .iter()
            .map(|(&key, operation)| (key, operation.progress.activity()))
    }

    fn submit(
        &mut self,
        key: ActivityKey,
        progress: ArchiveProgress,
        work: impl FnOnce(&dyn Fn() -> bool) -> ArchiveResult + Send + 'static,
    ) -> enzui_core::blocking_task::BlockingTask<ArchiveResult> {
        self.generation = self.generation.wrapping_add(1);
        let task = self.owner.submit(self.generation, move |context| {
            work(&|| context.is_cancelled())
        });
        self.active.insert(
            key,
            ArchiveOperation {
                cancellation: task.cancellation(),
                progress,
            },
        );
        task
    }

    // Extract `archive` into `dest_dir` on the owner.
    pub(crate) fn extract(
        &mut self,
        key: enzui_core::operation_activity::ActivityKey,
        archive: PathBuf,
        dest_dir: PathBuf,
    ) -> enzui_core::blocking_task::BlockingTask<ArchiveResult> {
        let progress = ExtractProgress::default();
        self.submit(
            key,
            ArchiveProgress::Extract(progress.clone()),
            move |cancelled| {
                settle(
                    extract(
                        &archive,
                        &dest_dir,
                        &Limits::default(),
                        cancelled,
                        &progress,
                    )
                    .map(|report| {
                        let detail = crate::i18n::format(
                            "files-extracted-1d0a8b32",
                            &[
                                (
                                    "arg0",
                                    (crate::i18n::count("files-file-count", report.files)),
                                ),
                                ("arg1", (skipped_suffix(report.skipped))),
                            ],
                        );
                        (report.output, detail)
                    }),
                )
            },
        )
    }

    // Compress `sources` into `dest_dir` on the owner.
    pub(crate) fn compress(
        &mut self,
        key: enzui_core::operation_activity::ActivityKey,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        format: ArchiveFormat,
    ) -> enzui_core::blocking_task::BlockingTask<ArchiveResult> {
        let progress = CompressionProgress::default();
        self.submit(
            key,
            ArchiveProgress::Compress(progress.clone()),
            move |cancelled| {
                settle(
                    compress(
                        &sources,
                        &dest_dir,
                        format,
                        &Limits::default(),
                        cancelled,
                        &progress,
                    )
                    .map(|report| {
                        let detail = crate::i18n::format(
                            "files-compressed-6fa5970e",
                            &[
                                (
                                    "arg0",
                                    (crate::i18n::count("files-item-count", report.entries)),
                                ),
                                ("arg1", (skipped_suffix(report.skipped))),
                            ],
                        );
                        (report.output, detail)
                    }),
                )
            },
        )
    }

    // Request cancellation of the operation behind `key`; `false` when no
    // archive operation owns that row.
    pub(crate) fn cancel(&mut self, key: enzui_core::operation_activity::ActivityKey) -> bool {
        match self.active.remove(&key) {
            Some(operation) => {
                operation.cancellation.cancel();
                true
            }
            None => false,
        }
    }

    pub(crate) fn settled(&mut self, key: enzui_core::operation_activity::ActivityKey) {
        self.active.remove(&key);
    }
}

#[cfg(test)]
mod tests;
