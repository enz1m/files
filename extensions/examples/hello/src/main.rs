//! Minimal native extension worker for Files protocol API 1.

use files_extension_sdk::{Response, serve};

fn main() -> std::process::ExitCode {
    serve(|request| {
        let greeting = request
            .settings
            .get("greeting")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Hello");
        let greeting: String = greeting.chars().take(120).collect();
        let message = match request.selection.as_slice() {
            [] => format!("{greeting} from this folder"),
            [file] => format!(
                "{greeting}: {}",
                file.name.chars().take(120).collect::<String>()
            ),
            files => format!("{greeting}: {} selected items", files.len()),
        };
        Ok(Response {
            api: 1,
            message,
            ..Response::default()
        })
    })
}
