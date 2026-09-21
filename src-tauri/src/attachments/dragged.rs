//! What a drag carries when it carries no files: text, a URL, some HTML.
//!
//! # Why this exists at all
//!
//! Before `dragDropEnabled` was turned on, the page received every drop itself
//! and read these three off `DataTransfer`. It no longer receives any drop —
//! see [`super::dropping`] for the two vendored lines that settle that — and
//! Tauri's own `DragDropEvent` carries only `paths` and `position`
//! (`tauri-runtime-2.11.3/src/window.rs:97-119`). So a drag of selected text,
//! or of an image from a web page, would arrive as a drop with an empty path
//! list and nothing else at all. Both of those worked before. The host reads
//! them off the drag pasteboard instead and hands them over under the names
//! `DataTransfer` uses, so the panel's existing `droppedText` and
//! `droppedImageUrl` take them unchanged.
//!
//! # Why the snapshot is taken when the drag *enters*
//!
//! The obvious place is the drop. It is the wrong place. The drag pasteboard
//! belongs to the drag session, and the session ends shortly after
//! `performDragOperation:` returns — while Tauri's event does not reach any
//! handler synchronously inside that call. It is posted to the event loop
//! (`tauri-runtime-wry-2.11.4/src/lib.rs:4888-4894` sends it through the proxy),
//! so a handler reading the pasteboard runs at least one run-loop iteration
//! later, racing the teardown for a payload that would simply be missing when
//! it lost.
//!
//! `Enter` has no such race. It is posted the same way, but it fires when the
//! pointer crosses into the window, with the session certainly still alive
//! because the pointer is still inside it — and the pasteboard holds the same
//! contents for the whole session, so a snapshot taken then is the same
//! snapshot the drop would have wanted.
//!
//! # What is not verified
//!
//! The macOS adapter reads AppKit and cannot be tested here; see the module
//! header in [`super`] for the full list of what that covers. What *is* tested
//! is everything the snapshot is read by: the remembering, the drop that finds
//! it, and every way the panel turns it into an attachment or a paste.

use std::sync::{Arc, Mutex};

use serde::Serialize;

/// The three flavours of a drag, named the way `DataTransfer` names them.
///
/// Strings rather than `Option`s because that is what `getData` answers with:
/// a flavour the drag does not carry is the empty string, and the panel's
/// readers already treat it that way.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraggedText {
    /// `text/plain`.
    pub plain: String,
    /// `text/uri-list`.
    pub uri_list: String,
    /// `text/html`.
    pub html: String,
}

impl DraggedText {
    /// Whether the drag carried anything at all. A drop that produced neither
    /// files nor any of these is nothing the panel should react to.
    pub(super) fn is_empty(&self) -> bool {
        self.plain.is_empty() && self.uri_list.is_empty() && self.html.is_empty()
    }
}

/// What the drag currently over the window carries besides files.
///
/// A port because it reads the operating system's drag pasteboard, and because
/// the remembering between `Enter` and `Drop` is the part worth testing: a
/// substitute can stage a drop whose snapshot was never taken, which is what
/// happens when a drag begins outside the window.
pub trait DraggedContent: Send + Sync {
    /// What was on the pasteboard when the drag entered.
    fn entered(&self) -> DraggedText;
}

/// The host's memory of the drag currently in progress.
///
/// One at a time, because there is one pointer. A new `Enter` replaces what is
/// held; a `Leave` clears it, so a drag that left and came back empty cannot
/// paste what an earlier one was carrying.
pub struct DragBoard {
    reader: Arc<dyn Fn() -> DraggedText + Send + Sync>,
    held: Mutex<DraggedText>,
}

impl DragBoard {
    /// Read the pasteboard and remember what it said.
    pub fn entering(&self) {
        *self.held.lock().expect("the drag board") = (self.reader)();
    }

    /// Forget it. The drag left, or its drop has been answered.
    pub fn left(&self) {
        *self.held.lock().expect("the drag board") = DraggedText::default();
    }
}

impl DraggedContent for DragBoard {
    fn entered(&self) -> DraggedText {
        self.held.lock().expect("the drag board").clone()
    }
}

/// The board composition wires, reading the platform's drag pasteboard.
pub fn drag_board() -> Arc<DragBoard> {
    Arc::new(DragBoard {
        reader: Arc::new(platform::dragged_text),
        held: Mutex::new(DraggedText::default()),
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use super::DraggedText;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeString, NSPasteboardTypeURL,
    };

    /// The drag pasteboard, in the three flavours the panel reads.
    ///
    /// `NSPasteboardNameDrag` is the same pasteboard for the whole drag
    /// session, which is what makes reading it on `Enter` sound.
    pub(super) fn dragged_text() -> DraggedText {
        // Safety: every call here is a read of an AppKit pasteboard on the
        // main thread, which is where window events are delivered.
        unsafe {
            let board = NSPasteboard::pasteboardWithName(objc2_app_kit::NSPasteboardNameDrag);
            let read = |kind| {
                board
                    .stringForType(kind)
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            };
            DraggedText {
                plain: read(NSPasteboardTypeString),
                uri_list: read(NSPasteboardTypeURL),
                html: read(NSPasteboardTypeHTML),
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::DraggedText;

    /// Nothing, everywhere else. The panel is a macOS surface today and a drag
    /// of text on another platform is a thing to build when there is one to
    /// test it on — an empty answer means the drop attaches nothing, which is
    /// visibly wrong rather than quietly wrong.
    pub(super) fn dragged_text() -> DraggedText {
        DraggedText::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(answers: DraggedText) -> DragBoard {
        DragBoard {
            reader: Arc::new(move || answers.clone()),
            held: Mutex::new(DraggedText::default()),
        }
    }

    fn prose() -> DraggedText {
        DraggedText {
            plain: "some prose".to_string(),
            uri_list: String::new(),
            html: "<p>some prose</p>".to_string(),
        }
    }

    /// The whole point: what the drop sees is what the drag was carrying when
    /// it entered, not what the pasteboard says at some later moment.
    #[test]
    fn a_drop_reads_what_was_there_when_the_drag_entered() {
        let board = board(prose());

        assert_eq!(board.entered(), DraggedText::default());
        board.entering();
        assert_eq!(board.entered(), prose());
    }

    /// A drag that left takes its payload with it. Otherwise a drag of files
    /// that followed a drag of text would paste the text as well.
    #[test]
    fn a_drag_that_left_leaves_nothing_behind() {
        let board = board(prose());
        board.entering();

        board.left();

        assert_eq!(board.entered(), DraggedText::default());
        assert!(board.entered().is_empty());
    }

    /// Entering again replaces what is held rather than adding to it.
    #[test]
    fn a_second_drag_replaces_what_the_first_was_carrying() {
        let first = board(prose());
        first.entering();
        let second = board(DraggedText {
            plain: "different".to_string(),
            ..DraggedText::default()
        });

        second.entering();

        assert_eq!(second.entered().plain, "different");
        assert!(second.entered().html.is_empty());
    }

    /// A drag carrying nothing is told apart from one carrying something, so
    /// the panel can ignore a drop that was neither files nor text.
    #[test]
    fn a_drag_carrying_nothing_says_so() {
        assert!(DraggedText::default().is_empty());
        assert!(!prose().is_empty());
        assert!(!DraggedText {
            uri_list: "https://example.com/".to_string(),
            ..DraggedText::default()
        }
        .is_empty());
    }
}
