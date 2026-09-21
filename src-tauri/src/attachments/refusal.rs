//! Why nothing was attached, said once so every path says it the same way.
//!
//! A variant rather than a sentence, for the same reason as
//! [`crate::host::NotOpened`]: the panel decides what to say and this decides
//! what happened, so rewording the screen cannot break a test and confusing two
//! refusals cannot pass one.
//!
//! The reasons divide into three families, and the division is the point.
//! *The path* — a path that is not text, or names no file. *The file* — a
//! `stat` that failed, a thing that is not an ordinary file, a read that
//! failed, bytes past what the panel will hold, a filesystem that never
//! answered at all. *The ticket* — the one-shot token the picker minted was
//! never minted, has been spent, or has run out of time. A panel that cannot
//! tell "your file moved" from "you already read that" puts the wrong sentence
//! on screen and offers the wrong way out.
//!
//! One rule binds the ticket family: a ticket refusal names no file. The whole
//! reason tickets exist is that a path is not something the webview may hand
//! back, so a refusal that answered "the ticket for /Users/dev/.ssh/id_ed25519
//! is unknown" would hand out by accident exactly what it was built to withhold.
//! [`FileNotAttached::ticket_unknown`] and its two siblings leave `shown` empty,
//! and the tests below hold them to it.

use std::path::Path;

use serde::Serialize;

/// Why nothing was attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotAttached {
    /// No picker appeared, so nothing was ever chosen. The host could not put
    /// one on screen, or what came back from it was not a file on this machine.
    PickerUnavailable,
    /// A chosen path is not valid UTF-8, so Nessa cannot name it. On macOS and
    /// Linux a path is bytes and need not be text at all; carrying a lossy
    /// rendering would hand out a path that opens nothing, or the wrong thing.
    PathNotText,
    /// A chosen path is text and names no file — it ends in a root or a `..`
    /// rather than a final component. Nothing a file picker produces looks like
    /// this, and the answer is still a refusal rather than an invented name.
    PathNamesNoFile,
    /// A chosen file's size could not be read: deleted, unmounted, or made
    /// unreadable between the click and the read.
    SizeUnreadable,
    /// A chosen path names something that is not an ordinary file: a directory,
    /// a named pipe, a socket, a block or character device.
    ///
    /// Its own reason because it is its own fact, and because the refusal is
    /// made *before* anything is opened. Opening a pipe with nothing on the
    /// other end never returns, and a host that discovers that by trying has
    /// already lost the thread it was trying on.
    NotARegularFile,
    /// The filesystem never answered at all, so the host stopped waiting.
    ///
    /// Distinct from [`NotAttached::SizeUnreadable`], which is the filesystem
    /// answering with a refusal. This is a `stat`, a content-type lookup or a
    /// read that went out to a stalled network mount or sleeping medium and did
    /// not come back inside the host's own bound. The file may well be fine;
    /// what is not fine is waiting for it forever.
    FilesystemStalled,
    /// A chosen file's contents could not be read, although its size was. The
    /// separate reason from [`NotAttached::SizeUnreadable`] because it is a
    /// separate fact: the file was there a moment ago and answered a `stat`,
    /// and it is opening or reading it that failed.
    FileUnreadable,
    /// A chosen file has more bytes in it than the panel will hold — either it
    /// said so, or it turned out to once the reading started. See
    /// [`super::reading::LARGEST_ATTACHMENT_BYTES`]: this is a limit of where the bytes
    /// are going, so it is not something a person can fix by waiting.
    FileTooLarge,
    /// A file whose bytes are not on this machine: an iCloud, Dropbox, Drive or
    /// Box placeholder that has never been downloaded.
    ///
    /// Its own reason rather than folded into [`NotAttached::FileUnreadable`],
    /// because it is the only one of these the person can fix in two seconds
    /// and watch succeed — and because nothing is actually wrong. The file is
    /// fine, the path is right, and the contents are somewhere else.
    ///
    /// This is the assumption the whole feature rests on, caught at the one
    /// moment it can be: Nessa sends the agent a path rather than the bytes, so
    /// a path with nothing behind it reaches the agent as a read that fails
    /// minutes later, with nothing on screen to explain it.
    FileNotReadable,
    /// The host could not mint the one-shot ticket a chosen file is read
    /// through, so the choice cannot be completed. The operating system's
    /// random source is what failed; nothing about the file is wrong.
    TicketUnavailable,
    /// The presented ticket was never minted, or was minted so long ago that
    /// the host no longer keeps it. Nothing is read, and no path is named.
    TicketUnknown,
    /// The presented ticket was minted and has already been spent. A ticket
    /// buys one read; a second use of the same one is a bug in the caller or an
    /// attempt at a replay, and either way it is not a file failure.
    TicketAlreadyUsed,
    /// The presented ticket was minted, was never spent, and has run out of
    /// time. Choosing the file again mints a new one.
    TicketExpired,
    /// A file iCloud is fetching that had not arrived before the host's
    /// deadline. Distinct from [`NotAttached::FileNotReadable`] because the
    /// answer is different: nothing needs doing, the download is running, and
    /// attaching again in a moment works.
    FileNotReadyYet,
    /// A dropped folder holds nothing to attach. Its own reason because it is
    /// the one folder outcome that is not a failure of anything: the folder
    /// was read perfectly well and there was nothing in it.
    FolderEmpty,
    /// A dropped folder holds more files than a draft can, or is deep enough
    /// that the walk gave up before finding them. One bound rather than two,
    /// because the advice is the same either way — choose the files inside it
    /// — and a person cannot act on which of the two they hit.
    FolderTooLarge,
    /// A dropped folder could not be listed. The files inside it may be
    /// perfectly readable, so the advice is to choose them directly.
    FolderUnreadable,
}

/// Which file, and why it is not being attached.
///
/// The same shape as [`crate::host::LinkNotOpened`], and for the same reasons.
/// `shown` is what can be said about the file concerned — the whole path, or
/// its name once one is known — because "a file you chose could not be read" is
/// not something anybody can act on. `detail` is the operating system's own
/// words, which belong with the diagnostics rather than on screen.
///
/// `shown` is deliberately empty for every ticket refusal. See the module
/// header: naming the file a ticket stands for would give the webview back the
/// path the ticket exists to keep from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileNotAttached {
    /// What went wrong.
    pub reason: NotAttached,
    /// The file it went wrong for, as far as it can be said.
    pub shown: Option<String>,
    /// The technical reason, where there is one.
    pub detail: Option<String>,
}

impl FileNotAttached {
    /// Nothing usable came back from the picker, and why not.
    pub(super) fn picker_unavailable(detail: &str) -> Self {
        Self {
            reason: NotAttached::PickerUnavailable,
            shown: None,
            detail: Some(detail.to_string()),
        }
    }

    /// A path that is not text. `shown` is the closest rendering that can be
    /// made of it — lossy, and never used as a path: it is there so the person
    /// can tell which of the files they chose is the one being refused.
    pub(super) fn path_not_text(path: &Path) -> Self {
        Self {
            reason: NotAttached::PathNotText,
            shown: Some(path.to_string_lossy().into_owned()),
            detail: None,
        }
    }

    /// A path with no final component to use as a name.
    pub(super) fn path_names_no_file(path: &str) -> Self {
        Self {
            reason: NotAttached::PathNamesNoFile,
            shown: Some(path.to_string()),
            detail: None,
        }
    }

    /// A file whose size the filesystem would not report.
    pub(super) fn size_unreadable(name: &str, error: &std::io::Error) -> Self {
        Self {
            reason: NotAttached::SizeUnreadable,
            shown: Some(name.to_string()),
            detail: Some(error.to_string()),
        }
    }

    /// A path that names something other than an ordinary file. `what` is what
    /// it turned out to be, in plain words, because "that is not a file" is a
    /// sentence somebody argues with and "that is a directory" is not.
    pub(super) fn not_a_regular_file(name: &str, what: &str) -> Self {
        Self {
            reason: NotAttached::NotARegularFile,
            shown: Some(name.to_string()),
            detail: Some(what.to_string()),
        }
    }

    /// A file that is not downloaded. `detail` carries what the `stat` said, so
    /// a report can tell an iCloud placeholder from any other reason this could
    /// ever fire.
    pub(super) fn file_not_readable(name: &str, what: &str) -> Self {
        Self {
            reason: NotAttached::FileNotReadable,
            shown: Some(name.to_string()),
            detail: Some(what.to_string()),
        }
    }

    /// A file that is still on its way. `detail` says how long was waited, so
    /// a report can tell a slow link from a download that never started.
    pub(super) fn file_not_ready_yet(name: &str, waited: std::time::Duration) -> Self {
        Self {
            reason: NotAttached::FileNotReadyYet,
            shown: Some(name.to_string()),
            detail: Some(format!("still downloading after {} s", waited.as_secs())),
        }
    }

    /// A dropped folder with nothing in it.
    pub(super) fn folder_empty() -> Self {
        Self {
            reason: NotAttached::FolderEmpty,
            shown: None,
            detail: None,
        }
    }

    /// A dropped folder past one of the walk's two bounds. `name` is whatever
    /// the walk was looking at when it stopped, which is the most useful thing
    /// it knows.
    pub(super) fn folder_too_large(name: &str) -> Self {
        Self {
            reason: NotAttached::FolderTooLarge,
            shown: Some(name.to_string()),
            detail: None,
        }
    }

    /// A dropped folder that would not list.
    pub(super) fn folder_unreadable(name: &str, error: &std::io::Error) -> Self {
        Self {
            reason: NotAttached::FolderUnreadable,
            shown: Some(name.to_string()),
            detail: Some(error.to_string()),
        }
    }

    /// A filesystem that never came back. `detail` says which way it failed to
    /// — see [`super::files::NoAnswer`], which has three and keeps them apart.
    pub(super) fn no_filesystem_answer(name: &str, detail: &str) -> Self {
        Self {
            reason: NotAttached::FilesystemStalled,
            shown: Some(name.to_string()),
            detail: Some(detail.to_string()),
        }
    }

    /// A file that would not open, or would not read to the end.
    pub(super) fn file_unreadable(name: &str, error: &std::io::Error) -> Self {
        Self {
            reason: NotAttached::FileUnreadable,
            shown: Some(name.to_string()),
            detail: Some(error.to_string()),
        }
    }

    /// A file with more bytes in it than the panel will hold.
    ///
    /// The numbers are the diagnostic rather than the sentence: the screen says
    /// the file is too large, and the log says how much too large — and says a
    /// different length for a file that declared one size and then read longer
    /// than the bound, which is the case worth being able to recognise later.
    pub(super) fn file_too_large(name: &str, length: u64, most: u64) -> Self {
        Self {
            reason: NotAttached::FileTooLarge,
            shown: Some(name.to_string()),
            detail: Some(format!(
                "{length} bytes, over the {most} the panel will hold"
            )),
        }
    }

    /// The host has no way to mint a ticket for a file it otherwise accepted.
    /// Named, unlike the three below: nothing has been minted yet, so there is
    /// no ticket whose path could leak, and the person still has to be told
    /// which of their files the choice broke on.
    pub(super) fn ticket_unavailable(name: &str, detail: &str) -> Self {
        Self {
            reason: NotAttached::TicketUnavailable,
            shown: Some(name.to_string()),
            detail: Some(detail.to_string()),
        }
    }

    /// A ticket the host has no record of. Says nothing about any file, on
    /// purpose — see the module header.
    pub(super) fn ticket_unknown() -> Self {
        Self {
            reason: NotAttached::TicketUnknown,
            shown: None,
            detail: None,
        }
    }

    /// A ticket that has already bought its one read.
    pub(super) fn ticket_already_used() -> Self {
        Self {
            reason: NotAttached::TicketAlreadyUsed,
            shown: None,
            detail: None,
        }
    }

    /// A ticket that was never spent and has run out of time.
    pub(super) fn ticket_expired() -> Self {
        Self {
            reason: NotAttached::TicketExpired,
            shown: None,
            detail: None,
        }
    }
}

/// What can be said about a file, for a refusal to point at.
///
/// Its final component where it has one, and otherwise the path as it was
/// given. Lossy, and never used as a path — it is there so the person can tell
/// which file is being refused, which is the whole job of `shown`.
pub(super) fn named(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reason the panel can be given is a name it can branch on, and the
    /// names are the contract. The panel matches on these strings to decide
    /// what to put on screen, so a Rust variant reworded without the panel's
    /// own notice table following would change what somebody is told — or tell
    /// them nothing at all.
    /// Every reason, and the name it crosses the seam as. Written once so the
    /// two tests below cannot disagree about what the seam carries.
    const EVERY_REASON: [(&NotAttached, &str); 17] = [
        (&NotAttached::PickerUnavailable, "picker-unavailable"),
        (&NotAttached::PathNotText, "path-not-text"),
        (&NotAttached::PathNamesNoFile, "path-names-no-file"),
        (&NotAttached::SizeUnreadable, "size-unreadable"),
        (&NotAttached::NotARegularFile, "not-a-regular-file"),
        (&NotAttached::FilesystemStalled, "filesystem-stalled"),
        (&NotAttached::FileUnreadable, "file-unreadable"),
        (&NotAttached::FileTooLarge, "file-too-large"),
        (&NotAttached::TicketUnavailable, "ticket-unavailable"),
        (&NotAttached::TicketUnknown, "ticket-unknown"),
        (&NotAttached::TicketAlreadyUsed, "ticket-already-used"),
        (&NotAttached::TicketExpired, "ticket-expired"),
        (&NotAttached::FileNotReadable, "file-not-readable"),
        (&NotAttached::FileNotReadyYet, "file-not-ready-yet"),
        (&NotAttached::FolderEmpty, "folder-empty"),
        (&NotAttached::FolderTooLarge, "folder-too-large"),
        (&NotAttached::FolderUnreadable, "folder-unreadable"),
    ];

    #[test]
    fn each_reason_crosses_the_seam_as_its_own_name() {
        for (reason, name) in EVERY_REASON {
            let name = format!("\"{name}\"");
            assert_eq!(
                serde_json::to_string(&reason).expect("a reason serializes"),
                name
            );
        }
    }

    /// The panel has to have words for every name this enum can send, and it
    /// had words for six of the twelve: the rest fell through a `default:` and
    /// were told as "the file picker did not open", which for a `.key` \u2014 a
    /// package, which is a directory the picker shows as a file \u2014 was simply
    /// untrue.
    ///
    /// The panel keeps its own list of the names it answers, and walks it in a
    /// test of its own. This is the other half: every name that exists here is
    /// on that list, so adding a variant and forgetting the panel fails here
    /// rather than reaching somebody as the wrong sentence.
    #[test]
    fn the_panel_has_a_word_for_every_reason_this_can_send() {
        let panel = include_str!("../../../src/panel/application/attachment-notice.ts");
        let listed = panel
            .split_once("export const hostRefusals = [")
            .expect("the panel lists the names it answers")
            .1
            .split_once(']')
            .expect("that list closes")
            .0;
        for (reason, name) in EVERY_REASON {
            let quoted = format!("\"{name}\"");
            assert!(
                listed.contains(&quoted),
                "the panel does not answer {name}, so it would be told as something else"
            );
            // And the name really is the one that crosses, not one written
            // twice and drifted.
            assert_eq!(
                serde_json::to_string(reason).expect("a reason serializes"),
                quoted
            );
        }
        // Nothing on the panel's list that this cannot send: a name answered
        // here and never sent is a sentence nobody will ever read.
        let answered = listed.matches('"').count() / 2;
        assert_eq!(answered, EVERY_REASON.len(), "{listed}");
    }

    /// No two reasons share a name. A duplicate would compile, serialize, and
    /// quietly merge two refusals the panel acts on differently.
    #[test]
    fn no_two_reasons_cross_the_seam_as_the_same_name() {
        let names = [
            NotAttached::PickerUnavailable,
            NotAttached::PathNotText,
            NotAttached::PathNamesNoFile,
            NotAttached::SizeUnreadable,
            NotAttached::NotARegularFile,
            NotAttached::FilesystemStalled,
            NotAttached::FileUnreadable,
            NotAttached::FileTooLarge,
            NotAttached::TicketUnavailable,
            NotAttached::TicketUnknown,
            NotAttached::TicketAlreadyUsed,
            NotAttached::TicketExpired,
        ]
        .map(|reason| serde_json::to_string(&reason).expect("a reason serializes"));
        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(sorted.len(), names.len(), "{names:?}");
    }

    /// The rule the whole ticket scheme rests on, asserted rather than
    /// remembered: a refusal about a ticket carries nothing about the file it
    /// stood for. `detail` too — the operating system's own words about a path
    /// are a path.
    #[test]
    fn a_ticket_refusal_names_no_file_and_no_path() {
        for refused in [
            FileNotAttached::ticket_unknown(),
            FileNotAttached::ticket_already_used(),
            FileNotAttached::ticket_expired(),
        ] {
            assert_eq!(refused.shown, None, "{refused:?}");
            assert_eq!(refused.detail, None, "{refused:?}");
        }
    }

    /// A refusal nobody can attach to a file is no better than silence, so a
    /// path with no final component still names itself.
    #[test]
    fn a_path_with_no_final_component_is_named_by_the_path_itself() {
        assert_eq!(named(Path::new("/")), "/");
        assert_eq!(named(Path::new("/Users/dev/notes.md")), "notes.md");
    }
}
