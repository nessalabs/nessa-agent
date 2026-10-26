//! The paths a new channel names, chosen in the system's own pickers: the
//! folder its agents work in, and the files and folders attached to it.
//!
//! The page only learns paths here; it never reads what is at them. The
//! gateway checks every path again when the channel is made (it must exist,
//! and a folder must be one agents may work in), so a path chosen here is a
//! choice, not a grant. Linux has no picker of its own yet, so the page does
//! not offer these there (`main.tsx`).
//!
//! Files and folders dragged onto the desktop window arrive the same way: the
//! window's drag and drop belongs to the host (a DOM `File` names no path), so
//! the host tells the page while a drag is over it and which paths it let go
//! of, each marked a file or a folder. They are a choice too, checked again
//! by the gateway.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::async_runtime::channel;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath};

use crate::desktop_window::DESKTOP_WINDOW;
use crate::host::{PATHS_DRAGGED, PATHS_DRAGGING};

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

/// One path dropped on the desktop window, as the page is told it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DroppedPath {
    pub path: String,
    pub kind: DroppedKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DroppedKind {
    File,
    Folder,
}

/// How long one dropped path may take to say whether it is a folder. A path
/// on a stalled network share must not hold the rest; past this it is taken
/// as a file, which only decides its icon.
const KIND_WAIT: Duration = Duration::from_secs(2);

/// Whether a drag is over the window after `event`, when it changes that: a
/// drag naming no paths (text, say) is nothing the page can take.
fn dragging_after(event: &DragDropEvent) -> Option<bool> {
    match event {
        DragDropEvent::Enter { paths, .. } => Some(!paths.is_empty()),
        DragDropEvent::Leave | DragDropEvent::Drop { .. } => Some(false),
        _ => None,
    }
}

/// What the page is told of `paths`: each one there, in order, with its kind.
/// A path that is not text, or no longer there, is left out; the rest still go.
async fn described(paths: Vec<PathBuf>) -> Vec<DroppedPath> {
    let mut described = Vec::with_capacity(paths.len());
    for path in paths {
        let Some(text) = path.to_str().map(str::to_owned) else {
            continue;
        };
        let Some(kind) = kind_of(path).await else {
            continue;
        };
        described.push(DroppedPath { path: text, kind });
    }
    described
}

async fn kind_of(path: PathBuf) -> Option<DroppedKind> {
    let look = tauri::async_runtime::spawn_blocking(move || is_folder(&path));
    match tokio::time::timeout(KIND_WAIT, look).await {
        Ok(Ok(found)) => found.map(|folder| {
            if folder {
                DroppedKind::Folder
            } else {
                DroppedKind::File
            }
        }),
        Ok(Err(_)) | Err(_) => Some(DroppedKind::File),
    }
}

/// Whether `path` is a folder (following links); `None` when nothing is there.
fn is_folder(path: &Path) -> Option<bool> {
    std::fs::metadata(path).ok().map(|found| found.is_dir())
}

/// Handles one drag-drop event on the desktop window: tells the page while a
/// drag carrying paths is over it, and what it dropped. Looking at the paths
/// happens on a spawned task, off the thread that draws.
pub fn dropped_on_desktop(app: &AppHandle, event: &DragDropEvent) {
    if let Some(over) = dragging_after(event) {
        let _ = app.emit_to(
            DESKTOP_WINDOW,
            PATHS_DRAGGING,
            serde_json::json!({ "dragging": over }),
        );
    }
    let DragDropEvent::Drop { paths, .. } = event else {
        return;
    };
    if paths.is_empty() {
        return;
    }
    let app = app.clone();
    let paths = paths.clone();
    tauri::async_runtime::spawn(async move {
        let paths = described(paths).await;
        if paths.is_empty() {
            return;
        }
        let _ = app.emit_to(
            DESKTOP_WINDOW,
            PATHS_DRAGGED,
            serde_json::json!({ "paths": paths }),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::PhysicalPosition;

    fn entering(paths: &[&Path]) -> DragDropEvent {
        DragDropEvent::Enter {
            paths: paths.iter().map(|path| path.to_path_buf()).collect(),
            position: PhysicalPosition::new(0.0, 0.0),
        }
    }

    #[test]
    fn only_a_drag_of_paths_is_over_the_window() {
        assert_eq!(dragging_after(&entering(&[Path::new("/a")])), Some(true));
        assert_eq!(dragging_after(&entering(&[])), Some(false));
        assert_eq!(dragging_after(&DragDropEvent::Leave), Some(false));
        let drop = DragDropEvent::Drop {
            paths: vec![PathBuf::from("/a")],
            position: PhysicalPosition::new(0.0, 0.0),
        };
        assert_eq!(dragging_after(&drop), Some(false));
        let over = DragDropEvent::Over {
            position: PhysicalPosition::new(0.0, 0.0),
        };
        assert_eq!(dragging_after(&over), None);
    }

    #[test]
    fn dropped_paths_say_file_or_folder_and_leave_out_what_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("brief.md");
        std::fs::write(&file, "# Brief").unwrap();
        let folder = root.path().join("docs");
        std::fs::create_dir(&folder).unwrap();
        let gone = root.path().join("gone");
        let said =
            tauri::async_runtime::block_on(described(vec![file.clone(), gone, folder.clone()]));
        assert_eq!(
            said,
            vec![
                DroppedPath {
                    path: file.to_str().unwrap().to_owned(),
                    kind: DroppedKind::File,
                },
                DroppedPath {
                    path: folder.to_str().unwrap().to_owned(),
                    kind: DroppedKind::Folder,
                },
            ]
        );
        let shape = serde_json::to_value(&said[1]).unwrap();
        assert_eq!(shape["kind"], "folder");
    }
}
