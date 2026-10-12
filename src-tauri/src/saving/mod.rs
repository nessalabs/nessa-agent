//! Saving a file the page hands over: the host's save dialog, and the host's
//! own write.
//!
//! The page holds a file's verified bytes — one an agent published into a
//! conversation — and the person asks to keep it. A webview cannot do that
//! itself here: WKWebView cancels a download nobody handles and WebKitGTK has
//! no handler either, and the link policy refuses the `blob:` URL a download
//! would follow (`crate::links`), which is also why the page carries no
//! `<a download>` at all. So the page hands the host two things and nothing
//! else — the bytes, and a name it suggests — and the host asks the person
//! where, in the operating system's own dialog, and writes them there.
//!
//! ```text
//! page ──save_file(bytes, x-nessa-save-name)──▶ saved()
//!          size ≤ LARGEST_SAVED_BYTES ─▶ suggested_name ─▶ save dialog ─▶ write
//!      ◀── "saved" | "cancelled" | FileNotSaved ──┘
//! ```
//!
//! The page names no path, and learns none: the dialog's answer stays in the
//! host. The name is the agent's, so it is cleaned before the dialog suggests
//! it ([`rule::suggested_name`]), and the person has the last word on it. The
//! webview is granted nothing new by this: `save_file` is Nessa's own command,
//! and the host calls the dialog plugin.

mod rule;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tauri::async_runtime::{channel, spawn_blocking};
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::composition::HostDependencies;
pub use rule::SaveDestination;
use rule::{name_from_header, saved, Chosen, FileNotSaved, Saved, LARGEST_SAVED_BYTES};

/// The request header that carries the suggested name, percent-encoded.
/// Matches `SAVE_NAME_HEADER` in `src/host/window.ts`.
pub const NAME_HEADER: &str = "x-nessa-save-name";

/// The real dialog: the operating system's save panel, through the plugin.
struct NativeDestination(AppHandle);

impl SaveDestination for NativeDestination {
    fn choose(&self, name: String) -> Pin<Box<dyn Future<Output = Chosen> + Send>> {
        let app = self.0.clone();
        Box::pin(async move {
            // The callback form, as the file picker uses it: the dialog has to
            // be shown from the main thread, which a blocking wait would hold.
            let (answered, mut answer) = channel(1);
            app.dialog()
                .file()
                .set_file_name(name)
                .save_file(move |chosen| {
                    let _ = answered.try_send(chosen);
                });
            let Some(chosen) = answer.recv().await else {
                return Chosen::Unavailable("the save dialog did not open".to_string());
            };
            let Some(chosen) = chosen else {
                return Chosen::Cancelled;
            };
            match chosen.into_path() {
                Ok(path) => Chosen::Path(path),
                Err(error) => Chosen::Unavailable(format!(
                    "the save dialog answered with something that is not a path on this \
                     machine: {error}"
                )),
            }
        })
    }
}

/// The dialog this build asks. Called from composition.
pub fn save_destination(app: &AppHandle) -> Arc<dyn SaveDestination> {
    Arc::new(NativeDestination(app.clone()))
}

/// Writes `bytes` to `path` off the async runtime: up to
/// [`LARGEST_SAVED_BYTES`] to a disk that may be slow.
async fn write_off_thread(path: PathBuf, bytes: Vec<u8>) -> std::io::Result<()> {
    spawn_blocking(move || std::fs::write(path, bytes))
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error.to_string())))
}

/// Save the bytes the page sends — the request's raw body — as a file the
/// person chooses, suggesting the name in [`NAME_HEADER`].
///
/// `"saved"` once written, `"cancelled"` when the person closed the dialog,
/// which is not a failure, and [`FileNotSaved`] otherwise.
/// The bytes the page sent, however the IPC carried them, refused past `most`
/// before they are copied.
///
/// A raw body is what `invoke` sends with a `Uint8Array`. The webview's
/// `postMessage` fallback, used when the custom IPC protocol is unavailable,
/// cannot carry one and sends the same bytes as a JSON array of numbers, so
/// that is taken too — each element a byte, or it is not this page's file.
fn body_bytes(body: &InvokeBody, most: usize) -> Result<Vec<u8>, FileNotSaved> {
    match body {
        InvokeBody::Raw(bytes) => {
            if bytes.len() > most {
                return Err(FileNotSaved::too_large(bytes.len(), most));
            }
            Ok(bytes.clone())
        }
        InvokeBody::Json(serde_json::Value::Array(items)) => {
            if items.len() > most {
                return Err(FileNotSaved::too_large(items.len(), most));
            }
            items
                .iter()
                .map(|item| {
                    item.as_u64()
                        .and_then(|byte| u8::try_from(byte).ok())
                        .ok_or_else(|| {
                            FileNotSaved::malformed("the file's bytes were not all bytes")
                        })
                })
                .collect()
        }
        InvokeBody::Json(_) => Err(FileNotSaved::malformed("the file was not sent as bytes")),
    }
}

/// Save the bytes the page sends — the request's body — as a file the person
/// chooses, suggesting the name in [`NAME_HEADER`].
///
/// `"saved"` once written, `"cancelled"` when the person closed the dialog,
/// which is not a failure, and [`FileNotSaved`] otherwise; a refusal's detail
/// goes to the host's log, since the page shows only that nothing was saved.
#[tauri::command]
pub async fn save_file(
    request: Request<'_>,
    deps: State<'_, HostDependencies>,
) -> Result<Saved, FileNotSaved> {
    let outcome = async {
        let name = request
            .headers()
            .get(NAME_HEADER)
            .and_then(|value| name_from_header(value.as_bytes()))
            .ok_or_else(|| FileNotSaved::malformed("no percent-encoded name came with it"))?;
        let bytes = body_bytes(request.body(), LARGEST_SAVED_BYTES)?;
        let destination = deps.save_destination.clone();
        saved(
            &*destination,
            &name,
            bytes,
            LARGEST_SAVED_BYTES,
            write_off_thread,
        )
        .await
    }
    .await;
    if let Err(refusal) = &outcome {
        eprintln!(
            "[nessa] a file was not saved ({:?}): {}",
            refusal.reason, refusal.detail
        );
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use rule::NotSavedReason;

    #[test]
    fn takes_the_bytes_raw_or_as_the_fallback_s_json_array() {
        assert_eq!(
            body_bytes(&InvokeBody::Raw(vec![1, 2, 255]), 10),
            Ok(vec![1, 2, 255])
        );
        assert_eq!(
            body_bytes(&InvokeBody::Json(serde_json::json!([1, 2, 255])), 10),
            Ok(vec![1, 2, 255])
        );
    }

    #[test]
    fn refuses_too_much_however_it_came() {
        let raw = body_bytes(&InvokeBody::Raw(vec![0; 11]), 10).unwrap_err();
        assert_eq!(raw.reason, NotSavedReason::TooLarge);
        let json = body_bytes(&InvokeBody::Json(serde_json::json!(vec![0; 11])), 10).unwrap_err();
        assert_eq!(json.reason, NotSavedReason::TooLarge);
    }

    #[test]
    fn refuses_json_that_is_not_bytes() {
        for body in [
            serde_json::json!([1, 256]),
            serde_json::json!([1, -1]),
            serde_json::json!([1, "2"]),
            serde_json::json!({ "bytes": [1] }),
            serde_json::json!("AQI="),
        ] {
            let refusal = body_bytes(&InvokeBody::Json(body.clone()), 10).unwrap_err();
            assert_eq!(refusal.reason, NotSavedReason::Malformed, "for {body}");
        }
    }
}
