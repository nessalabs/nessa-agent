//! A chosen file that is not readable yet, and who can do something about it.
//!
//! ```text
//!   stat says dataless ──▶ Readiness::make_ready ──▶ the first handler that claims it
//!                                 │                         │
//!                                 │                    iCloud: ask, poll, land
//!                                 │                         │
//!                                 └── nothing claimed it ──▶ refused, and says
//!                                                            what to do instead
//! ```
//!
//! # What this is for
//!
//! The whole feature hands the agent a *path*. A path is only worth handing
//! over if something is there to open, and a file kept in iCloud Drive and
//! never downloaded answers a `stat` perfectly well — real name, real type,
//! real length — with nothing behind it. Nessa sent one, and the person watched
//! the agent fail to read it several minutes later with nothing on screen to
//! explain why. The file was
//! `~/Library/Mobile Documents/com~apple~CloudDocs/amica-document 2.pdf`.
//!
//! # Dispatch is on the file's *state*, never on its type
//!
//! The media type decides the route — an image is uploaded, everything else is
//! linked — and it must go on deciding only that. What decides who is asked to
//! fix an unreadable file is the state the file is in, which is a different
//! question with different answers: a PDF in iCloud and a `.swift` beside it
//! need the same handler, while a PDF in iCloud and a PDF on a stalled SMB
//! share need different ones. So the handlers below claim files by asking the
//! operating system what it is keeping, not by looking at the name.
//!
//! The other two unreadable states never reach here at all, and should not:
//! something that is not a regular file is refused on what the `stat` said, and
//! a mount that has stopped answering is refused by the deadline around the
//! `stat`. Both already have their own words. A file that is simply readable
//! pays none of this — the common path never asks.
//!
//! # What every handler owes, whichever one runs
//!
//! - **A bound.** No handler may wait forever. Waiting on somebody else's
//!   network without a deadline is the stalled-mount hang in different
//!   clothes: it would park the attach path and leave `+` dead for the session.
//! - **Nothing else waits.** This runs on the task that describes one
//!   attachment, so other attachments, other conversations and the composer
//!   stay live throughout.
//! - **A typed outcome.** [`Readied`] has three, and the panel answers each of
//!   them by name, as it answers every other host refusal.
//! - **No history.** The caller is never told which handler ran. Once the bytes
//!   are here it is an ordinary local file, and nothing downstream branches on
//!   where it used to be.
//!
//! # Two handlers, and why there is not a third
//!
//! iCloud, and "nothing can be done". Dropbox, Google Drive and Box put their
//! placeholders on the disk through File Provider extensions; they are dataless
//! in exactly the same way and **they are not handled**. Whether they
//! materialise on read is theirs to decide and neither this repository nor its
//! tests can find out, so nothing is built for them on the strength of the port
//! existing — they fall to the second handler and are refused with the sentence
//! that says to open the file once, which is true of them and is the only thing
//! that is.
//!
//! Adding one later is a file beside [`icloud`] and a line in
//! [`Readiness::new`]. If this ever grows a registry or a plugin list before
//! there is a second real provider to exercise it, it has become flexibility
//! nobody can use.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

// Apple's, whole. Everything above and below this line is about *whether* a
// file is readable and who might fix it, which every platform has; the module
// is about iCloud, which only one has. Gated rather than left compiled-but-
// unreachable, because a handler that cannot be constructed is dead code, and
// this repository builds with `-D warnings` on three platforms.
#[cfg(target_os = "macos")]
pub(super) mod icloud;

/// How long a file is given to become readable before the attach gives up.
///
/// Long enough for a document over a working connection, short enough that
/// somebody who is offline finds out rather than waiting.
pub(super) const LONGEST_READY_WAIT: Duration = Duration::from_secs(45);

/// How often a handler is asked whether it has finished.
///
/// A poll rather than a notification because the frameworks report progress on
/// queues this host does not run, and a quarter second is far below what
/// anybody notices while a tile says the file is downloading.
pub(super) const READY_POLL: Duration = Duration::from_millis(250);

/// What became of the attempt.
///
/// Two of the three are a handler's to report, and a platform with no handler
/// reaches neither: `Unhandled` answers `Refused` and nothing else. They are
/// gated to the platforms that have a handler rather than left compiled and
/// unreachable, because `-D warnings` is right about them — a state nothing
/// can produce is not a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readied {
    /// The bytes are here. The caller looks at the path again and carries on.
    #[cfg(target_os = "macos")]
    Ready,
    /// The deadline passed with the work still going. Attaching again in a
    /// moment is the answer, and nothing needs doing in between.
    #[cfg(target_os = "macos")]
    StillComing,
    /// Nothing was fetched, and why in the service's own words.
    Refused(String),
}

/// A future a handler answers with. Boxed because the handlers are behind a
/// trait object, which is what lets composition choose them.
pub type ReadyFuture<'a> = Pin<Box<dyn Future<Output = Readied> + Send + 'a>>;

/// Something that can make one class of not-yet-readable file readable.
pub trait MakeReadable: Send + Sync {
    /// Whether this handler is the one for `path`.
    ///
    /// Asked of the operating system rather than inferred from the path: a
    /// name says nothing about who is keeping a file, and a rule written over
    /// `~/Library/Mobile Documents` would miss an iCloud folder somebody has
    /// linked elsewhere and would claim a plain directory with the same name.
    fn mine(&self, path: &Path) -> bool;

    /// Bring `path`'s bytes here, taking at most `within` and asking every
    /// `poll`. Must release whatever it is holding when the deadline fires.
    fn make_ready<'a>(
        &'a self,
        path: &'a Path,
        within: Duration,
        poll: Duration,
    ) -> ReadyFuture<'a>;
}

/// The handlers, asked in order.
///
/// The last one claims everything, so the list is total and `bring` always has
/// an answer. Held as a list rather than as named fields because the order is
/// the whole of the dispatch rule and a list is how an order is written down.
pub struct Readiness {
    handlers: Vec<Arc<dyn MakeReadable>>,
}

impl Readiness {
    /// The handlers this build has, most specific first.
    pub fn new(handlers: Vec<Arc<dyn MakeReadable>>) -> Self {
        Self { handlers }
    }

    /// Make `path` readable, or say why not.
    pub(super) async fn make_ready(
        &self,
        path: &Path,
        within: Duration,
        poll: Duration,
    ) -> Readied {
        for handler in &self.handlers {
            if handler.mine(path) {
                return handler.make_ready(path, within, poll).await;
            }
        }
        // Unreachable while the last handler claims everything, and an honest
        // answer rather than a panic if one day it does not.
        Readied::Refused("nothing here can make this file readable".to_string())
    }
}

/// Told which file is being made ready, and when it stops.
///
/// A port because the panel has to hear it *while* it happens rather than in
/// the answer at the end: up to [`LONGEST_READY_WAIT`] can pass, and a panel
/// that says nothing for forty-five seconds reads as broken. Named per file,
/// not as a count, because somebody who dropped five needs to know which one.
///
/// Mechanism-free like the rest of the seam: this says a file needs a moment,
/// never what is being done about it, so a handler that is not iCloud does not
/// make the sentence a lie.
pub trait Announce: Send + Sync {
    /// This file is being made ready.
    fn readying(&self, name: &str);
    /// It is not any more — it arrived, or it will not.
    fn settled(&self, name: &str);
}

/// The panel's own event name for a file that needs a moment.
pub(super) const READYING_EVENT: &str = "nessa://attachment-readying";

/// Telling the panel, which draws a tile for it.
pub(super) struct Telling<'a>(pub &'a tauri::AppHandle);

impl Announce for Telling<'_> {
    fn readying(&self, name: &str) {
        self.say(name, true);
    }
    fn settled(&self, name: &str) {
        self.say(name, false);
    }
}

impl Telling<'_> {
    fn say(&self, name: &str, readying: bool) {
        use tauri::{Emitter, Manager};

        if let Some(panel) = self.0.get_webview_window(crate::panel::MAIN_WINDOW) {
            let _ = panel.emit(
                READYING_EVENT,
                serde_json::json!({ "name": name, "readying": readying }),
            );
        }
    }
}

/// Nobody to tell.
///
/// Test-only: both real callers — `+` and a drop — have a panel to tell, and
/// a third that did not would be a file being readied with nothing on screen
/// saying so, which is the silence this whole port exists to close.
#[cfg(test)]
pub struct Untold;

#[cfg(test)]
impl Announce for Untold {
    fn readying(&self, _name: &str) {}
    fn settled(&self, _name: &str) {}
}

/// The one that claims what is left.
///
/// Every dataless file no other handler took: another provider's placeholder,
/// or one on a platform with no iCloud at all. Nothing is attempted, because
/// there is nothing this can attempt — and saying so at once is better than a
/// deadline's worth of waiting to say the same thing.
pub(super) struct Unhandled;

impl MakeReadable for Unhandled {
    fn mine(&self, _path: &Path) -> bool {
        true
    }

    fn make_ready<'a>(
        &'a self,
        _path: &'a Path,
        _within: Duration,
        _poll: Duration,
    ) -> ReadyFuture<'a> {
        Box::pin(async {
            Readied::Refused(
                "the file is kept by a service Nessa cannot make it ready from".to_string(),
            )
        })
    }
}

/// The readiness seam composition wires.
pub fn readiness() -> Arc<Readiness> {
    Arc::new(Readiness::new(vec![
        #[cfg(target_os = "macos")]
        Arc::new(icloud::ICloud),
        Arc::new(Unhandled),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A handler that claims what it is told to and answers what it is told to.
    struct Staged {
        claims: bool,
        answer: Readied,
        asked: Mutex<Vec<PathBuf>>,
    }

    impl Staged {
        fn new(claims: bool, answer: Readied) -> Arc<Self> {
            Arc::new(Self {
                claims,
                answer,
                asked: Mutex::new(Vec::new()),
            })
        }
    }

    impl MakeReadable for Staged {
        fn mine(&self, _path: &Path) -> bool {
            self.claims
        }
        fn make_ready<'a>(
            &'a self,
            path: &'a Path,
            _within: Duration,
            _poll: Duration,
        ) -> ReadyFuture<'a> {
            self.asked
                .lock()
                .expect("the paths asked about")
                .push(path.to_path_buf());
            let answer = self.answer.clone();
            Box::pin(async move { answer })
        }
    }

    fn bring(handlers: Vec<Arc<dyn MakeReadable>>) -> Readied {
        tauri::async_runtime::block_on(Readiness::new(handlers).make_ready(
            Path::new("/Users/dev/iCloud/report.pdf"),
            Duration::from_millis(10),
            Duration::from_millis(1),
        ))
    }

    /// The first handler that claims the file is the one that runs, and the
    /// ones after it are never asked.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_first_handler_that_claims_a_file_is_the_one_that_runs() {
        let first = Staged::new(true, Readied::Ready);
        let second = Staged::new(true, Readied::Refused("not me".to_string()));

        assert_eq!(bring(vec![first.clone(), second.clone()]), Readied::Ready);
        assert_eq!(first.asked.lock().unwrap().len(), 1);
        assert!(second.asked.lock().unwrap().is_empty());
    }

    /// A handler that does not claim the file is skipped, and the next one
    /// answers. This is the dispatch: a file iCloud is not keeping falls
    /// through to the handler that says so.
    #[test]
    fn a_handler_that_does_not_claim_a_file_is_passed_over() {
        // What it would have answered does not matter; it is never asked. Said
        // with an outcome every platform has, so the dispatch rule is tested
        // wherever there is a dispatch to test.
        let icloud = Staged::new(false, Readied::Refused("not me".to_string()));
        let unhandled = Staged::new(true, Readied::Refused("nothing to be done".to_string()));

        assert_eq!(
            bring(vec![icloud.clone(), unhandled.clone()]),
            Readied::Refused("nothing to be done".to_string())
        );
        assert!(icloud.asked.lock().unwrap().is_empty());
    }

    /// Every outcome a handler can give reaches the caller unchanged: the
    /// panel answers all three by name and none of them may be flattened here.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_outcome_reaches_the_caller_as_itself() {
        for answer in [
            Readied::Ready,
            Readied::StillComing,
            Readied::Refused("because".to_string()),
        ] {
            assert_eq!(bring(vec![Staged::new(true, answer.clone())]), answer);
        }
    }

    /// The list this build ships is total: something always claims the file, so
    /// no caller can be left without an answer.
    #[test]
    fn the_shipped_handlers_always_answer() {
        let answer = tauri::async_runtime::block_on(readiness().make_ready(
            // Not a path anything keeps, and not one that exists.
            Path::new("/nowhere/at/all.pdf"),
            Duration::from_millis(10),
            Duration::from_millis(1),
        ));

        assert!(matches!(answer, Readied::Refused(_)), "{answer:?}");
    }

    /// And the fallback answers at once rather than spending the deadline
    /// discovering it has nothing to do.
    #[test]
    fn the_fallback_refuses_immediately_rather_than_waiting() {
        let started = std::time::Instant::now();

        let answer = tauri::async_runtime::block_on(Unhandled.make_ready(
            Path::new("/Users/dev/Dropbox/report.pdf"),
            Duration::from_secs(45),
            READY_POLL,
        ));

        assert!(matches!(answer, Readied::Refused(_)), "{answer:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
