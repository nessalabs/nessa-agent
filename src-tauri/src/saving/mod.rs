//! Saving a file the page hands over: the host's save dialog, and the host's
//! own write, one bounded part at a time.
//!
//! The page holds a file's verified bytes — one an agent published into a
//! conversation — and the person asks to keep it. A webview cannot do that
//! itself here: WKWebView cancels a download nobody handles and WebKitGTK has
//! no handler either, and the link policy refuses the `blob:` URL a download
//! would follow (`crate::links`), which is also why the page carries no
//! `<a download>` at all. So the page asks the host to save, suggesting a
//! name; the host asks the person where, in the operating system's own
//! dialog; and the page sends the bytes in parts the host writes there.
//!
//! ```text
//! page ──save_file_begin{name}──▶ suggested_name ─▶ save dialog
//!      ◀── {state: cancelled} | {state: open, token} ─┘   (temporary file created beside the choice)
//! page ──save_file_part(≤ 1 MiB, x-nessa-save-token)──▶ appended, whole ≤ 64 MiB
//! page ──save_file_finish{token}──▶ synced, renamed over the chosen path
//! page ──save_file_abort{token}───▶ temporary file removed
//!          idle > LONGEST_IDLE ───▶ temporary file removed, token gone
//! ```
//!
//! Parts, rather than one body, because a call is bounded only if what it
//! carries is: under the webview's `postMessage` fallback a raw body travels
//! as a JSON array of one number per byte, which the IPC layer has already
//! parsed before any check here runs. A part of [`LARGEST_PART_BYTES`] keeps
//! that array, and the copy each call makes, bounded; the whole is held to
//! [`LARGEST_SAVED_BYTES`] as it grows (`sessions`).
//!
//! The page names no path, and learns none: the dialog's answer stays in the
//! host, and the page holds only a token. The name is the agent's, so it is
//! cleaned before the dialog suggests it ([`rule::suggested_name`]), and the
//! person has the last word on it. The webview is granted nothing new by
//! this: the commands are Nessa's own, and the host calls the dialog plugin.

mod rule;
mod sessions;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::async_runtime::{channel, spawn, spawn_blocking};
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::composition::HostDependencies;
pub use rule::SaveDestination;
use rule::{suggested_name, Begun, Chosen, FileNotSaved, NotSavedReason, LARGEST_PART_BYTES};
pub use sessions::SaveSessions;

/// The request header that names the save a part belongs to. Matches
/// `SAVE_TOKEN_HEADER` in `src/host/window.ts`.
pub const TOKEN_HEADER: &str = "x-nessa-save-token";

/// How often a save is checked for having fallen idle.
const IDLE_CHECK: Duration = Duration::from_secs(15);

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

/// The saves in flight. Called from composition, once per process.
pub fn save_sessions() -> Arc<SaveSessions> {
    Arc::new(SaveSessions::default())
}

/// A token no page could guess: what stands for the chosen path.
fn new_token() -> Result<String, FileNotSaved> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        FileNotSaved::new(
            NotSavedReason::NotWritten,
            format!("no randomness: {error}"),
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// One beginning, from the suggested name to an open save or none: the
/// dialog, then the temporary file beside what the person chose.
async fn begun(
    destination: &dyn SaveDestination,
    sessions: &SaveSessions,
    name: &str,
    token: String,
    now: Instant,
) -> Result<Begun, FileNotSaved> {
    match destination.choose(suggested_name(name)).await {
        Chosen::Cancelled => Ok(Begun::Cancelled),
        Chosen::Unavailable(detail) => Err(FileNotSaved::new(NotSavedReason::NoDialog, detail)),
        Chosen::Path(target) => {
            sessions.open(target, &token, now)?;
            Ok(Begun::Open { token })
        }
    }
}

/// The bytes of one part, however the IPC carried them, refused past
/// [`LARGEST_PART_BYTES`].
///
/// A raw body is what `invoke` sends with a `Uint8Array`. The `postMessage`
/// fallback sends the same bytes as a JSON array of numbers, so that is taken
/// too, within the same bound — each element a byte, or it is not this page's.
fn part_bytes(body: &InvokeBody, most: usize) -> Result<Vec<u8>, FileNotSaved> {
    let too_large = |size: usize| {
        FileNotSaved::new(
            NotSavedReason::TooLarge,
            format!("a part of {size} bytes is more than the {most} one part takes"),
        )
    };
    match body {
        InvokeBody::Raw(bytes) if bytes.len() > most => Err(too_large(bytes.len())),
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        InvokeBody::Json(serde_json::Value::Array(items)) if items.len() > most => {
            Err(too_large(items.len()))
        }
        InvokeBody::Json(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .and_then(|byte| u8::try_from(byte).ok())
                    .ok_or_else(|| {
                        FileNotSaved::new(
                            NotSavedReason::Malformed,
                            "the part's bytes were not all bytes",
                        )
                    })
            })
            .collect(),
        InvokeBody::Json(_) => Err(FileNotSaved::new(
            NotSavedReason::Malformed,
            "the part was not sent as bytes",
        )),
    }
}

/// `outcome`, with a refusal's detail in the host's log: the page shows only
/// that nothing was saved.
fn logged<T>(outcome: Result<T, FileNotSaved>) -> Result<T, FileNotSaved> {
    if let Err(refusal) = &outcome {
        eprintln!(
            "[nessa] a file was not saved ({:?}): {}",
            refusal.reason, refusal.detail
        );
    }
    outcome
}

/// Runs a ledger step off the async runtime: a part is a disk write.
async fn on_disk<T: Send + 'static>(
    step: impl FnOnce() -> Result<T, FileNotSaved> + Send + 'static,
) -> Result<T, FileNotSaved> {
    spawn_blocking(step).await.unwrap_or_else(|error| {
        Err(FileNotSaved::new(
            NotSavedReason::NotWritten,
            error.to_string(),
        ))
    })
}

/// Lets go of the save under `token` once it has sat idle for
/// [`sessions::LONGEST_IDLE`], and of any other that has; stops once it is gone.
fn let_go_when_idle(sessions: Arc<SaveSessions>, token: String) {
    spawn(async move {
        loop {
            tokio::time::sleep(IDLE_CHECK).await;
            if !sessions.sweep(Instant::now(), &token) {
                break;
            }
        }
    });
}

/// Begin saving a file the person names the place of, suggesting `name`.
///
/// `{state: "open", token}` once they chose, `{state: "cancelled"}` when they
/// closed the dialog — not a failure — and [`FileNotSaved`] otherwise.
#[tauri::command]
pub async fn save_file_begin(
    name: String,
    deps: State<'_, HostDependencies>,
) -> Result<Begun, FileNotSaved> {
    let token = new_token()?;
    let sessions = deps.save_sessions.clone();
    let outcome = begun(
        &*deps.save_destination,
        &sessions,
        &name,
        token,
        Instant::now(),
    )
    .await;
    if let Ok(Begun::Open { token }) = &outcome {
        let_go_when_idle(sessions, token.clone());
    }
    logged(outcome)
}

/// Append one part — the request's body, at most [`LARGEST_PART_BYTES`] — to
/// the save named by [`TOKEN_HEADER`].
#[tauri::command]
pub async fn save_file_part(
    request: Request<'_>,
    deps: State<'_, HostDependencies>,
) -> Result<(), FileNotSaved> {
    let outcome = async {
        let token = request
            .headers()
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| FileNotSaved::new(NotSavedReason::Malformed, "no save token"))?;
        let bytes = part_bytes(request.body(), LARGEST_PART_BYTES)?;
        let sessions = deps.save_sessions.clone();
        on_disk(move || sessions.part(&token, &bytes, Instant::now())).await
    }
    .await;
    logged(outcome)
}

/// Finish the save under `token`: synced, and renamed over the chosen path.
#[tauri::command]
pub async fn save_file_finish(
    token: String,
    deps: State<'_, HostDependencies>,
) -> Result<(), FileNotSaved> {
    let sessions = deps.save_sessions.clone();
    logged(on_disk(move || sessions.finish(&token)).await)
}

/// Give up the save under `token`, removing what it had written.
#[tauri::command]
pub async fn save_file_abort(
    token: String,
    deps: State<'_, HostDependencies>,
) -> Result<(), FileNotSaved> {
    let sessions = deps.save_sessions.clone();
    on_disk(move || {
        sessions.abort(&token);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A dialog that answers what it was given, and remembers what it suggested.
    struct Answering {
        answer: Chosen,
        suggested: Mutex<Vec<String>>,
    }

    impl SaveDestination for Answering {
        fn choose(&self, name: String) -> Pin<Box<dyn Future<Output = Chosen> + Send>> {
            self.suggested.lock().unwrap().push(name);
            let answer = self.answer.clone();
            Box::pin(async move { answer })
        }
    }

    fn answering(answer: Chosen) -> Answering {
        Answering {
            answer,
            suggested: Mutex::new(Vec::new()),
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        // Every future here is ready the first time it is polled.
        use std::task::{Context, Poll, Waker};
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("a substitute dialog waited"),
        }
    }

    #[test]
    fn begins_a_save_where_the_person_chose_under_the_cleaned_name() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("chosen.png");
        let dialog = answering(Chosen::Path(target.clone()));
        let sessions = SaveSessions::default();
        let now = Instant::now();
        let outcome = block_on(begun(
            &dialog,
            &sessions,
            "../../.ssh/chart.png",
            "t".into(),
            now,
        ));
        assert_eq!(outcome, Ok(Begun::Open { token: "t".into() }));
        assert_eq!(*dialog.suggested.lock().unwrap(), ["chart.png"]);
        sessions.part("t", b"\x89PNG", now).unwrap();
        sessions.finish("t").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"\x89PNG");
    }

    #[test]
    fn opens_nothing_when_the_person_cancels_or_no_dialog_comes_up() {
        let folder = tempfile::tempdir().unwrap();
        let sessions = SaveSessions::default();
        let cancelled = block_on(begun(
            &answering(Chosen::Cancelled),
            &sessions,
            "a.txt",
            "t".into(),
            Instant::now(),
        ));
        assert_eq!(cancelled, Ok(Begun::Cancelled));
        let unavailable = block_on(begun(
            &answering(Chosen::Unavailable("no window".into())),
            &sessions,
            "a.txt",
            "u".into(),
            Instant::now(),
        ));
        assert_eq!(unavailable.unwrap_err().reason, NotSavedReason::NoDialog);
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
        assert_eq!(
            sessions.part("t", b"a", Instant::now()).unwrap_err().reason,
            NotSavedReason::NotOpen
        );
    }

    #[test]
    fn takes_a_part_raw_or_as_the_fallback_s_json_array() {
        assert_eq!(
            part_bytes(&InvokeBody::Raw(vec![1, 2, 255]), 10),
            Ok(vec![1, 2, 255])
        );
        assert_eq!(
            part_bytes(&InvokeBody::Json(serde_json::json!([1, 2, 255])), 10),
            Ok(vec![1, 2, 255])
        );
    }

    #[test]
    fn refuses_a_part_past_the_bound_however_it_came() {
        let raw = part_bytes(&InvokeBody::Raw(vec![0; 11]), 10).unwrap_err();
        assert_eq!(raw.reason, NotSavedReason::TooLarge);
        let json = part_bytes(&InvokeBody::Json(serde_json::json!(vec![0; 11])), 10).unwrap_err();
        assert_eq!(json.reason, NotSavedReason::TooLarge);
        // At the real bound: one more number than a part takes is refused.
        let over =
            serde_json::Value::Array(vec![serde_json::Value::from(0u8); LARGEST_PART_BYTES + 1]);
        let refusal = part_bytes(&InvokeBody::Json(over), LARGEST_PART_BYTES).unwrap_err();
        assert_eq!(refusal.reason, NotSavedReason::TooLarge);
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
            let refusal = part_bytes(&InvokeBody::Json(body.clone()), 10).unwrap_err();
            assert_eq!(refusal.reason, NotSavedReason::Malformed, "for {body}");
        }
    }

    #[test]
    fn mints_a_fresh_token_each_time() {
        let one = new_token().unwrap();
        assert_eq!(one.len(), 32);
        assert!(one.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(one, new_token().unwrap());
    }
}
