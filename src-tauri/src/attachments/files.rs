//! The filesystem, asked about a path the picker already answered with — and
//! the deadline the host puts on asking.
//!
//! Two things live here because they are one problem. The filesystem is the
//! only outside thing in this module that can take an *unbounded* amount of
//! time to answer: a named pipe with no writer never returns from `open`, and a
//! network mount whose server has gone never returns from `stat` either. So the
//! port is written to be called from a thread that is allowed to block, and
//! [`answered_within`] is the only sanctioned way to call it.
//!
//! ```text
//!   caller ──answered_within(wait, work)──▶ a thread of its own
//!                    │                            │
//!                    │                     ChosenFiles::look
//!                    │                     ChosenFiles::bytes
//!                    ▼                            │
//!            whichever lands first ◀───────────────┘
//!              (the answer, or the wait running out)
//! ```
//!
//! **A thread of its own, not a slot in the runtime's blocking pool.** That is
//! the whole point of the helper and it is worth saying why, because
//! `spawn_blocking` is the obvious reach. Tokio's blocking pool is bounded —
//! 512 threads by default — and a task abandoned at a deadline keeps its thread
//! forever, because nothing can cancel a thread parked inside `open(2)`. Enough
//! abandoned reads and the pool has no slot left, at which point every *other*
//! piece of blocking work the runtime has to do queues behind them and the host
//! is wedged again, in a way that is much harder to recognise than the original
//! bug. A plain thread costs one thread and one stack, shares nothing, and
//! exhausts nothing but the process's own thread limit.
//!
//! Two bounds rather than one, because the two questions have honestly
//! different worst cases. [`LONGEST_LOOK_WAIT`] covers a `stat` and a
//! content-type lookup, which on healthy media answer in microseconds;
//! [`LONGEST_READ_WAIT`] has to cover reading [`super::reading::LARGEST_ATTACHMENT_BYTES`]
//! off slow removable or network media, which honestly takes tens of seconds.
//! One bound big enough for the second would make the first useless.
//!
//! A `stat` comes *first* and its answer decides whether anything is opened at
//! all: a pipe, socket, device or directory is refused on what the `stat` said,
//! so the `open` that would never return is never made. The deadline is the
//! second line rather than the first, for the two cases the `stat` cannot
//! settle — a `stat` that hangs before it can report anything, and a path
//! replaced by a pipe in the moment between the `stat` and the open.

use std::fs::{File, Metadata};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tauri::async_runtime::channel;

/// How long the host waits for a path to describe itself.
///
/// A `stat` and a content-type lookup on working media answer in far less than
/// a millisecond; on a mount whose server has gone they answer never. Five
/// seconds is past any healthy answer, including a sleeping disk spinning up,
/// and is not long enough for somebody to conclude the app has died. A mount
/// that has not answered in five seconds is not about to.
pub const LONGEST_LOOK_WAIT: Duration = Duration::from_secs(5);

/// How long the host waits for a chosen file's bytes.
///
/// Longer than [`LONGEST_LOOK_WAIT`] because it is bounding real work rather
/// than a lookup: [`super::reading::LARGEST_ATTACHMENT_BYTES`] off a USB stick or a
/// wireless network share, at the couple of megabytes a second such media
/// honestly manage, is most of half a minute. Thirty seconds keeps the slowest
/// legitimate read and abandons a read that is not progressing.
///
/// It can afford to be generous precisely because of where it runs. The wait
/// happens on a thread of its own, so nothing about the panel is held up by it:
/// the cost of a long bound is one abandoned thread, not a frozen composer.
pub const LONGEST_READ_WAIT: Duration = Duration::from_secs(30);

/// What kind of thing a chosen path turned out to name.
///
/// Only one of these can be attached, and the other two exist so the refusal
/// can say what was there instead. A directory is worth telling apart from a
/// pipe because a person can plausibly have meant to pick a directory and
/// cannot plausibly have meant to pick a socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An ordinary file: the only thing the panel will carry.
    Regular,
    /// A directory.
    Directory,
    /// Something else entirely: a named pipe, a socket, a symbolic link to
    /// nothing, a block or character device.
    Other,
}

impl Kind {
    /// What a `stat` says the path is. `std::fs::metadata` follows symbolic
    /// links, so a link to an ordinary file is an ordinary file — which is what
    /// the person picking a macOS alias or a Linux symlink meant.
    fn of(found: &Metadata) -> Self {
        if found.is_file() {
            Self::Regular
        } else if found.is_dir() {
            Self::Directory
        } else {
            Self::Other
        }
    }

    /// The words a refusal puts in its diagnostic.
    pub fn described(self) -> &'static str {
        match self {
            Self::Regular => "an ordinary file",
            Self::Directory => "a directory, not a file",
            Self::Other => "not an ordinary file — a pipe, socket, device or broken link",
        }
    }
}

/// Whether a file's bytes are actually on this disk.
///
/// A placeholder is the case the whole feature turns on. Nessa sends the agent
/// a *path*, and the agent opens it — so a path is only worth sending if
/// something is there to open. A file stored in iCloud Drive, Dropbox, Google
/// Drive or Box and not yet downloaded answers a `stat` perfectly well, with
/// its real name, its real type and its real length, and has no contents at
/// all. Reading it either fails outright or blocks for as long as the download
/// takes, and both of those happen inside the agent, minutes later, as a string
/// of opaque failed tool calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stored {
    /// The bytes are here.
    Locally,
    /// A placeholder: name, size and type, and nothing to read.
    ///
    /// Answered from `SF_DATALESS`, which is Apple's. No other platform this
    /// runs on has a portable way to ask, so no other platform can produce
    /// this — and a variant nothing can produce is gated rather than left to
    /// look like a state the code handles.
    #[cfg(target_os = "macos")]
    Elsewhere,
}

impl Stored {
    /// What the `stat` says about whether the bytes are here.
    ///
    /// **`SF_DATALESS`, not a block count.** `st_blocks == 0 && st_size > 0` is
    /// the obvious signature and it has a false positive that matters: an
    /// ordinary sparse file reports zero blocks and a real length, and reads
    /// back perfectly well. Measured on a macOS 26 APFS volume — a 1 MiB sparse
    /// file has `blocks=0`, `flags=0x00000000` and reads fine, while a real
    /// iCloud placeholder alongside it has `blocks=0` and
    /// `flags=0x40000060`. Refusing the sparse one would turn away a file that
    /// works.
    ///
    /// **And not `NSURLUbiquitousItemDownloadingStatusKey`, although that is
    /// the documented API.** It answers for iCloud items and only for those, so
    /// it would have missed a Dropbox or Google Drive placeholder — which
    /// reaches the disk through File Provider and is dataless in exactly the
    /// same way. `SF_DATALESS` is the kernel's own materialisation flag,
    /// covers every provider, and arrives in the `stat` this already makes:
    /// no second syscall, no second port, and nothing else to hold a deadline
    /// over.
    fn of(found: &Metadata) -> Self {
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;

            /// `sys/stat.h`: "file is dataless object". Not in `libc` 0.2, so
            /// it is written out here with where it came from.
            const SF_DATALESS: u32 = 0x4000_0000;

            if found.st_flags() & SF_DATALESS != 0 {
                return Self::Elsewhere;
            }
        }
        let _ = found;
        Self::Locally
    }

    /// The words a refusal puts in its diagnostic.
    pub fn described(self) -> &'static str {
        match self {
            Self::Locally => "on this disk",
            #[cfg(target_os = "macos")]
            Self::Elsewhere => "a placeholder with no contents on this disk",
        }
    }
}

/// What one `stat` found out about a chosen path.
///
/// The kind, the length and whether the bytes are here, from one call, because
/// they have to agree: a length read separately from the kind is a length that
/// could describe a different thing than the kind did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnDisk {
    /// What the path names.
    pub kind: Kind,
    /// Its length in bytes, as the `stat` reported it. For a placeholder this
    /// is the file's real length and none of it is here; see [`Stored`].
    pub size: u64,
    /// Whether the bytes behind that length are on this disk.
    pub stored: Stored,
}

/// What the filesystem says about a file the picker already answered with.
///
/// Its own port, separate from the picker: this is the filesystem rather than
/// the window server, it is read after the choice has already been made, and it
/// fails for its own reasons — a file deleted, unmounted or made unreadable
/// between the click and the read.
///
/// Two questions on one port because they are one outside thing asked twice,
/// and because a file whose kind, length and contents disagree is a case only a
/// single substitute can stage. Both take the path exactly as it was given:
/// nothing here canonicalises, resolves or rewrites it.
///
/// **Both methods block, and that is the contract rather than an accident.**
/// They are called from inside [`answered_within`] and nowhere else, so a call
/// that never returns costs a thread the host has already decided it can lose.
pub trait ChosenFiles: Send + Sync {
    /// What the path names and how long it is, or what the operating system
    /// said instead.
    fn look(&self, path: &Path) -> std::io::Result<OnDisk>;

    /// What a directory holds, one level down, in whatever order the
    /// filesystem gives them.
    ///
    /// Here because a dropped folder is the same filesystem answering about
    /// the same path at the same moment as the `stat` that said it was a
    /// directory. A browser answers this one through `webkitGetAsEntry`, which
    /// a webview that no longer receives the drop cannot call; see
    /// [`super::dropping`].
    fn entries(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>>;

    /// The file's bytes, reading no more than `most` of them.
    ///
    /// Stopping at `most` is the contract, not a hint: the caller asks for one
    /// byte more than it is willing to keep, so an answer of exactly `most`
    /// bytes means "at least this long" rather than "this long", and a file
    /// that outgrew its own declared length is refused instead of truncated.
    /// An implementation that read the whole file and then trimmed it would
    /// satisfy the signature and defeat the entire point of the parameter.
    fn bytes(&self, path: &Path, most: u64) -> std::io::Result<Vec<u8>>;
}

/// Why a question put to the filesystem produced no answer at all.
///
/// One shape and three sentences rather than three reasons, because the panel
/// does the same thing with all of them and the person is owed the same
/// sentence: the host asked and never heard back, and the file is not at fault.
/// Which of the three it was belongs in the diagnostics, which is where
/// `detail` goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoAnswer {
    /// The host's own words for what happened.
    pub detail: String,
}

/// The answer to a blocking question, or the host giving up on it.
///
/// `ask` runs on a thread of its own and `wait` is how long its answer is worth
/// waiting for. Passing the wait in rather than reading a constant is what lets
/// a test assert the deadline in milliseconds instead of half a minute.
///
/// An abandoned `ask` is exactly that: abandoned. Nothing interrupts it, because
/// nothing can interrupt a thread parked in a kernel call — it finishes whenever
/// the filesystem lets it, finds the receiver gone, and exits. See the module
/// header for why that thread is its own and not the runtime's.
pub async fn answered_within<T: Send + 'static>(
    wait: Duration,
    ask: impl FnOnce() -> T + Send + 'static,
) -> Result<T, NoAnswer> {
    let (answered, mut answer) = channel(1);
    let started = std::thread::Builder::new()
        .name("nessa-attachment".to_string())
        .spawn(move || {
            // Nothing to do if the receiver has gone: the wait it belonged to
            // has already run out, and this answer is no longer wanted.
            let _ = answered.try_send(ask());
        });
    if let Err(error) = started {
        return Err(NoAnswer {
            detail: format!("the host could not start a thread to ask the filesystem on: {error}"),
        });
    }

    match tokio::time::timeout(wait, answer.recv()).await {
        Ok(Some(answered)) => Ok(answered),
        // The sender was dropped without sending, which means the thread
        // unwound. Reported rather than swallowed: a panic inside a filesystem
        // call is a bug worth seeing in the diagnostics, and the person still
        // gets an answer instead of a request that never settles.
        Ok(None) => Err(NoAnswer {
            detail: "the filesystem was asked on a thread that ended without answering".to_string(),
        }),
        Err(_) => Err(NoAnswer {
            detail: format!(
                "nothing came back from the filesystem in {} ms",
                wait.as_millis()
            ),
        }),
    }
}

/// The real filesystem: a `stat` and an open of the path the picker gave.
struct FilesOnDisk;

impl ChosenFiles for FilesOnDisk {
    fn look(&self, path: &Path) -> std::io::Result<OnDisk> {
        let found = std::fs::metadata(path)?;
        Ok(OnDisk {
            kind: Kind::of(&found),
            size: found.len(),
            stored: Stored::of(&found),
        })
    }

    fn entries(&self, path: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        // Sorted, so a folder attaches in the same order twice. `read_dir`
        // hands back whatever order the filesystem keeps, which on some of
        // them is insertion order and on others is a hash.
        let mut held: Vec<std::path::PathBuf> = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<_>>()?;
        held.sort();
        Ok(held)
    }

    fn bytes(&self, path: &Path, most: u64) -> std::io::Result<Vec<u8>> {
        // `take` before `read_to_end`, and an empty `Vec` rather than one sized
        // from the `stat`: the reader stops at `most` whatever the file turns
        // out to contain, and nothing is reserved on the strength of a length
        // that was measured before the file was opened.
        let mut bytes = Vec::new();
        File::open(path)?.take(most).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Where a chosen file is read from. Called from composition.
pub fn chosen_files() -> Arc<dyn ChosenFiles> {
    Arc::new(FilesOnDisk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    /// The ordinary case: an answer that arrives well inside the wait is the
    /// answer, and the helper adds nothing to it.
    #[test]
    fn an_answer_inside_the_wait_is_the_answer() {
        let answered =
            tauri::async_runtime::block_on(answered_within(Duration::from_secs(5), || {
                "a paragraph of bytes"
            }));

        assert_eq!(answered, Ok("a paragraph of bytes"));
    }

    /// The bug this exists for. The ask never returns — exactly what opening a
    /// named pipe with no writer does — and the caller is answered anyway,
    /// inside the wait rather than in three minutes and forty-nine seconds.
    #[test]
    fn an_ask_that_never_returns_is_abandoned_at_the_deadline() {
        // Held until the test ends, so the parked thread is genuinely still
        // parked when the deadline is reported rather than having quietly
        // finished first.
        let (release, blocked) = mpsc::channel::<()>();
        let started = Instant::now();

        let answered =
            tauri::async_runtime::block_on(answered_within(Duration::from_millis(50), move || {
                blocked.recv()
            }));

        let refused = answered.expect_err("nothing came back in time");
        assert!(
            refused.detail.contains("50 ms"),
            "the diagnostic says how long it waited: {refused:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the caller was answered at the deadline, not at the ask's convenience"
        );
        drop(release);
    }

    /// And the thread it abandoned is only a thread: it finishes on its own
    /// terms afterwards, with nothing to send to and nobody waiting, rather
    /// than holding a slot anything else needs.
    #[test]
    fn an_abandoned_ask_finishes_on_its_own_and_takes_nothing_with_it() {
        let (release, blocked) = mpsc::channel::<()>();
        let (finished, watch) = mpsc::channel();

        let answered =
            tauri::async_runtime::block_on(answered_within(Duration::from_millis(50), move || {
                let _ = blocked.recv();
                finished
                    .send("the ask ran to its end")
                    .expect("a live watch");
            }));
        assert!(answered.is_err(), "the caller had already been answered");

        // Nothing was cancelled, so releasing the ask lets it run to its end.
        release.send(()).expect("the ask is still parked");
        assert_eq!(
            watch
                .recv_timeout(Duration::from_secs(5))
                .expect("the abandoned ask finishes"),
            "the ask ran to its end"
        );
    }

    /// The module header's decision, made load-bearing.
    ///
    /// Every test above passes just as well if this hands its work to the
    /// runtime's blocking pool: none of them abandons more work than a pool has
    /// slots, so none can tell the two apart. Two facts can, and both are the
    /// reason the header gives. The ask runs on a thread this module named and
    /// started, which no pool's thread carries. And on a runtime with a single
    /// blocking slot, one ask abandoned at its deadline and still parked in its
    /// kernel call does not take the next one down with it — which is the whole
    /// failure, only at 512 abandoned reads instead of one.
    #[test]
    fn an_ask_runs_on_a_thread_of_its_own_rather_than_a_slot_in_a_pool() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("a runtime with room for one piece of blocking work");
        // Held until the test ends, so the abandoned ask is genuinely still
        // parked while the one after it is waiting to be answered.
        let (release, blocked) = mpsc::channel::<()>();

        runtime.block_on(async move {
            let asked_on = answered_within(Duration::from_secs(5), || {
                std::thread::current().name().map(str::to_string)
            })
            .await;
            assert_eq!(asked_on, Ok(Some("nessa-attachment".to_string())));

            let abandoned = answered_within(Duration::from_millis(50), move || blocked.recv());
            assert!(abandoned.await.is_err(), "the ask never returns");

            assert_eq!(
                answered_within(Duration::from_millis(500), || "answered anyway").await,
                Ok("answered anyway"),
                "the abandoned ask was holding the only slot the next one could have had"
            );
        });

        drop(release);
    }

    /// A panic on the asking thread is an answer of its own rather than a
    /// request that never settles — and it does not take the caller down with
    /// it.
    #[test]
    fn an_ask_that_panics_is_reported_rather_than_left_pending() {
        let answered = tauri::async_runtime::block_on(answered_within(
            Duration::from_secs(5),
            || -> &'static str { panic!("the filesystem call blew up") },
        ));

        let refused = answered.expect_err("a thread that ended has no answer");
        assert!(
            refused.detail.contains("ended without answering"),
            "{refused:?}"
        );
    }

    /// The three ways to get no answer are three sentences, so a log can tell a
    /// stalled mount from a host that has run out of threads.
    #[test]
    fn the_ways_to_get_no_answer_read_differently() {
        let stalled =
            tauri::async_runtime::block_on(answered_within(Duration::from_millis(10), || {
                std::thread::sleep(Duration::from_secs(30))
            }))
            .expect_err("nothing came back");
        let ended =
            tauri::async_runtime::block_on(answered_within(Duration::from_secs(5), || -> bool {
                panic!("gone")
            }))
            .expect_err("the thread ended");

        assert_ne!(stalled.detail, ended.detail);
    }

    /// A file's kind and the words a refusal uses for it are one decision, and
    /// each kind says something a person can act on.
    #[test]
    fn each_kind_describes_itself_differently() {
        let described = [Kind::Regular, Kind::Directory, Kind::Other].map(Kind::described);
        let mut sorted = described.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(sorted.len(), described.len(), "{described:?}");
        assert!(described[1].contains("directory"), "{described:?}");
    }
}
