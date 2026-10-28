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

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::async_runtime::channel;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath};

use crate::attachments::{answered_within, ChosenFiles, Kind};
use crate::desktop_window::DESKTOP_WINDOW;
use crate::host::{PATHS_DRAGGING, PATHS_DROPPED, PATHS_DROP_BEGAN};

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

/// The product protocol, whose `x-channelRules` say what a channel holds.
const PRODUCT_PROTOCOL: &str = include_str!("../../protocol/product/v1.json");

/// Most paths of one drop the host looks at: what a channel holds at most,
/// read from the published `x-channelRules.maxResources`, never retyped.
/// Each look is a thread of its own that a stalled share can keep, so a drop
/// of thousands looks at no more than could ever be kept; the rest are
/// counted and the page says they did not fit.
fn most_dropped() -> usize {
    static MOST: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        let rules: serde_json::Value =
            serde_json::from_str(PRODUCT_PROTOCOL).expect("bundled product protocol must parse");
        rules["x-channelRules"]["maxResources"]
            .as_u64()
            .and_then(|most| usize::try_from(most).ok())
            .expect("product protocol must say x-channelRules.maxResources")
    });
    *MOST
}

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
/// A path that is not text, or that the filesystem says is not there, is left
/// out; the rest still go. The filesystem is the host's own port
/// ([`ChosenFiles`], the one the panel's drops look through), asked through
/// [`answered_within`] for every path at once, so one on a stalled share holds
/// the drop for at most `wait`, not that once per path.
async fn described(
    paths: Vec<PathBuf>,
    files: Arc<dyn ChosenFiles>,
    wait: Duration,
) -> Vec<DroppedPath> {
    let looks: Vec<_> = paths
        .into_iter()
        .filter_map(|path| {
            let text = path.to_str()?.to_owned();
            let files = files.clone();
            let look = answered_within(wait, move || files.look(&path));
            Some((text, tauri::async_runtime::spawn(look)))
        })
        .collect();
    let mut described = Vec::with_capacity(looks.len());
    for (path, look) in looks {
        let kind = match look.await {
            Ok(Ok(Ok(found))) if found.kind == Kind::Directory => DroppedKind::Folder,
            Ok(Ok(Ok(_))) => DroppedKind::File,
            // The filesystem said it is not there, or not to be read.
            Ok(Ok(Err(_))) => continue,
            // No answer in time: kept, as a file.
            Ok(Err(_)) | Err(_) => DroppedKind::File,
        };
        described.push(DroppedPath { path, kind });
    }
    described
}

/// A fresh name for one drop: what [`PATHS_DROP_BEGAN`] announces and its
/// [`PATHS_DROPPED`] answer carries.
fn next_drop() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("d{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The paths of a drop the host will look at, and how many it will not.
fn within_bound(mut paths: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let most = most_dropped();
    let unread = paths.len().saturating_sub(most);
    paths.truncate(most);
    (paths, unread)
}

/// Handles one drag-drop event on the desktop window: tells the page while a
/// drag carrying paths is over it, that a drop landed (at once, named), and
/// later what it dropped under the same name. Looking at the paths happens on
/// a spawned task, off the thread that draws, and can take up to
/// [`KIND_WAIT`]: by then the page may be asking something else, which is why
/// the answer is bound by name to the moment of the gesture.
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
    let Some(deps) = crate::composition::resolve(app) else {
        return;
    };
    let drop = next_drop();
    let _ = app.emit_to(
        DESKTOP_WINDOW,
        PATHS_DROP_BEGAN,
        serde_json::json!({ "drop": drop }),
    );
    let app = app.clone();
    let paths = paths.clone();
    tauri::async_runtime::spawn(async move {
        let (looked, unread) = within_bound(paths);
        let paths = described(looked, deps.files.clone(), KIND_WAIT).await;
        // Said even when nothing is left, so the page lets go of the name.
        let _ = app.emit_to(
            DESKTOP_WINDOW,
            PATHS_DROPPED,
            serde_json::json!({ "drop": drop, "paths": paths, "unread": unread }),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::{DirectoryEntries, OnDisk, Stored};
    use std::path::Path;
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

    /// A filesystem that answers from a list: a folder, a file, nothing
    /// there, or a stall that never answers.
    struct Answers(Vec<(&'static str, Answer)>);

    #[derive(Clone, Copy)]
    enum Answer {
        Folder,
        File,
        Gone,
        Stalled,
    }

    impl ChosenFiles for Answers {
        fn look(&self, path: &Path) -> std::io::Result<OnDisk> {
            let answer = self
                .0
                .iter()
                .find(|(named, _)| Path::new(named) == path)
                .map(|(_, answer)| *answer)
                .expect("a path the test named");
            let on_disk = |kind| OnDisk {
                kind,
                size: 0,
                stored: Stored::Locally,
            };
            match answer {
                Answer::Folder => Ok(on_disk(Kind::Directory)),
                Answer::File => Ok(on_disk(Kind::Regular)),
                Answer::Gone => Err(std::io::ErrorKind::NotFound.into()),
                Answer::Stalled => {
                    std::thread::sleep(Duration::from_secs(5));
                    Ok(on_disk(Kind::Directory))
                }
            }
        }

        fn entries(&self, _: &Path, _: usize) -> std::io::Result<DirectoryEntries> {
            unreachable!("a drop on the desktop window never walks a folder")
        }

        fn bytes(&self, _: &Path, _: u64) -> std::io::Result<Vec<u8>> {
            unreachable!("a drop on the desktop window never reads a file")
        }
    }

    #[test]
    fn dropped_paths_say_file_or_folder_in_order_and_leave_out_what_is_gone() {
        let files = Arc::new(Answers(vec![
            ("/work/brief.md", Answer::File),
            ("/work/gone", Answer::Gone),
            ("/work/docs", Answer::Folder),
        ]));
        let said = tauri::async_runtime::block_on(described(
            vec![
                "/work/brief.md".into(),
                "/work/gone".into(),
                "/work/docs".into(),
            ],
            files,
            Duration::from_secs(5),
        ));
        assert_eq!(
            said,
            vec![
                DroppedPath {
                    path: "/work/brief.md".to_owned(),
                    kind: DroppedKind::File,
                },
                DroppedPath {
                    path: "/work/docs".to_owned(),
                    kind: DroppedKind::Folder,
                },
            ]
        );
        let shape = serde_json::to_value(&said[1]).unwrap();
        assert_eq!(shape["kind"], "folder");
    }

    #[test]
    fn a_drop_looks_at_no_more_paths_than_a_channel_holds() {
        let most = most_dropped();
        assert!(most > 0);
        let many: Vec<PathBuf> = (0..most + 5)
            .map(|n| PathBuf::from(format!("/p/{n}")))
            .collect();
        let (looked, unread) = within_bound(many);
        assert_eq!(looked.len(), most);
        assert_eq!(looked[0], PathBuf::from("/p/0"));
        assert_eq!(unread, 5);
        assert_eq!(within_bound(vec!["/a".into()]), (vec!["/a".into()], 0));
    }

    #[test]
    fn stalled_paths_hold_the_drop_for_one_wait_and_count_as_files() {
        let files = Arc::new(Answers(vec![
            ("/share/a", Answer::Stalled),
            ("/share/b", Answer::Stalled),
            ("/share/c", Answer::Stalled),
            ("/work/docs", Answer::Folder),
        ]));
        let started = std::time::Instant::now();
        let said = tauri::async_runtime::block_on(described(
            vec![
                "/share/a".into(),
                "/share/b".into(),
                "/share/c".into(),
                "/work/docs".into(),
            ],
            files,
            Duration::from_millis(200),
        ));
        // Three stalls looked at together wait one 200 ms, not three.
        assert!(
            started.elapsed() < Duration::from_millis(550),
            "the drop waited {:?}",
            started.elapsed()
        );
        let kinds: Vec<_> = said.iter().map(|each| each.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DroppedKind::File,
                DroppedKind::File,
                DroppedKind::File,
                DroppedKind::Folder,
            ]
        );
    }
}
