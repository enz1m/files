//! Files extension API v1: one JSON request on stdin and one JSON response on stdout.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Read;

/// Stable manifest and worker protocol major version.
pub const API: u32 = 1;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One selected item with native path bytes and display metadata.
pub struct Input {
    /// Raw Unix path bytes; use this rather than the display name for filesystem I/O.
    pub path: Vec<u8>,
    /// User-visible label, which may differ from the native filename.
    pub name: String,
    /// MIME type supplied by the host classifier.
    pub mime: String,
    /// Whether selection metadata identifies a directory.
    pub is_dir: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One user-invoked action with validated applied settings.
pub struct Request {
    /// Protocol major version; must equal [`API`].
    pub api: u32,
    /// Manifest identifier of the invoked extension.
    pub extension: String,
    /// Manifest action identifier chosen by the user.
    pub action: String,
    /// Raw Unix folder bytes, also used as the worker’s working directory.
    pub cwd: Vec<u8>,
    /// Selected entries in host-provided order.
    pub selection: Vec<Input>,
    /// Validated settings keyed by manifest setting identifier.
    pub settings: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Completion effects independently validated by the host.
/// Default responses use API 1 and have no effects.
pub struct Response {
    /// Protocol major version; must equal [`API`].
    pub api: u32,
    #[serde(default)]
    /// Optional completion text shown to the user.
    pub message: String,
    #[serde(default)]
    /// Requires the manifest’s clipboard host permission.
    pub clipboard: Option<String>,
    #[serde(default)]
    /// Absolute local path bytes; requires the navigate host permission.
    pub navigate: Option<Vec<u8>>,
    #[serde(default)]
    /// Request a directory refresh after successful completion.
    pub refresh: bool,
}

impl Default for Response {
    fn default() -> Self {
        Self {
            api: API,
            message: String::new(),
            clipboard: None,
            navigate: None,
            refresh: false,
        }
    }
}

/// Serve a single bounded request; reserve stdout for protocol data.
///
/// Reads at most 2 MiB and emits at most 64 KiB of JSON. Invalid input, a handler
/// error, an unsupported API version or an output failure returns a failure
/// exit code with diagnostics on stderr.
///
/// # Panics
///
/// Propagates a panic from `handler`.
///
/// ```no_run
/// use files_extension_sdk::{Response, serve};
/// let status = serve(|_request| Ok(Response::default()));
/// assert_eq!(status, std::process::ExitCode::SUCCESS);
/// ```
pub fn serve(handler: impl FnOnce(Request) -> Result<Response, String>) -> std::process::ExitCode {
    let mut bytes = Vec::new();
    let result = std::io::stdin()
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())
        .and_then(|_| {
            if bytes.len() > 2 * 1024 * 1024 {
                return Err("request exceeds the size limit".into());
            }
            let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if request.api != 1 {
                return Err("unsupported Files extension API".into());
            }
            handler(request)
        })
        .and_then(|response| {
            let encoded = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
            if response.api != 1 || encoded.len() > 65536 {
                return Err("response exceeds the protocol limit or API version".into());
            }
            use std::io::Write;
            std::io::stdout()
                .lock()
                .write_all(&encoded)
                .map_err(|e| e.to_string())
        });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
