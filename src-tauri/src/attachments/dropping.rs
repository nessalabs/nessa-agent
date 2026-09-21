//! Files dragged onto the panel, described by the host rather than by the page.
//!
//! ```text
//!   macOS ──drag──▶ Tauri window event ──paths──▶ expand ──▶ describe_each
//!                          │                        │              │
//!                          │                   a folder's      ChosenFile[]
//!                          │                   files, bounded       │
//!                          └── no paths ──▶ the Enter snapshot      ▼
//!                                              (text, URIs)    emitted to the page
//! ```
//!
//! # Why the host describes a dropped path, and the page never names one
//!
//! The page is handed `ChosenFile`s — path, name, size, type and a one-shot
//! ticket — exactly as it is for the picker, and it calls the same panel code
//! with them. What it must never do is receive a path and hand it back for the
//! host to open, because the whole of [`super::tickets`] rests on the host
//! being the only thing that can turn a path into a read. A command taking
//! paths from the page would make a path a capability again and leave the
//! ticket desk decorative: a page made to run somebody else's code could ask
//! for any file on the machine. So the paths arrive at the host from the
//! operating system, are described here, and leave as tickets.
//!
//! # Why the webview no longer receives the drop at all
//!
//! `dragDropEnabled` was `false` (#34), which is what let the page handle drops
//! itself — and a DOM `File` deliberately carries no path, so a dragged
//! document could never be sent. Turning it on is the only way to learn a
//! dropped file's path, and it costs the page every HTML5 drop event, not just
//! the ones carrying files. That is not a guess:
//!
//! - `wry-0.55.1/src/wkwebview/drag_drop.rs:44-50` calls `super` — the real
//!   `WKWebView` handling, which is what produces the DOM events — only when
//!   the registered listener returns `false`.
//! - `tauri-runtime-wry-2.11.4/src/lib.rs:4862-4896` registers a listener that
//!   ends in an unconditional `true`.
//!
//! So with the flag on, `draggingEntered:` and `performDragOperation:` never
//! reach `WKWebView`, and the page sees nothing — no file drop, no text drop,
//! no image-from-a-web-page drop. Two of those three worked before, so the host
//! has to give them back, which is what [`super::dragged`] and the folder walk
//! below are for.
//!
//! # Folders
//!
//! A browser walks a dropped folder with `webkitGetAsEntry`, which a page that
//! no longer receives the drop cannot call. The walk moved here and carries the
//! same two bounds the panel's `readDroppedFolder` already used — twenty files
//! and a thousand entries examined — rather than inventing new ones, so a
//! folder that attached before attaches now and one that was refused is refused
//! for the same reason.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager};

use super::choosing::{describe_each, ChosenFile};
use super::content_type::ContentTypes;
use super::dragged::DraggedContent;
use super::files::{answered_within, ChosenFiles, Kind, NoAnswer};
use super::readiness::{Announce, Readiness, Telling, LONGEST_READY_WAIT};
use super::refusal::{named, FileNotAttached};
use super::tickets::AttachmentTickets;

/// Most files one dropped folder may contribute. The panel's own walk used
/// twenty and the draft holds twenty, so a folder that fills the draft is the
/// most a folder can ever be.
pub(super) const MOST_FOLDER_FILES: usize = 20;

/// Most entries the walk will look at before giving up, counting directories
/// as well as files. A tree of ten thousand empty folders holds no files and
/// would otherwise be walked in full.
pub(super) const MOST_FOLDER_ENTRIES: usize = 1_000;

/// What a drag put on the panel.
///
/// Both halves in one answer because one drop produces one of them and the
/// page has to be told which: a drag of files has no text, and a drag of text
/// has no files. `refused` rides along rather than replacing them, because a
/// selection is refused whole — there is never a partial `files` beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dropped {
    /// Files, in the order the operating system gave them, each with a ticket.
    pub files: Vec<ChosenFile>,
    /// What the drag carried when it named no files, in the three flavours
    /// `DataTransfer` names them by, so the panel's existing readers take it
    /// unchanged.
    pub text: super::dragged::DraggedText,
    /// Why nothing was attached, when that is the answer.
    pub refused: Option<FileNotAttached>,
}

/// Every file under `paths`, with folders walked one level at a time.
///
/// Pure given the port, which is what lets every bound and every refusal be a
/// line in a test: the two limits, a folder holding nothing, a folder that
/// cannot be read, and a folder whose files spill past what a draft holds.
///
/// A path that is not a directory is passed through untouched — including one
/// that is not a regular file either, because refusing a pipe is
/// [`super::choosing::attachment`]'s job and it says so in words the panel
/// already has. This only expands what it is sure is a folder.
pub(super) fn expand(
    paths: Vec<PathBuf>,
    files: &dyn ChosenFiles,
) -> Result<Vec<PathBuf>, FileNotAttached> {
    let mut found = Vec::new();
    let mut examined = 0_usize;
    let mut walked = false;
    let mut queue: Vec<PathBuf> = paths;
    queue.reverse();
    while let Some(path) = queue.pop() {
        examined += 1;
        if examined > MOST_FOLDER_ENTRIES {
            return Err(FileNotAttached::folder_too_large(&named(&path)));
        }
        let directory = matches!(files.look(&path), Ok(on_disk) if on_disk.kind == Kind::Directory);
        if !directory {
            if found.len() >= MOST_FOLDER_FILES {
                return Err(FileNotAttached::folder_too_large(&named(&path)));
            }
            found.push(path);
            continue;
        }
        walked = true;
        let mut held = match files.entries(&path) {
            Ok(held) => held,
            Err(error) => return Err(FileNotAttached::folder_unreadable(&named(&path), &error)),
        };
        // Depth first and in order, so a folder attaches the same way twice.
        held.reverse();
        queue.extend(held);
    }
    // Only a folder can be empty in a way worth saying. A drop of nothing at
    // all is not something a person did.
    if walked && found.is_empty() {
        return Err(FileNotAttached::folder_empty());
    }
    Ok(found)
}

/// One drop, from what the operating system said to what the panel is handed.
///
/// A drag with no paths is a drag of text, and its payload comes from the
/// snapshot [`super::dragged`] took when the drag entered rather than from the
/// pasteboard now; see that module for why.
///
/// The whole expansion happens on one thread under one deadline, because a
/// folder on a mount that has stopped answering hangs on `read_dir` exactly as
/// a file hangs on `stat`, and the panel must not wait for it.
pub(super) struct Describing {
    pub files: Arc<dyn ChosenFiles>,
    pub types: Arc<dyn ContentTypes>,
    pub tickets: Arc<dyn AttachmentTickets>,
    pub readiness: Arc<Readiness>,
    /// How long the filesystem is given to answer about one path.
    pub wait: Duration,
    /// How long a file that is not readable yet is given to become so.
    pub ready_wait: Duration,
}

pub(super) async fn dropped(
    paths: Vec<PathBuf>,
    with: Describing,
    dragged: &dyn DraggedContent,
    announce: &dyn Announce,
) -> Dropped {
    let Describing {
        files,
        types,
        tickets,
        readiness,
        wait,
        ready_wait,
    } = with;
    if paths.is_empty() {
        return Dropped {
            text: dragged.entered(),
            ..Dropped::default()
        };
    }
    let expanded = {
        let files = files.clone();
        let walking = paths.clone();
        answered_within(wait, move || expand(walking, files.as_ref())).await
    };
    let expanded = match expanded {
        Ok(Ok(expanded)) => expanded,
        Ok(Err(refused)) => return refused.into(),
        Err(NoAnswer { detail }) => {
            let first = paths.first().map(PathBuf::as_path).unwrap_or(Path::new(""));
            return FileNotAttached::no_filesystem_answer(&named(first), &detail).into();
        }
    };
    match describe_each(
        expanded, files, types, tickets, readiness, announce, wait, ready_wait,
    )
    .await
    {
        Ok(files) => Dropped {
            files,
            ..Dropped::default()
        },
        Err(refused) => refused.into(),
    }
}

/// The panel's own event name for a finished drop.
///
/// A Tauri event rather than a command, because a drop is something the
/// operating system tells the host about: there is no page call to answer, and
/// making one would mean the page asking "did anything get dropped?" on a
/// timer.
pub(super) const DROPPED_EVENT: &str = "nessa://attachment-dropped";

/// Whether a drag is currently over the panel.
///
/// The webview used to know this from its own `dragenter`/`dragleave` and drew
/// the drop target from it. It receives neither any more, so the host says so:
/// without this the panel would take a dropped file perfectly well and give no
/// sign, while the drag was happening, that it would.
pub(super) const DRAGGING_EVENT: &str = "nessa://attachment-dragging";

/// Handle one window drag-drop event for the panel.
///
/// `Enter` takes the pasteboard snapshot, `Leave` forgets it, and `Drop`
/// describes whatever was dropped and emits it. Everything slow happens on a
/// spawned task: this is called from the window-event handler, which runs on
/// the thread that draws.
pub fn dropped_on_panel(app: &AppHandle, event: &DragDropEvent) {
    let Some(deps) = crate::composition::resolve(app) else {
        return;
    };
    let over = |dragging: bool| {
        if let Some(panel) = app.get_webview_window(crate::panel::MAIN_WINDOW) {
            let _ = panel.emit(DRAGGING_EVENT, dragging);
        }
    };
    match event {
        DragDropEvent::Enter { .. } => {
            deps.dragging.entering();
            over(true);
        }
        DragDropEvent::Leave => {
            deps.dragging.left();
            over(false);
        }
        DragDropEvent::Drop { paths, .. } => {
            over(false);
            let app = app.clone();
            let paths = paths.clone();
            tauri::async_runtime::spawn(async move {
                let answer = dropped(
                    paths,
                    Describing {
                        files: deps.files.clone(),
                        types: deps.types.clone(),
                        tickets: deps.tickets.clone(),
                        readiness: deps.readiness.clone(),
                        wait: super::files::LONGEST_LOOK_WAIT,
                        ready_wait: LONGEST_READY_WAIT,
                    },
                    deps.dragging.as_ref(),
                    &Telling(&app),
                )
                .await;
                deps.dragging.left();
                // Nothing at all is not worth telling the panel about: a drag
                // that carried neither files nor text is something the person
                // did to another application over this window.
                if answer.files.is_empty() && answer.refused.is_none() && answer.text.is_empty() {
                    return;
                }
                if let Some(panel) = app.get_webview_window(crate::panel::MAIN_WINDOW) {
                    let _ = panel.emit(DROPPED_EVENT, answer);
                }
            });
        }
        _ => {}
    }
}

impl From<FileNotAttached> for Dropped {
    fn from(refused: FileNotAttached) -> Self {
        Self {
            refused: Some(refused),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::doubles::{FakeFiles, FakeReadiness, FakeTickets, FakeTypes};
    use crate::attachments::readiness::{Readied, Readiness, Untold};

    /// A readiness seam holding one staged handler.
    fn staged(answer: Readied) -> Arc<Readiness> {
        Arc::new(Readiness::new(vec![Arc::new(FakeReadiness::answering(
            answer,
        ))]))
    }

    /// Everything a drop is described with, staged around one filesystem.
    fn describing(files: Arc<FakeFiles>, wait: Duration) -> Describing {
        Describing {
            files,
            types: Arc::new(FakeTypes("text/plain")),
            tickets: Arc::new(FakeTickets::for_path("a-ticket", Path::new("/unused"))),
            readiness: staged(Readied::Ready),
            wait,
            ready_wait: Duration::from_millis(50),
        }
    }
    use crate::attachments::dragged::DraggedText;
    use crate::attachments::refusal::NotAttached;
    use std::io::ErrorKind;

    /// A wait nothing in these tests reaches, so a failure is about the
    /// decision rather than about a slow machine.
    const PATIENT: Duration = Duration::from_secs(30);

    /// A drag that carries text and no files at all.
    struct Carrying(DraggedText);
    impl DraggedContent for Carrying {
        fn entered(&self) -> DraggedText {
            self.0.clone()
        }
    }
    impl Carrying {
        fn nothing() -> Self {
            Self(DraggedText::default())
        }
    }

    fn paths(of: &[&str]) -> Vec<PathBuf> {
        of.iter().map(PathBuf::from).collect()
    }

    fn drop_of(of: &[&str], files: Arc<FakeFiles>, dragged: &dyn DraggedContent) -> Dropped {
        tauri::async_runtime::block_on(dropped(
            paths(of),
            describing(files, PATIENT),
            dragged,
            &Untold,
        ))
    }

    /// An ordinary drop of two files is two described files, in order, each
    /// with a ticket — the same answer the picker gives for the same paths.
    #[test]
    fn dropped_files_are_described_exactly_as_chosen_ones_are() {
        let dropped = drop_of(
            &["/Users/dev/a.swift", "/Users/dev/b.pdf"],
            Arc::new(FakeFiles::holding(64)),
            &Carrying::nothing(),
        );

        assert_eq!(dropped.refused, None);
        assert_eq!(
            dropped
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            ["a.swift", "b.pdf"]
        );
        assert!(dropped.files.iter().all(|file| !file.ticket.is_empty()));
        assert!(dropped.files.iter().all(|file| file.size == 64));
        // The platform's type, carried through for both, which is what makes a
        // dropped file take the same route as a picked one.
        assert!(dropped
            .files
            .iter()
            .all(|file| file.mime_type == "text/plain"));
    }

    /// A drag that named no files is a drag of text, and what it carried is
    /// what the snapshot took when it entered.
    #[test]
    fn a_drag_with_no_paths_answers_with_what_it_carried() {
        let carried = DraggedText {
            plain: "some prose".to_string(),
            uri_list: "https://example.com/a.png".to_string(),
            html: "<img src=\"https://example.com/a.png\">".to_string(),
        };

        let dropped = drop_of(
            &[],
            Arc::new(FakeFiles::holding(1)),
            &Carrying(carried.clone()),
        );

        assert_eq!(dropped.text, carried);
        assert!(dropped.files.is_empty());
        assert_eq!(dropped.refused, None);
    }

    /// A folder is walked, and its files come back as ordinary attachments.
    #[test]
    fn a_dropped_folder_contributes_the_files_inside_it() {
        let files = Arc::new(FakeFiles::holding(8).tree(
            &[
                (
                    "/Users/dev/notes",
                    &["/Users/dev/notes/a.md", "/Users/dev/notes/deep"],
                ),
                ("/Users/dev/notes/deep", &["/Users/dev/notes/deep/b.md"]),
            ],
            &[
                ("/Users/dev/notes", Kind::Directory),
                ("/Users/dev/notes/deep", Kind::Directory),
            ],
        ));

        let dropped = drop_of(&["/Users/dev/notes"], files, &Carrying::nothing());

        assert_eq!(dropped.refused, None);
        assert_eq!(
            dropped
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["/Users/dev/notes/a.md", "/Users/dev/notes/deep/b.md"]
        );
    }

    /// A folder with nothing in it says so, rather than attaching nothing in
    /// silence. This is the panel's `FolderDropEmptyError`, moved.
    #[test]
    fn an_empty_folder_is_refused_as_empty() {
        let files = Arc::new(FakeFiles::holding(8).tree(
            &[("/Users/dev/empty", &[])],
            &[("/Users/dev/empty", Kind::Directory)],
        ));

        let dropped = drop_of(&["/Users/dev/empty"], files, &Carrying::nothing());

        assert_eq!(
            dropped.refused.map(|refused| refused.reason),
            Some(NotAttached::FolderEmpty)
        );
    }

    /// And a folder holding more than a draft can is refused rather than
    /// silently shortened — the panel's `FolderDropLimitError`, with the same
    /// bound it already had.
    #[test]
    fn a_folder_with_more_files_than_a_draft_holds_is_refused_whole() {
        let held: Vec<String> = (0..=MOST_FOLDER_FILES)
            .map(|n| format!("/Users/dev/many/{n}.md"))
            .collect();
        let held: Vec<&str> = held.iter().map(String::as_str).collect();
        let files = Arc::new(FakeFiles::holding(8).tree(
            &[("/Users/dev/many", &held)],
            &[("/Users/dev/many", Kind::Directory)],
        ));

        let dropped = drop_of(&["/Users/dev/many"], files, &Carrying::nothing());

        assert_eq!(
            dropped.refused.map(|refused| refused.reason),
            Some(NotAttached::FolderTooLarge)
        );
    }

    /// A tree with no files in it is still bounded: the walk gives up after
    /// `MOST_FOLDER_ENTRIES` rather than descending forever.
    #[test]
    fn a_deep_tree_of_empty_folders_stops_at_the_entry_bound() {
        let names: Vec<String> = (0..MOST_FOLDER_ENTRIES + 10)
            .map(|n| format!("/Users/dev/wide/{n}"))
            .collect();
        let inside: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut kinds: Vec<(&str, Kind)> = vec![("/Users/dev/wide", Kind::Directory)];
        kinds.extend(inside.iter().map(|at| (*at, Kind::Directory)));
        let mut tree: Vec<(&str, &[&str])> = vec![("/Users/dev/wide", inside.as_slice())];
        tree.extend(inside.iter().map(|at| (*at, [].as_slice())));
        let files = Arc::new(FakeFiles::holding(8).tree(&tree, &kinds));

        let dropped = drop_of(&["/Users/dev/wide"], files, &Carrying::nothing());

        assert_eq!(
            dropped.refused.map(|refused| refused.reason),
            Some(NotAttached::FolderTooLarge)
        );
    }

    /// A folder that will not open is its own sentence: the person can choose
    /// the files inside it instead, which is advice they can act on.
    #[test]
    fn a_folder_that_cannot_be_read_is_refused_as_that() {
        let files = Arc::new(
            FakeFiles::refusing(ErrorKind::PermissionDenied)
                .tree(&[], &[("/Users/dev/locked", Kind::Directory)]),
        );

        let dropped = drop_of(&["/Users/dev/locked"], files, &Carrying::nothing());

        // The `stat` fails first here, which is the honest answer: a path whose
        // kind is unknown is not walked on the strength of a guess.
        assert!(dropped.refused.is_some(), "{dropped:?}");
        assert!(dropped.files.is_empty());
    }

    /// A drop on a mount that has stopped answering is refused at the
    /// deadline, naming the file it was waiting on, rather than hanging the
    /// panel for as long as the filesystem takes.
    #[test]
    fn a_filesystem_that_never_answers_refuses_the_drop_at_the_deadline() {
        let dropped = tauri::async_runtime::block_on(dropped(
            paths(&["/Volumes/gone/holiday.heic"]),
            describing(
                Arc::new(FakeFiles::stalling(Duration::from_secs(30))),
                Duration::from_millis(50),
            ),
            &Carrying::nothing(),
            &Untold,
        ));

        let refused = dropped.refused.expect("a mount that is not answering");
        assert_eq!(refused.reason, NotAttached::FilesystemStalled);
        assert_eq!(refused.shown.as_deref(), Some("holiday.heic"));
    }

    /// One unusable file refuses the whole drop, exactly as it refuses a whole
    /// selection: a person who dragged five files and silently got four has
    /// been told nothing about the fifth.
    #[test]
    fn one_unusable_file_refuses_the_whole_drop() {
        let files =
            Arc::new(FakeFiles::holding(8).tree(&[], &[("/Users/dev/a-pipe", Kind::Other)]));

        let dropped = drop_of(
            &["/Users/dev/notes.md", "/Users/dev/a-pipe"],
            files,
            &Carrying::nothing(),
        );

        assert!(dropped.files.is_empty(), "{dropped:?}");
        assert_eq!(
            dropped.refused.map(|refused| refused.reason),
            Some(NotAttached::NotARegularFile)
        );
    }

    /// The shape the panel reads this by. A field renamed here without the
    /// interface following would leave the page reading `undefined` and
    /// attaching nothing at all.
    #[test]
    fn a_drop_crosses_the_seam_with_the_names_the_panel_reads() {
        let dropped = Dropped {
            files: Vec::new(),
            text: DraggedText {
                plain: "prose".to_string(),
                uri_list: "https://example.com/".to_string(),
                html: "<p>prose</p>".to_string(),
            },
            refused: None,
        };

        assert_eq!(
            serde_json::to_string(&dropped).expect("a drop serializes"),
            r#"{"files":[],"text":{"plain":"prose","uriList":"https://example.com/","html":"<p>prose</p>"},"refused":null}"#
        );
    }
}
