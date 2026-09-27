//! The substitutes the two halves of this module are tested against.
//!
//! Test-only, and shared rather than written twice: choosing a file and reading
//! one are the same four outside things asked in a different order, so a
//! filesystem double that behaved differently in the two test modules would let
//! one of them pass on a fiction the other had already disproved.
//!
//! Every refusal the panel can be given has a constructor here, including the
//! ones no real machine can be asked to produce on demand: a path that is not
//! text, a file that answers a `stat` and will not open, a file longer than it
//! claimed, a pipe, a filesystem that never answers, a random source that has
//! failed, and a ticket presented twice or too late.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use super::content_type::ContentTypes;
use super::files::{ChosenFiles, Kind, OnDisk, Stored};
use super::picker::{FilePicker, Picked};
#[cfg(target_os = "macos")]
use super::readiness::{MakeReadable, Readied, ReadyFuture};
use super::tickets::{AttachmentTickets, NoTicket, Redeemed};

/// A picker that has already made up its mind.
///
/// Answering is the whole of what it does, which is the point: "somebody
/// cancelled" and "no picker appeared" are one line here and a window server, a
/// mouse and a person otherwise.
pub struct FakePicker(pub Picked);

impl FilePicker for FakePicker {
    fn choose(&self) -> Pin<Box<dyn Future<Output = Picked> + Send>> {
        let picked = self.0.clone();
        Box::pin(async move { picked })
    }
}

/// A filesystem that answers the same way for every path, and writes down what
/// it was asked.
///
/// One substitute for both questions, which is the point of one port: a file
/// that is absent, one that stats and will not open, one that is longer than it
/// said it was, and one that is a pipe rather than a file are each a single
/// constructor here, and the third of them cannot be staged at all by two
/// doubles that do not know about each other.
///
/// [`FakeFiles::bytes`] honours `most` by truncating, exactly as `Read::take`
/// does for the real filesystem. A substitute that ignored the bound would let
/// the bound look tested while nothing enforced it.
///
/// `stall` is how long a call parks before answering. It is what makes a
/// deadline testable in milliseconds: a pipe nobody writes to and a mount whose
/// server has gone are both, from here, a call that does not come back.
pub struct FakeFiles {
    looked: Result<OnDisk, ErrorKind>,
    contents: Result<Vec<u8>, ErrorKind>,
    stall: Duration,
    asked: Mutex<Vec<PathBuf>>,
    opened: Mutex<Vec<(PathBuf, u64)>>,
    /// What each directory holds, for the folder walk. Empty by default: most
    /// tests here are about one file and have no tree to stage.
    held: HashMap<PathBuf, Vec<PathBuf>>,
    /// What each path is, where it differs from this double's one answer.
    kinds: HashMap<PathBuf, Kind>,
    /// Whether a placeholder becomes an ordinary file after the first look.
    arrives: bool,
    /// How many times `look` has been called, for `arrives`.
    looks: Mutex<usize>,
}

impl FakeFiles {
    /// An ordinary file of `size` bytes that reads back as that many bytes.
    pub fn holding(size: u64) -> Self {
        Self::new(
            Ok(OnDisk {
                kind: Kind::Regular,
                size,
                stored: Stored::Locally,
            }),
            Ok(vec![
                b'n';
                usize::try_from(size).expect("a test-sized file")
            ]),
        )
    }

    /// A file the filesystem will not even `stat`: deleted, unmounted, or made
    /// unreadable between the click and the read.
    pub fn refusing(kind: ErrorKind) -> Self {
        Self::new(Err(kind), Err(kind))
    }

    /// A file that answers a `stat` and then will not open or will not read to
    /// the end — a permission change, a disconnected volume, bad media.
    pub fn unreadable(size: u64, kind: ErrorKind) -> Self {
        Self::new(
            Ok(OnDisk {
                kind: Kind::Regular,
                size,
                stored: Stored::Locally,
            }),
            Err(kind),
        )
    }

    /// A file whose declared length is not what it turns out to contain,
    /// because it grew between the two reads or never had a length worth
    /// believing in the first place.
    pub fn lying(declared: u64, contents: Vec<u8>) -> Self {
        Self::new(
            Ok(OnDisk {
                kind: Kind::Regular,
                size: declared,
                stored: Stored::Locally,
            }),
            Ok(contents),
        )
    }

    /// A file the filesystem describes and has no bytes for: an iCloud,
    /// Dropbox, Drive or Box placeholder. The `stat` answers with its real
    /// length, which is exactly what makes this dangerous — everything looks
    /// fine until something tries to read it.
    #[cfg(target_os = "macos")]
    pub fn dataless(size: u64) -> Self {
        Self::new(
            Ok(OnDisk {
                kind: Kind::Regular,
                size,
                stored: Stored::Elsewhere,
            }),
            Ok(Vec::new()),
        )
    }

    /// A placeholder that becomes an ordinary file after the first look —
    /// which is exactly what a file arriving looks like from here.
    #[cfg(target_os = "macos")]
    pub fn arriving(size: u64) -> Self {
        Self {
            arrives: true,
            ..Self::dataless(size)
        }
    }

    /// A path that is not an ordinary file at all. The `stat` succeeds and says
    /// so; the contents are whatever an implementation that ignored the `stat`
    /// would have got, so a test can tell "refused on the kind" from "refused
    /// on the read".
    pub fn being(kind: Kind) -> Self {
        Self::new(
            Ok(OnDisk {
                kind,
                size: 0,
                stored: Stored::Locally,
            }),
            Ok(Vec::new()),
        )
    }

    /// A filesystem that does not come back. Every call parks for `stall`,
    /// which the caller makes longer than the deadline it is testing.
    pub fn stalling(stall: Duration) -> Self {
        Self {
            stall,
            ..Self::holding(4)
        }
    }

    fn new(looked: Result<OnDisk, ErrorKind>, contents: Result<Vec<u8>, ErrorKind>) -> Self {
        Self {
            looked,
            contents,
            stall: Duration::ZERO,
            asked: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            held: HashMap::new(),
            kinds: HashMap::new(),
            arrives: false,
            looks: Mutex::new(0),
        }
    }

    /// A tree: what each directory holds, and what each path is. Paths not
    /// named here are ordinary files of the size this double was built with,
    /// which keeps a folder test to the few paths it is actually about.
    pub fn tree(mut self, held: &[(&str, &[&str])], kinds: &[(&str, Kind)]) -> Self {
        self.held = held
            .iter()
            .map(|(at, entries)| {
                (
                    PathBuf::from(at),
                    entries.iter().map(PathBuf::from).collect(),
                )
            })
            .collect();
        self.kinds = kinds
            .iter()
            .map(|(at, kind)| (PathBuf::from(at), *kind))
            .collect();
        self
    }

    /// Every path that was looked at.
    pub fn asked(&self) -> Vec<PathBuf> {
        self.asked.lock().expect("the paths asked about").clone()
    }

    /// Every read that was actually made, and how many bytes it was allowed.
    pub fn opened(&self) -> Vec<(PathBuf, u64)> {
        self.opened.lock().expect("the reads made").clone()
    }
}

impl ChosenFiles for FakeFiles {
    fn look(&self, path: &Path) -> std::io::Result<OnDisk> {
        self.asked
            .lock()
            .expect("the paths asked about")
            .push(path.to_path_buf());
        std::thread::sleep(self.stall);
        let mut looks = self.looks.lock().expect("the looks made");
        *looks += 1;
        let arrived = self.arrives && *looks > 1;
        match self.kinds.get(path) {
            Some(kind) => self.looked.map(|on_disk| OnDisk {
                kind: *kind,
                ..on_disk
            }),
            None => self.looked,
        }
        .map(|on_disk| {
            if arrived {
                OnDisk {
                    stored: Stored::Locally,
                    ..on_disk
                }
            } else {
                on_disk
            }
        })
        .map_err(Error::from)
    }

    fn entries(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.asked
            .lock()
            .expect("the paths asked about")
            .push(path.to_path_buf());
        std::thread::sleep(self.stall);
        match self.held.get(path) {
            Some(entries) => Ok(entries.clone()),
            // A directory this double was not told about reads as empty rather
            // than as an error: a test that stages one folder should not have
            // to stage every leaf it does not care about.
            None => self.looked.map(|_| Vec::new()).map_err(Error::from),
        }
    }

    fn bytes(&self, path: &Path, most: u64) -> std::io::Result<Vec<u8>> {
        self.opened
            .lock()
            .expect("the reads made")
            .push((path.to_path_buf(), most));
        std::thread::sleep(self.stall);
        let mut bytes = self.contents.clone().map_err(Error::from)?;
        bytes.truncate(usize::try_from(most).unwrap_or(usize::MAX));
        Ok(bytes)
    }
}

/// A readiness handler with one answer, and a record of what it was asked for.
///
/// Claims every file, because a test that stages a handler wants it to run.
/// The dispatch — which handler claims which file — has its own tests beside
/// the seam; this is for the callers that only need "it was made ready" or
/// "it was not".
#[cfg(target_os = "macos")]
pub struct FakeReadiness {
    answer: Readied,
    asked: Mutex<Vec<PathBuf>>,
}

#[cfg(target_os = "macos")]
impl FakeReadiness {
    /// A handler that always answers `answer`.
    pub fn answering(answer: Readied) -> Self {
        Self {
            answer,
            asked: Mutex::new(Vec::new()),
        }
    }

    /// Every path it was asked to make ready.
    pub fn asked(&self) -> Vec<PathBuf> {
        self.asked.lock().expect("the paths asked about").clone()
    }
}

#[cfg(target_os = "macos")]
impl MakeReadable for FakeReadiness {
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
        Box::pin(async move { Some(answer) })
    }
}

/// A type database with one answer, whatever it is asked.
///
/// The empty string is a platform with no opinion — Windows, or a file macOS
/// has no MIME name for — and is the case worth staging most, because it is
/// what hands the decision back to the panel's extension fallback.
pub struct FakeTypes(pub &'static str);

impl ContentTypes for FakeTypes {
    fn of(&self, _path: &Path) -> String {
        self.0.to_string()
    }
}

/// A ticket desk that answers the same way for every ticket, and writes down
/// what it was asked to mint.
///
/// Every refusal has a constructor, which is the reason the desk is a port at
/// all: a ticket presented twice and a ticket presented too late are one line
/// here and an unreachable pair of races otherwise.
pub struct FakeTickets {
    minted: Result<String, NoTicket>,
    redeems: Redeemed,
    paths: Mutex<Vec<PathBuf>>,
    presented: Mutex<Vec<String>>,
}

impl FakeTickets {
    /// A desk that mints `ticket` for everything and redeems it for `path`.
    pub fn for_path(ticket: &str, path: &Path) -> Self {
        Self::new(Ok(ticket.to_string()), Redeemed::Path(path.to_path_buf()))
    }

    /// A desk that mints, and whose tickets are then refused for `redeems`.
    pub fn refusing(redeems: Redeemed) -> Self {
        Self::new(Ok("a-ticket".to_string()), redeems)
    }

    /// A desk with no randomness behind it, so nothing can be minted at all.
    pub fn unavailable() -> Self {
        Self::new(
            Err(NoTicket {
                detail: "the operating system's random source did not answer".to_string(),
            }),
            Redeemed::Unknown,
        )
    }

    fn new(minted: Result<String, NoTicket>, redeems: Redeemed) -> Self {
        Self {
            minted,
            redeems,
            paths: Mutex::new(Vec::new()),
            presented: Mutex::new(Vec::new()),
        }
    }

    /// Every path a ticket was minted for, in order.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.paths.lock().expect("the paths minted for").clone()
    }

    /// Every ticket that was presented for redemption.
    pub fn presented(&self) -> Vec<String> {
        self.presented
            .lock()
            .expect("the tickets presented")
            .clone()
    }
}

impl AttachmentTickets for FakeTickets {
    fn mint(&self, path: &Path) -> Result<String, NoTicket> {
        self.paths
            .lock()
            .expect("the paths minted for")
            .push(path.to_path_buf());
        self.minted.clone()
    }

    fn redeem(&self, ticket: &str) -> Redeemed {
        self.presented
            .lock()
            .expect("the tickets presented")
            .push(ticket.to_string());
        self.redeems.clone()
    }
}
