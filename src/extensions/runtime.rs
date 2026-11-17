use super::*;
use enzui_core::cancellation::CancelToken;
pub(crate) use files_extension_sdk::{Input, Request, Response};
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

pub(crate) fn request(
    extension: &Installed,
    action: &Action,
    cwd: &std::path::Path,
    selection: Vec<crate::model::FileEntry>,
) -> Request {
    Request {
        api: manifest::API,
        extension: extension.id.clone(),
        action: action.id.clone(),
        cwd: cwd.as_os_str().as_bytes().to_vec(),
        selection: selection
            .into_iter()
            .map(|e| Input {
                path: e.path.as_os_str().as_bytes().to_vec(),
                name: e.name,
                mime: e.mime,
                is_dir: e.kind == crate::model::EntryKind::Dir,
            })
            .collect(),
        settings: extension.settings(),
    }
}
pub(crate) fn validate(response: &Response, permissions: &[Permission]) -> Result<(), String> {
    if response.api != manifest::API
        || response.message.len() > 1024
        || response.message.contains('\0')
    {
        return Err(crate::i18n::tr("The extension returned an invalid response").into());
    }
    if response.clipboard.is_some()
        && (!permissions.contains(&Permission::Clipboard)
            || response
                .clipboard
                .as_ref()
                .is_some_and(|text| text.len() > 32768 || text.contains('\0')))
    {
        return Err(crate::i18n::tr(
            "The extension returned clipboard content without permission or beyond its limit",
        )
        .into());
    }
    if let Some(bytes) = &response.navigate {
        let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
        if !permissions.contains(&Permission::Navigate)
            || bytes.len() > 4096
            || bytes.contains(&0)
            || !path.is_absolute()
            || crate::services::is_virtual(&path)
        {
            return Err(
                crate::i18n::tr("The extension returned an invalid navigation request").into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn run(
    extension: Installed,
    root: PathBuf,
    request: Request,
    cancel: &CancelToken,
) -> Result<Response, String> {
    if !extension.active() {
        return Err(crate::i18n::tr("The extension is disabled or incompatible").into());
    }
    let manifest = extension
        .manifest
        .as_ref()
        .ok_or_else(|| crate::i18n::tr("The extension manifest is missing"))?;
    use enzui_services::extension_package as shared;
    let shared = shared::Installed {
        id: extension.id.clone(),
        manifest: Some(manifest.shared()),
        state: extension.state.clone(),
        bundled: extension.bundled,
        problem: None,
    };
    let cwd = PathBuf::from(std::ffi::OsString::from_vec(request.cwd.clone()));
    let response: Response = shared::invoke(
        &root,
        &shared,
        &request,
        &cwd,
        &shared::Isolation::Ambient,
        &shared::WorkerLimits::default(),
        cancel,
    )
    .map_err(|error| error.to_string())?;
    validate(&response, &manifest.permissions)?;
    Ok(response)
}

fn checksum(request: Request) -> Result<Response, String> {
    if request.api != 1
        || request.action != "sha256"
        || request.selection.is_empty()
        || request.selection.len() > 32
    {
        return Err(crate::i18n::tr("Invalid checksum request").into());
    }
    let mut results = String::new();
    let mut total = 0u64;
    for input in &request.selection {
        let path = PathBuf::from(std::ffi::OsString::from_vec(input.path.clone()));
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| e.to_string())?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err(crate::i18n::tr("Only regular files can be checksummed").into());
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0; 65536];
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > 8 * 1024 * 1024 * 1024 {
                return Err(crate::i18n::tr("Checksum selection exceeds 8 GB").into());
            }
            digest.update(&buffer[..n]);
        }
        use std::fmt::Write;
        writeln!(
            results,
            "{:x}  {}",
            digest.finalize(),
            input.name.replace(['\n', '\r'], " ")
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(Response {
        api: 1,
        message: crate::i18n::format(
            "files-copied-sha-256-checksums-for-files-de484a48",
            &[("arg0", (request.selection.len()).to_string())],
        ),
        clipboard: Some(results),
        ..Response::default()
    })
}

pub(crate) fn helper_mode() -> Option<std::process::ExitCode> {
    if let Some(code) = enzui_services::extension_package::serve_builtin(
        &enzui_services::extension_package::WorkerLimits::default(),
        |id, request| {
            if id == "checksum" {
                checksum(request)
            } else {
                Err(crate::i18n::tr("Unknown builtin extension").into())
            }
        },
    ) {
        return Some(code);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let first = args.first()?.to_str()?;
    if first == "--extension-check" && args.len() == 2 {
        return Some(match Prepared::open(&PathBuf::from(&args[1])) {
            Ok(package) => {
                let _ = serde_json::to_writer(std::io::stdout().lock(), &package.manifest);
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        });
    }
    if first == "--extension-pack" && args.len() == 3 {
        return Some(
            match pack(&PathBuf::from(&args[1]), &PathBuf::from(&args[2])) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::ExitCode::FAILURE
                }
            },
        );
    }
    None
}
fn pack(source: &std::path::Path, output: &std::path::Path) -> Result<(), String> {
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut pending = vec![source.clone()];
    let mut bytes = 0u64;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
                return Err(
                    crate::i18n::tr("Packages cannot contain links or special files").into(),
                );
            }
            let name = entry
                .path()
                .strip_prefix(&source)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or_else(|| crate::i18n::tr("Package names must be UTF-8"))?
                .to_owned();
            if name.split('/').any(|part| part.starts_with('.')) || name == "state.json" {
                return Err(
                    crate::i18n::tr("Packages cannot contain hidden or reserved files").into(),
                );
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                bytes += entry.metadata().map_err(|e| e.to_string())?.len();
                files.push((name, entry.path()));
            }
            if files.len() + pending.len() > 128 || bytes > store::PACKAGE_LIMIT {
                return Err(crate::i18n::tr("Package exceeds the file or size limit").into());
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let file = std::fs::File::create_new(output).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut zip = zip::ZipWriter::new(file);
        for (name, path) in files {
            zip.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(|e| e.to_string())?;
            std::io::copy(
                &mut std::fs::File::open(path).map_err(|e| e.to_string())?,
                &mut zip,
            )
            .map_err(|e| e.to_string())?;
        }
        zip.finish()
            .map_err(|e| e.to_string())?
            .sync_all()
            .map_err(|e| e.to_string())?;
        Prepared::open(output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}
