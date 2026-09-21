//! Choosing files to attach, by the path the operating system knows them by —
//! and reading one of them back when the panel needs what is inside it.
//!
//! The webview cannot see the filesystem, and a file it reads through a browser
//! file input arrives as bytes with a name and nothing else — no path, so
//! nothing an agent running on this machine could open for itself. This is the
//! other way round: the host puts the operating system's own picker on screen
//! and hands the page back the absolute path of everything the person chose.
//!
//! ```text
//!   composition ──file_picker──────▶ FilePicker ──────┐
//!               ──chosen_files──────▶ ChosenFiles ────┤
//!               ──content_types─────▶ ContentTypes ───┤
//!               ──attachment_tickets▶ AttachmentTickets
//!               ──readiness─────────▶ Readiness ──────┤
//!               ──drag_board────────▶ DragBoard ──────┤
//!                                                     │
//!   panel ──choose_attachment_files──▶ chosen_attachments ──┐
//!   OS   ──drag/drop events──────────▶ dropped_on_panel ────┤
//!                                                     │     └▶ describe_each
//!   panel ──read_attachment_bytes────▶ redeemed_bytes ──▶ attachment_bytes
//!                                                     │            │
//!                                        ChosenFile ◀──┴──▶ Vec<u8>│
//!                                                     │            │
//!                                            FileNotAttached ◀─────┘
//! ```
//! An arrow means "constructs" or "hands to". The ports are the only things
//! here that touch the world: everything the page is told is decided by
//! [`choosing::attachment`] and [`reading::attachment_bytes`], which take a
//! path and what the outside said about it as parameters and reach for nothing.
//!
//! # Two ways in, one description
//!
//! A file chosen with `+` and a file dropped on the panel are the same thing by
//! the time the page sees it: both are a `ChosenFile` with a path, a type and a
//! ticket, and both are produced by [`choosing::describe_each`] — one loop
//! asking the same outside things in the same order. Two loops would be two
//! chances to disagree about what an image is, and that disagreement is the
//! defect this feature has already had once.
//!
//! The drop half exists in this crate rather than in the page because
//! `dragDropEnabled` is on, which is the only way a dropped file's path can be
//! known and which costs the webview *every* HTML5 drag event rather than only
//! the ones carrying files. So the host also reads the drag pasteboard for a
//! drag of text or of a web-page image ([`dragged`]), walks a dropped folder
//! under bounds, and tells the panel when a drag is overhead so the drop target
//! can still be drawn. [`dropping`] carries the vendored lines that settle it.
//!
//! # Ports, and why each is its own
//!
//! [`FilePicker`] is a window the person interacts with: it takes as long as
//! they take, and its answer is a choice. [`ChosenFiles`] is the filesystem
//! being asked about a path that has *already* been chosen, and it fails for
//! its own reasons — a file deleted, unmounted or made unreadable between the
//! click and the read. Folding those together would mean a substitute could
//! only pair a choice with a working filesystem, and the case worth testing
//! most — a path the picker returned and the filesystem then refused — could
//! not be written at all.
//!
//! Asking for a chosen file's kind and length and asking for its contents are
//! not two kinds of outside thing, though, so they are two questions on one
//! port rather than a port each. They are the same filesystem, asked about the
//! same path, at the same moment, failing in the same ways. It also keeps the
//! substitute honest: a file whose length and contents *disagree* is one double
//! answering two questions, and cannot be written at all if two doubles are
//! free to be inconsistent by construction.
//!
//! [`ContentTypes`] is the platform's type database rather than the filesystem
//! — a framework call on macOS, a subprocess on Linux, nothing at all
//! elsewhere. It does not fail; it shrugs, answering with the empty string. See
//! [`content_type`] for the full argument and for which platforms have a real
//! implementation.
//!
//! [`AttachmentTickets`] is the host's own memory of what a person chose, and
//! the reason the panel can no longer name a path to read. See [`tickets`].
//!
//! [`Readiness`] is not one outside thing but a choice between several. A file
//! a cloud service is keeping answers a `stat` with a real name, type and
//! length and has nothing behind it, and who can do something about that
//! depends on which service is keeping it. It is a list of handlers, each
//! claiming the files it knows how to fetch, and it dispatches on the *state*
//! the file is in rather than on its type — the type decides the route and
//! must go on deciding only that. See [`readiness`].
//!
//! [`DragBoard`] is the drag pasteboard, which is a different outside thing
//! from the filesystem and has to be read at a different moment: when the drag
//! *enters*, because the session's payload is gone by the time Tauri's drop
//! event reaches a handler. See [`dragged`].
//!
//! # The rules the whole thing rests on
//!
//! **A file Nessa cannot carry faithfully is refused here, with a reason,
//! rather than sent hopefully.** A path that is not valid UTF-8 is a real case
//! on macOS and Linux, where a path is bytes; it is never lossily converted and
//! passed off as a path, and it is never quietly dropped from the answer
//! either. One unusable file refuses the whole selection, because a person who
//! chose five files and silently got four has been told nothing about the
//! fifth.
//!
//! **What becomes of an attached file is decided by its type, never by the
//! gesture that attached it.** The host reports the platform's own content type
//! for every path it hands over, so a file picked here and the same file
//! dropped on the panel are the same call with the same answer. The panel's
//! extension table stays where it belongs: as the fallback for a platform with
//! no answer, not as a second source of truth.
//!
//! **A path is not a capability.** [`read_attachment_bytes`] takes a one-shot
//! ticket, never a path, and the host resolves it. `ChosenFile.path` is still
//! in the answer — the panel sends it to the gateway, which is the whole
//! feature — but it has stopped being what authorises a read.
//!
//! **The host never waits on the filesystem forever, and never waits on the
//! runtime's account.** Every `stat`, type lookup and read happens on a thread
//! of its own, under a deadline, because a named pipe never answers an `open`
//! and a stalled mount never answers a `stat`. This is the correction of a
//! claim an earlier version of this header made: the stalled-mount case was
//! handled only for a `stat` that *returned an error*. A `stat` that hangs was
//! not handled at all, and the whole panel hung with it — `+` disabled, every
//! send and drop refused, across every conversation, for as long as the file
//! took, which in one measurement was three minutes and forty-nine seconds.
//! Three things fix it, and all three are needed: the kind check refuses a pipe
//! before it is opened, the thread keeps the wait off the async runtime, and
//! the deadline covers the `stat` that hangs before any kind is known. See
//! [`files`].
//!
//! # What is not verified here, stated plainly
//!
//! Every adapter in this module is untested, and each for its own reason.
//!
//! - The picker needs a window server and somebody to click it, so the plugin's
//!   callback, the cancellation it reports as `None`, and the `FilePath`
//!   conversion are exercised only by running the app.
//! - The filesystem adapter's `std::fs::metadata`, its `File::open` and its
//!   bounded `read_to_end` have no test of their own, so nothing here proves
//!   that a real `stat` and a real read of the same real path agree, or that
//!   the bound holds against a real file on a real disk.
//! - **The per-platform content-type adapters have no test and cannot usefully
//!   have one.** The macOS answer comes from Launch Services and the Linux
//!   answer from whatever shared-mime-info is installed, so a test could only
//!   assert the machine it runs on back at itself. Every other platform,
//!   Windows included, answers with the empty string by construction. What is
//!   tested is the normalising every answer passes through, and every way the
//!   panel reads the result.
//! - The ticket desk's adapter reads the clock and asks the operating system
//!   for randomness; nothing here tests that `getrandom` is random or that
//!   `Instant::now` advances.
//! - The drag pasteboard adapter reads AppKit and needs a drag in progress to
//!   read anything, so nothing here proves that what macOS puts on
//!   `NSPasteboardNameDrag` is what a browser would have put on a
//!   `DataTransfer`. What is tested is the remembering, the lifecycle that
//!   decides which drag a payload belongs to, and every way the panel turns a
//!   payload into an attachment or a paste.
//! - The iCloud adapter asks `NSFileManager` to start a download and polls a
//!   resource key; a test could only assert this machine's own iCloud account
//!   back at itself. What is tested is the loop around it — every outcome, the
//!   deadline, and what the panel is told at each end of the wait — against a
//!   substitute that answers the same three states.
//!
//! What *is* tested is every way those answers are read, every refusal the
//! panel can be given, and the deadline itself — with an ask that never returns,
//! which is the same shape as the pipe that caused the bug. Behind a trait is
//! not tested.
//!
//! The page never talks to the dialog plugin. It calls
//! [`choose_attachment_files`] and [`read_attachment_bytes`], which are Nessa's
//! own commands, and the host calls the plugin — so `src-tauri/capabilities/`
//! grants the webview nothing new, exactly as it grants it nothing for
//! `install_update`.

mod choosing;
mod content_type;
#[cfg(test)]
mod doubles;
mod dragged;
mod dropping;
mod files;
mod picker;
mod readiness;
mod reading;
mod refusal;
mod tickets;

// The two double-underscored names beside each command are Tauri's own and not
// a naming lapse. `#[tauri::command]` emits a pair of hidden macros next to the
// function, and `generate_handler![attachments::choose_attachment_files]`
// resolves them by the *same path* as the command — so a command declared in a
// submodule has to re-export all three or the handler cannot find it. They are
// named here rather than glob-imported so that what crosses out of this module
// stays a list somebody can read.
//
// Only what the rest of the host actually names is re-exported: the two
// commands `main` registers, the drop handler `main` installs, and the ports
// and factories composition wires. `ChosenFile`, `FileNotAttached` and the
// reasons cross the seam by
// being serialized rather than by being named anywhere else in this crate, so
// they stay where they are declared.
pub use choosing::{
    __cmd__choose_attachment_files, __tauri_command_name_choose_attachment_files,
    choose_attachment_files,
};
pub use content_type::{content_types, ContentTypes};
pub use dragged::{drag_board, DragBoard};
pub use dropping::dropped_on_panel;
pub use files::{chosen_files, ChosenFiles};
pub use picker::{file_picker, FilePicker};
pub use readiness::{readiness, Readiness};
pub use reading::{
    __cmd__read_attachment_bytes, __tauri_command_name_read_attachment_bytes, read_attachment_bytes,
};
pub use tickets::{attachment_tickets, AttachmentTickets};
