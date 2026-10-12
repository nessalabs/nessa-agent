//! A lease's fetches (issue #701, slice D3b): files the conversation holds,
//! placed in the workspace for the agent's own tool call.
//!
//! ```text
//! Fetch{request, destination, digest, mediaType, size} ──▶ LeaseFetches::ask
//!   size, digest, media type unreadable; past the in-flight bound
//!     ──▶ ledger: fetched(refused) ──▶ FetchRefused{reason}
//!   ArtifactInbox::destination: inside the workspace? what is there?
//!     off on this host (NoInbox) ──▶ ledger: fetched(unavailable) ──▶ FetchRefused
//!     exactly this file ──▶ ledger: fetched(alreadyPlaced) ──▶ Fetched{alreadyPlaced}
//!     something else, no directory, outside ──▶ ledger: fetched(refused) ──▶ FetchRefused
//!     nothing ──▶ ArtifactInbox::open_part ──▶ ledger: fetch_accepted
//!       ──▶ FetchAccepted{path}   (the gateway writes the part over sftp)
//! Delivered{request} ──▶ ArtifactInbox::place: the part as it is on disk,
//!   its size and SHA-256, then linked at the destination, never over anything
//!   ──▶ ledger: fetched(outcome) ──▶ Fetched{outcome}
//!       not recorded ──▶ placed by this fetch? ArtifactInbox::take_back
//!         ──▶ Fetched{refused: auditUnavailable}
//!         could not be taken back ──▶ Fetched{placed}, unrecorded here
//! Undelivered{request} ──▶ the part let go ──▶ ledger: fetch_unanswered
//!   (still being accepted: remembered, and recorded unanswered whatever
//!   the acceptance finds, its part let go once made)
//! a request still held asked again ──▶ logged and dropped, nothing recorded
//! the lease ends ──▶ every fetch under way finishes first; each one not
//!   answered: ended by the gateway ──▶ ledger: fetched(leaseEnded) ──▶ Fetched
//!             lost with the connection ──▶ ledger: fetch_unanswered
//!   ──▶ every part of the lease let go
//! ```
//!
//! Arrows are calls and frames, in order. Every fetch asked of a held lease
//! ends in exactly one terminal ledger entry, `fetched` or
//! `fetch_unanswered`, written before the frame that says it, and each
//! carries what the fetch asked for (destination, digest, media type, size),
//! so it stands on its own. One whose entry cannot be written is answered
//! `auditUnavailable` instead, nothing it placed is kept, and the lease's
//! end is then recorded uncertain. The frames carry where the bytes go and
//! what became of them, never the bytes.
use super::serve::{FetchAsked, LeaseLedger, LedgerEntry};
use nessa_protocol::lease::{
    FetchOutcome, FetchRefusal, FromEnvironment, MAX_ARTIFACT_BYTES, MAX_FETCHES_IN_FLIGHT,
};
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

/// A file asked for, as the inbox checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FetchedFile {
    /// Its SHA-256, 64 lowercase hex digits.
    pub(crate) digest: String,
    /// Its length in bytes.
    pub(crate) size: u64,
}

/// Which file a placement linked: its device and inode, so it is taken
/// back only while its name is still that very file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

/// A file this fetch linked at its destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlacedFile {
    /// Absolute, as the gateway is told.
    pub(crate) path: String,
    pub(crate) identity: FileIdentity,
}

/// What became of a fetch's part, as the inbox placed it: the wire's
/// [`FetchOutcome`], and for a file this fetch put there, which file it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// Linked at the destination by this fetch.
    Placed(PlacedFile),
    /// Exactly this file was there already; nothing of it is this fetch's.
    AlreadyPlaced {
        path: String,
    },
    Refused(FetchRefusal),
}

impl Placement {
    /// What the gateway is told of it.
    pub(crate) fn outcome(&self) -> FetchOutcome {
        match self {
            Self::Placed(placed) => FetchOutcome::Placed {
                path: placed.path.clone(),
            },
            Self::AlreadyPlaced { path } => FetchOutcome::AlreadyPlaced { path: path.clone() },
            Self::Refused(reason) => FetchOutcome::Refused { reason: *reason },
        }
    }
}

/// What is at a fetch's destination now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    /// Nothing: the file is linked at `path` once it is verified.
    Free { path: String },
    /// Exactly this file already.
    AlreadyPlaced { path: String },
}

/// Where a lease's fetched files arrive, and how they are placed in the
/// workspace. Implemented in infrastructure over the host's file system;
/// every call may block, and is made where blocking is allowed.
pub(crate) trait ArtifactInbox: Send + Sync {
    /// Resolve `destination`, absolute or relative to the workspace, as
    /// the outbox resolves a path to publish, and say what is there.
    ///
    /// # Errors
    /// Why it cannot be placed there.
    fn destination(
        &self,
        destination: &str,
        file: &FetchedFile,
    ) -> Result<Destination, FetchRefusal>;
    /// Make the empty part fetch `fetch` of `lease` is written to, and
    /// answer its absolute path.
    ///
    /// # Errors
    /// It could not be made.
    fn open_part(&self, lease: &str, fetch: u32) -> Result<String, FetchRefusal>;
    /// Check fetch `fetch`'s part as it is on disk against `file`, then
    /// link it at `destination`, resolved again, never over anything there.
    /// The part is let go whatever the outcome.
    fn place(&self, lease: &str, fetch: u32, destination: &str, file: &FetchedFile) -> Placement;
    /// Take back a file this fetch placed, whose placing could not be
    /// recorded: removed only while its name is still that very file, so
    /// anything put there since is left. `false` when that file is still
    /// there: it could not be reached or removed.
    fn take_back(&self, placed: &PlacedFile) -> bool;
    /// Let go of fetch `fetch`'s part.
    fn discard(&self, lease: &str, fetch: u32);
    /// Let go of every part `lease` has.
    fn close(&self, lease: &str);
}

/// The inbox of a host that takes no files (`envServe.artifactsIn` off):
/// every fetch is refused `unavailable` at its destination, recorded as any
/// refusal, and nothing is ever made.
pub(crate) struct NoInbox;

impl ArtifactInbox for NoInbox {
    fn destination(
        &self,
        _destination: &str,
        _file: &FetchedFile,
    ) -> Result<Destination, FetchRefusal> {
        Err(FetchRefusal::Unavailable)
    }
    fn open_part(&self, _lease: &str, _fetch: u32) -> Result<String, FetchRefusal> {
        Err(FetchRefusal::Unavailable)
    }
    fn place(
        &self,
        _lease: &str,
        _fetch: u32,
        _destination: &str,
        _file: &FetchedFile,
    ) -> Placement {
        Placement::Refused(FetchRefusal::Unavailable)
    }
    fn take_back(&self, _placed: &PlacedFile) -> bool {
        true
    }
    fn discard(&self, _lease: &str, _fetch: u32) {}
    fn close(&self, _lease: &str) {}
}

impl FetchAsked {
    /// The file it names, as the inbox checks it.
    fn file(&self) -> FetchedFile {
        FetchedFile {
            digest: self.digest.clone(),
            size: self.size,
        }
    }
}

/// One fetch asked under a lease and not yet answered.
struct Taken {
    /// The inbox's number for it; its part is named by it, never by the
    /// gateway's request, which is any text.
    fetch: u32,
    asked: FetchAsked,
    stage: Stage,
    /// The gateway gave it up while it was still being accepted: once its
    /// part is made, it is let go instead of offered.
    undelivered: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Its destination is being checked and its part made.
    Accepting,
    /// Its part is the gateway's to write.
    Accepted,
    /// Its part is being checked and linked.
    Placing,
}

#[derive(Default)]
struct Fetching {
    next: u32,
    /// By the gateway's request.
    taken: HashMap<String, Taken>,
    /// The lease ended: nothing more is taken or placed.
    ended: bool,
    /// Its tasks: every one is waited for before the lease's end settles.
    tasks: Vec<JoinHandle<()>>,
}

/// One lease's fetches, while it is held.
pub(crate) struct LeaseFetches {
    lease: String,
    state: Arc<Mutex<Fetching>>,
    inbox: Arc<dyn ArtifactInbox>,
    ledger: Arc<dyn LeaseLedger>,
    frames: mpsc::Sender<FromEnvironment>,
    /// Set once any entry of the lease's fetches could not be recorded.
    unrecorded: Arc<AtomicBool>,
    /// True once the gateway is gone.
    gone: watch::Receiver<bool>,
}

/// Record `entry` in `ledger`, noting in `unrecorded` when it could not be.
fn record(
    ledger: &dyn LeaseLedger,
    unrecorded: &AtomicBool,
    lease: &str,
    entry: &LedgerEntry,
    what: &str,
) -> bool {
    match ledger.record(entry) {
        Ok(()) => true,
        Err(error) => {
            unrecorded.store(true, Ordering::SeqCst);
            tracing::error!(lease, %error, what, "a fetch's entry could not be recorded");
            false
        }
    }
}

fn lock(state: &Mutex<Fetching>) -> MutexGuard<'_, Fetching> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a fetch's answer comes to, decided under the lock and done after.
enum Next {
    Nothing,
    Discard(u32),
    Send(FromEnvironment),
    Unrecorded(u32, String),
    Settle(String, FetchAsked, Answer),
}

/// What answers a fetch, for the ledger and for the gateway.
enum Answer {
    Refused(FetchRefusal),
    Fetched(Placement),
    Accepted(String),
}

impl LeaseFetches {
    pub(crate) fn new(
        lease: &str,
        inbox: Arc<dyn ArtifactInbox>,
        ledger: Arc<dyn LeaseLedger>,
        frames: mpsc::Sender<FromEnvironment>,
        gone: watch::Receiver<bool>,
    ) -> Self {
        Self {
            lease: lease.to_owned(),
            state: Arc::default(),
            inbox,
            ledger,
            frames,
            unrecorded: Arc::default(),
            gone,
        }
    }

    /// Take the gateway's fetch `request`, on a task of its own, and answer
    /// it: `FetchAccepted` with its part, `Fetched` when the destination is
    /// already exactly this file, or `FetchRefused`.
    pub(crate) fn ask(&self, request: String, asked: FetchAsked) {
        let size = asked.size;
        let readable = Sha256Digest::parse(&format!("sha256:{}", asked.digest)).is_ok()
            && MediaType::parse(&asked.media_type).is_ok()
            && !asked.destination.is_empty();
        let refusal = {
            let mut state = lock(&self.state);
            if !state.ended && state.taken.contains_key(&request) {
                // Any answer to it would read as the first one's, and the
                // first one's ledger entry is its only terminal entry.
                tracing::warn!(
                    lease = self.lease,
                    request,
                    "a fetch asked again under a request still held is dropped"
                );
                return;
            }
            if state.ended {
                Some(FetchRefusal::LeaseEnded)
            } else if size == 0 || size > MAX_ARTIFACT_BYTES {
                Some(FetchRefusal::Size)
            } else if !readable {
                Some(FetchRefusal::Invalid)
            } else if state.taken.len() >= MAX_FETCHES_IN_FLIGHT {
                Some(FetchRefusal::Busy)
            } else {
                let fetch = state.next;
                state.next = state.next.wrapping_add(1);
                state.taken.insert(
                    request.clone(),
                    Taken {
                        fetch,
                        asked: asked.clone(),
                        stage: Stage::Accepting,
                        undelivered: false,
                    },
                );
                None
            }
        };
        let fetches = self.handle();
        let task = tokio::spawn(async move {
            match refusal {
                // Not taken: nothing of it is held here to wait for.
                Some(reason) => {
                    fetches
                        .settle(request, asked, Answer::Refused(reason))
                        .await;
                }
                None => fetches.accept(request, asked).await,
            }
        });
        self.spawned(task);
    }

    /// The gateway wrote every byte of `request`'s part: check it and
    /// place it, on a task of its own. `false` when no such fetch waits for
    /// its bytes: the frame names nothing held.
    pub(crate) fn delivered(&self, request: &str) -> bool {
        let taken = {
            let mut state = lock(&self.state);
            if state.ended {
                return false;
            }
            match state.taken.get_mut(request) {
                Some(taken) if taken.stage == Stage::Accepted => {
                    taken.stage = Stage::Placing;
                    (taken.fetch, taken.asked.clone())
                }
                _ => return false,
            }
        };
        let fetches = self.handle();
        let request = request.to_owned();
        let task = tokio::spawn(async move {
            let (fetch, asked) = taken;
            let inbox = fetches.inbox.clone();
            let lease = fetches.lease.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                inbox.place(&lease, fetch, &asked.destination, &asked.file())
            })
            .await
            .unwrap_or(Placement::Refused(FetchRefusal::PlacingFailed));
            fetches.answer(request, Answer::Fetched(outcome)).await;
        });
        self.spawned(task);
        true
    }

    /// The gateway gave up on `request`: its part is let go and nothing is
    /// placed. One still being accepted is let go once its part is made.
    /// `false` when no such fetch waits for its bytes.
    pub(crate) fn undelivered(&self, request: &str) -> bool {
        let fetch = {
            let mut state = lock(&self.state);
            match state.taken.get_mut(request) {
                Some(taken) if taken.stage == Stage::Accepting => {
                    taken.undelivered = true;
                    return true;
                }
                Some(taken) if taken.stage == Stage::Accepted => {
                    let fetch = taken.fetch;
                    let asked = taken.asked.clone();
                    state.taken.remove(request);
                    let entry = LedgerEntry::FetchUnanswered {
                        lease: self.lease.clone(),
                        request: request.to_owned(),
                        asked,
                    };
                    record(
                        self.ledger.as_ref(),
                        &self.unrecorded,
                        &self.lease,
                        &entry,
                        "undelivered",
                    );
                    fetch
                }
                _ => return false,
            }
        };
        self.inbox.discard(&self.lease, fetch);
        true
    }

    /// The lease ends: nothing more is taken, every fetch under way
    /// finishes, and each one still unanswered is answered `leaseEnded`
    /// when the gateway ended it, or recorded unanswered when it was `lost`
    /// with the connection; then every part is let go. Awaited before the
    /// lease's own end is recorded and answered, so each answer goes first.
    /// Resolves `false` when any entry of the lease's fetches went
    /// unrecorded, so its end is not known clean.
    pub(crate) fn end(self, lost: bool) -> impl std::future::Future<Output = bool> + Send {
        // Marked here, before anything is awaited: from now on nothing more
        // is taken or placed, whenever the returned future runs.
        let tasks = {
            let mut state = lock(&self.state);
            state.ended = true;
            std::mem::take(&mut state.tasks)
        };
        async move {
            for task in tasks {
                let _ = task.await;
            }
            let unanswered: Vec<(String, FetchAsked, bool)> = {
                let mut state = lock(&self.state);
                state
                    .taken
                    .drain()
                    .map(|(request, taken)| (request, taken.asked, taken.undelivered))
                    .collect()
            };
            let fetches = self.handle();
            for (request, asked, undelivered) in unanswered {
                // One the gateway already gave up keeps that as its cause,
                // whatever ended the lease after.
                if lost || undelivered {
                    let entry = LedgerEntry::FetchUnanswered {
                        lease: self.lease.clone(),
                        request,
                        asked,
                    };
                    record(
                        self.ledger.as_ref(),
                        &self.unrecorded,
                        &self.lease,
                        &entry,
                        if undelivered {
                            "undelivered"
                        } else {
                            "lost with its lease"
                        },
                    );
                } else {
                    let ended = Placement::Refused(FetchRefusal::LeaseEnded);
                    fetches.settle(request, asked, Answer::Fetched(ended)).await;
                }
            }
            self.inbox.close(&self.lease);
            !self.unrecorded.load(Ordering::SeqCst)
        }
    }

    fn handle(&self) -> Handle {
        Handle {
            lease: self.lease.clone(),
            state: self.state.clone(),
            inbox: self.inbox.clone(),
            ledger: self.ledger.clone(),
            frames: self.frames.clone(),
            unrecorded: self.unrecorded.clone(),
            gone: self.gone.clone(),
        }
    }

    fn spawned(&self, task: JoinHandle<()>) {
        let mut state = lock(&self.state);
        state.tasks.retain(|task| !task.is_finished());
        state.tasks.push(task);
    }
}

/// What a fetch's own task needs of its lease's fetches.
struct Handle {
    lease: String,
    state: Arc<Mutex<Fetching>>,
    inbox: Arc<dyn ArtifactInbox>,
    ledger: Arc<dyn LeaseLedger>,
    frames: mpsc::Sender<FromEnvironment>,
    unrecorded: Arc<AtomicBool>,
    gone: watch::Receiver<bool>,
}

impl Handle {
    fn record(&self, entry: &LedgerEntry, what: &str) -> bool {
        record(
            self.ledger.as_ref(),
            &self.unrecorded,
            &self.lease,
            entry,
            what,
        )
    }

    /// Check `request`'s destination and make its part, then answer it.
    async fn accept(&self, request: String, asked: FetchAsked) {
        let inbox = self.inbox.clone();
        let lease = self.lease.clone();
        let fetch = lock(&self.state)
            .taken
            .get(&request)
            .map(|taken| taken.fetch);
        let Some(fetch) = fetch else { return };
        let destination = asked.destination.clone();
        let checking = asked.file();
        let made = tokio::task::spawn_blocking(move || {
            match inbox.destination(&destination, &checking)? {
                Destination::AlreadyPlaced { path } => {
                    Ok(Answer::Fetched(Placement::AlreadyPlaced { path }))
                }
                Destination::Free { .. } => inbox.open_part(&lease, fetch).map(Answer::Accepted),
            }
        })
        .await;
        let answer = match made {
            Ok(Ok(answer)) => answer,
            Ok(Err(reason)) => Answer::Refused(reason),
            Err(_) => Answer::Refused(FetchRefusal::PlacingFailed),
        };
        self.answer(request, answer).await;
    }

    /// Record `answer` for `request` and send it, unless the lease ended
    /// while its destination was checked: its end answers it then. Only an
    /// acceptance leaves the fetch taken.
    async fn answer(&self, request: String, answer: Answer) {
        let next = {
            let mut state = lock(&self.state);
            let ended = state.ended;
            match state
                .taken
                .get(&request)
                .map(|taken| (taken.fetch, taken.stage, taken.asked.clone()))
            {
                None => Next::Nothing,
                // Nothing of it is kept; the end answers it.
                Some((fetch, Stage::Accepting, _)) if ended => Next::Discard(fetch),
                // Given up while its destination was checked: unanswered,
                // whatever the check found, and nothing said.
                Some((fetch, _, asked))
                    if state
                        .taken
                        .get(&request)
                        .is_some_and(|taken| taken.undelivered) =>
                {
                    state.taken.remove(&request);
                    let entry = LedgerEntry::FetchUnanswered {
                        lease: self.lease.clone(),
                        request,
                        asked,
                    };
                    self.record(&entry, "undelivered");
                    match answer {
                        Answer::Accepted(_) => Next::Discard(fetch),
                        _ => Next::Nothing,
                    }
                }
                Some((fetch, _, asked)) => match answer {
                    Answer::Accepted(path) => {
                        let entry = LedgerEntry::FetchAccepted {
                            lease: self.lease.clone(),
                            request: request.clone(),
                            fetch,
                            asked,
                        };
                        if self.record(&entry, "accepted; it is not taken") {
                            if let Some(taken) = state.taken.get_mut(&request) {
                                taken.stage = Stage::Accepted;
                            }
                            Next::Send(FromEnvironment::FetchAccepted {
                                lease: self.lease.clone(),
                                request,
                                path,
                            })
                        } else {
                            state.taken.remove(&request);
                            Next::Unrecorded(fetch, request)
                        }
                    }
                    answer => {
                        state.taken.remove(&request);
                        Next::Settle(request, asked, answer)
                    }
                },
            }
        };
        match next {
            Next::Nothing => {}
            Next::Discard(fetch) => self.inbox.discard(&self.lease, fetch),
            Next::Send(frame) => self.send(frame).await,
            Next::Unrecorded(fetch, request) => {
                self.inbox.discard(&self.lease, fetch);
                self.send(FromEnvironment::FetchRefused {
                    lease: self.lease.clone(),
                    request,
                    reason: FetchRefusal::AuditUnavailable,
                })
                .await;
            }
            Next::Settle(request, asked, answer) => self.settle(request, asked, answer).await,
        }
    }

    /// Record `request`'s last word and send it. A last word that cannot be
    /// recorded is never said as it was: the gateway is told
    /// `auditUnavailable`, and a file this fetch placed is taken back first,
    /// so nothing unrecorded stays placed.
    async fn settle(&self, request: String, asked: FetchAsked, answer: Answer) {
        let outcome = match &answer {
            Answer::Refused(reason) => FetchOutcome::Refused { reason: *reason },
            Answer::Fetched(placement) => placement.outcome(),
            Answer::Accepted(_) => unreachable!("an acceptance is not a last word"),
        };
        let entry = LedgerEntry::Fetched {
            lease: self.lease.clone(),
            request: request.clone(),
            asked,
            outcome: outcome.clone(),
        };
        let recorded = self.record(&entry, "its last word");
        let frame = match (answer, recorded) {
            (Answer::Refused(reason), true) => FromEnvironment::FetchRefused {
                lease: self.lease.clone(),
                request,
                reason,
            },
            (Answer::Fetched(_), true) => FromEnvironment::Fetched {
                lease: self.lease.clone(),
                request,
                outcome,
            },
            (Answer::Refused(_), false) => FromEnvironment::FetchRefused {
                lease: self.lease.clone(),
                request,
                reason: FetchRefusal::AuditUnavailable,
            },
            (Answer::Fetched(placement), false) => {
                // Ours only when this fetch linked it: a file already there
                // is left as it was.
                let kept = match placement {
                    Placement::Placed(placed) => {
                        let inbox = self.inbox.clone();
                        !tokio::task::spawn_blocking(move || inbox.take_back(&placed))
                            .await
                            .unwrap_or(false)
                    }
                    _ => false,
                };
                // One that could not be taken back is still placed: said
                // so, unrecorded here (the lease's end is then uncertain),
                // never as a refusal that placed nothing.
                FromEnvironment::Fetched {
                    lease: self.lease.clone(),
                    request,
                    outcome: match kept {
                        true => outcome,
                        false => FetchOutcome::Refused {
                            reason: FetchRefusal::AuditUnavailable,
                        },
                    },
                }
            }
            (Answer::Accepted(_), _) => unreachable!("an acceptance is not a last word"),
        };
        self.send(frame).await;
    }

    /// Queue `frame`, unless the gateway is gone first: its entry is
    /// already recorded, so a frame nobody will read is given up on rather
    /// than holding the lease's end on a full queue.
    async fn send(&self, frame: FromEnvironment) {
        let mut gone = self.gone.clone();
        tokio::select! {
            _ = self.frames.send(frame) => {}
            _ = gone.wait_for(|gone| *gone) => {}
        }
    }
}
