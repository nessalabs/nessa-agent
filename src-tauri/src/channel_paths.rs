//! The paths a new channel names, chosen in the system's own pickers: the
//! folder its agents work in, and the files and folders attached to it.
//!
//! The page only learns paths here; it never reads what is at them. The
//! gateway checks every path again when the channel is made (it must exist,
//! and a folder must be one agents may work in), so a path chosen here is a
//! choice, not a grant. Linux has no picker of its own yet, so the page does
//! not offer these there (`main.tsx`).

use std::path::PathBuf;

use tauri::async_runtime::channel;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath};

/// Paths the picker answered with, as text. A path that is not text, or an
/// answer that is not a path on this machine, refuses the whole choice rather
/// than arriving one short.
fn texts(chosen: Vec<FilePath>) -> Result<Vec<String>, String> {
    chosen
        .into_iter()
        .map(|file| {
            let path: PathBuf = file
                .into_path()
                .map_err(|error| format!("the picker answered with no path: {error}"))?;
            path.into_os_string()
                .into_string()
                .map_err(|_| "the picker answered with a path that is not text".to_string())
        })
        .collect()
}

/// Waits for the one answer a picker's callback sends. `None` when the person
/// cancelled; an error when the picker never asked anybody.
async fn answered<T: Send + 'static>(
    ask: impl FnOnce(Box<dyn FnOnce(Option<T>) + Send>),
) -> Result<Option<T>, String> {
    let (answer, mut answers) = channel(1);
    ask(Box::new(move |chosen| {
        let _ = answer.try_send(chosen);
    }));
    answers
        .recv()
        .await
        .ok_or_else(|| "the picker did not open".to_string())
}

/// The folder the person chose for a channel; `None` when they cancelled.
#[tauri::command]
pub async fn choose_channel_folder(app: AppHandle) -> Result<Option<String>, String> {
    let chosen = answered(|done| {
        app.dialog()
            .file()
            .set_title("Where the channel's agents work")
            .pick_folder(done)
    })
    .await?;
    Ok(match chosen {
        Some(folder) => texts(vec![folder])?.pop(),
        None => None,
    })
}

/// Files, or folders, the person chose to attach to a channel; empty when
/// they cancelled. One picker cannot choose both on every system, so the
/// page asks for one kind at a time.
#[tauri::command]
pub async fn choose_channel_resources(
    app: AppHandle,
    folders: bool,
) -> Result<Vec<String>, String> {
    let chosen = answered(|done| {
        let dialog = app.dialog().file().set_title("Attach to the channel");
        if folders {
            dialog.pick_folders(done)
        } else {
            dialog.pick_files(done)
        }
    })
    .await?;
    texts(chosen.unwrap_or_default())
}

/// The person's home folder, so the page can write paths under it from `~`.
#[tauri::command]
pub fn home_folder(app: AppHandle) -> Option<String> {
    app.path()
        .home_dir()
        .ok()
        .and_then(|home| home.into_os_string().into_string().ok())
}
