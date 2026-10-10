//! A lease's published artifacts on the environment's side (issue #701):
//! a harness asks to publish a file in its workspace, this side stages a
//! copy of it by digest, tells the gateway where, and answers the harness
//! with what the gateway made of it.
//!
//! ```text
//! nessa artifact publish PATH ──▶ the lease's publish point (NESSA_ARTIFACTS)
//!   ──▶ PublishCall ──▶ in flight < MAX_ARTIFACTS_IN_FLIGHT? no ──▶ busy
//!   ──▶ ArtifactOutbox::stage (copy under the outbox, hashed as it is copied)
//!   ──▶ ledger: published ──▶ Published{lease, artifact, file}   (no bytes)
//!   ... gateway reads the staged copy over the artifact channel ...
//!   ◀── Collected{lease, artifact, outcome} ──▶ ledger: collected
//!   ──▶ ArtifactOutbox::discard ──▶ answer the harness
//! the lease ends ──▶ every artifact not collected answered lease_ended,
//!                    its publish point closed, its outbox let go
//! ```
//!
//! Arrows are calls and frames, in order. The bytes the gateway reads are
//! the staged copy, never the workspace file: what was hashed is what is
//! read, whatever the harness does to its file afterwards. The control
//! stream carries only where the copy is and what it hashes to.
use super::serve::{LeaseLedger, LedgerEntry};
use nessa_protocol::lease::{Collection, CollectionRefusal, FromEnvironment, StagedArtifact};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

/// The environment variable a harness finds its lease's publish point in.
/// Set by this side, never by the gateway.
pub(crate) const PUBLISH_POINT_VARIABLE: &str = "NESSA_ARTIFACTS";

/// What `nessa artifact publish` asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PublishRequest {
    /// The file, as an absolute path.
    pub(crate) path: String,
    /// Its media type, when the publisher names one; else it is read from
    /// the file's extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) media_type: Option<String>,
}

/// Why this side did not publish a file at all: nothing reached the gateway.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PublishRefusal {
    /// The path is not a regular file inside the workspace, after every
    /// link in it is followed.
    OutsideWorkspace,
    /// No such file, or not one that can be read.
    Unreadable,
    /// Empty, or larger than [`nessa_protocol::lease::MAX_ARTIFACT_BYTES`].
    Size,
    /// A media type that is not lowercase `type/subtype`, or a file name
    /// that cannot be shown.
    Invalid,
    /// This lease already has as many artifacts in flight as it may.
    Busy,
    /// The copy could not be staged.
    StagingFailed,
    /// The publish could not be recorded in this side's audit.
    AuditUnavailable,
    /// The lease ended before the gateway answered.
    LeaseEnded,
}

/// What `nessa artifact publish` is answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum PublishAnswer {
    /// The conversation holds it now, verified by its digest.
    Held {
        /// Its SHA-256, lowercase hex.
        digest: String,
        /// Its length.
        size: u64,
    },
    /// The conversation already held exactly this file.
    AlreadyHeld {
        /// Its SHA-256, lowercase hex.
        digest: String,
        /// Its length.
        size: u64,
    },
    /// The gateway did not keep it.
    Refused {
        /// Why.
        reason: CollectionRefusal,
    },
    /// This side did not publish it.
    NotPublished {
        /// Why.
        reason: PublishRefusal,
    },
    /// The gateway answered, and what it answered stands, but this side
    /// could not record that answer: the publish is not reported as a
    /// success while its evidence here is missing.
    Unrecorded {
        /// What the gateway answered.
        collection: Collection,
    },
    /// The connection to the gateway was lost before it answered: the
    /// conversation may hold the file or may not, and this side cannot
    /// tell. Publishing it again under a later lease answers which.
    Unanswered,
}

/// One request on a lease's publish point, with where its answer goes.
pub(crate) struct PublishCall {
    pub(crate) request: PublishRequest,
    pub(crate) answer: oneshot::Sender<PublishAnswer>,
}

/// A lease's publish point: what its harnesses are told, and what arrives
/// on it.
pub(crate) struct PublishPoint {
    /// Where a harness reaches it: the value of [`PUBLISH_POINT_VARIABLE`].
    pub(crate) address: String,
    /// Each request, as it arrives.
    pub(crate) calls: mpsc::Receiver<PublishCall>,
}

/// Where a lease's artifacts are staged and its publish point is served.
/// Implemented in infrastructure, over the host's file system.
pub(crate) trait ArtifactOutbox: Send + Sync {
    /// Open `lease`'s publish point.
    ///
    /// # Errors
    /// It could not be opened; the lease runs without one.
    fn open(&self, lease: &str) -> std::io::Result<PublishPoint>;
    /// Copy the requested file under `lease`'s outbox as `artifact`, hashing
    /// what is copied. Blocking: called on a thread that may block.
    ///
    /// # Errors
    /// Why it was not staged; nothing is left of it.
    fn stage(
        &self,
        lease: &str,
        artifact: u32,
        request: &PublishRequest,
    ) -> Result<StagedArtifact, PublishRefusal>;
    /// Let go of `artifact`'s staged copy.
    fn discard(&self, lease: &str, artifact: u32);
    /// Close `lease`'s publish point and let go of everything it staged.
    fn close(&self, lease: &str);
}

/// Artifacts published under one lease and not yet answered, by number.
#[derive(Default)]
struct InFlight {
    next: u32,
    /// Numbers held by a publish not yet answered, staging included.
    reserved: usize,
    /// Each published artifact's wait for how it was settled.
    waiting: HashMap<u32, oneshot::Sender<Settled>>,
    /// The lease ended: nothing more is published under it.
    ended: bool,
}

/// How a published artifact was settled, recorded where it was settled:
/// as the gateway's answer arrives, or as the lease ends. So the ledger
/// orders each publish's settlement by when it happened, before an end that
/// follows it.
enum Settled {
    /// The gateway answered, and the answer is recorded.
    Answered(Collection),
    /// The gateway answered, and the answer could not be recorded.
    Unrecorded(Collection),
    /// The connection was lost before the gateway answered.
    Unanswered,
}

/// Record how `artifact` was settled, `None` meaning unanswered, and say
/// so to its publisher.
fn settle(
    ledger: &dyn LeaseLedger,
    lease: &str,
    artifact: u32,
    waiting: oneshot::Sender<Settled>,
    outcome: Option<Collection>,
) {
    let settled = match outcome {
        Some(outcome) => {
            let entry = LedgerEntry::Collected {
                lease: lease.to_owned(),
                artifact,
                outcome,
            };
            match ledger.record(&entry) {
                Ok(()) => Settled::Answered(outcome),
                Err(error) => {
                    tracing::error!(lease, artifact, %error, "an artifact's collection could not be recorded");
                    Settled::Unrecorded(outcome)
                }
            }
        }
        None => {
            let entry = LedgerEntry::Unanswered {
                lease: lease.to_owned(),
                artifact,
            };
            if let Err(error) = ledger.record(&entry) {
                tracing::error!(lease, artifact, %error, "an unanswered publish could not be recorded");
            }
            Settled::Unanswered
        }
    };
    let _ = waiting.send(settled);
}

/// One lease's artifacts, while it is held.
pub(crate) struct LeaseArtifacts {
    lease: String,
    address: String,
    in_flight: Arc<Mutex<InFlight>>,
    dispatcher: JoinHandle<()>,
    outbox: Arc<dyn ArtifactOutbox>,
    ledger: Arc<dyn LeaseLedger>,
}

impl LeaseArtifacts {
    /// Open `lease`'s publish point and answer what arrives on it. `None`
    /// when it cannot be opened: the lease runs, and its harnesses are told
    /// of no publish point.
    pub(crate) fn open(
        lease: &str,
        outbox: Arc<dyn ArtifactOutbox>,
        ledger: Arc<dyn LeaseLedger>,
        frames: mpsc::Sender<FromEnvironment>,
    ) -> Option<Self> {
        let point = match outbox.open(lease) {
            Ok(point) => point,
            Err(error) => {
                tracing::warn!(lease, %error, "the lease's publish point could not be opened; it publishes nothing");
                return None;
            }
        };
        let in_flight = Arc::new(Mutex::new(InFlight::default()));
        let dispatcher = tokio::spawn(dispatch(
            lease.to_owned(),
            point.calls,
            Publisher {
                in_flight: in_flight.clone(),
                outbox: outbox.clone(),
                ledger: ledger.clone(),
                frames,
            },
        ));
        Some(Self {
            lease: lease.to_owned(),
            address: point.address,
            in_flight,
            dispatcher,
            outbox,
            ledger,
        })
    }

    /// What a harness under this lease is told: the publish point.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// The gateway's answer for `artifact`, recorded before this returns,
    /// so before any frame that follows it. `false` when no such artifact
    /// waits for one: the frame names nothing held.
    pub(crate) fn collected(&self, artifact: u32, outcome: Collection) -> bool {
        let waiting = lock(&self.in_flight).waiting.remove(&artifact);
        match waiting {
            Some(waiting) => {
                settle(
                    self.ledger.as_ref(),
                    &self.lease,
                    artifact,
                    waiting,
                    Some(outcome),
                );
                true
            }
            None => false,
        }
    }

    /// The lease ended: nothing more is published and what it staged is let
    /// go. An artifact still in flight is answered as ended with it when the
    /// gateway ended it, since the gateway answers each before its end; when
    /// the lease was `lost` with the connection, it is left unanswered, as
    /// the gateway may have kept it.
    pub(crate) fn end(self, lost: bool) {
        self.dispatcher.abort();
        let waiting: Vec<_> = {
            let mut in_flight = lock(&self.in_flight);
            in_flight.ended = true;
            in_flight.waiting.drain().collect()
        };
        // Recorded here, before the end itself is: each settled first.
        for (artifact, waiting) in waiting {
            let outcome = (!lost).then_some(Collection::Refused {
                reason: CollectionRefusal::LeaseEnded,
            });
            settle(
                self.ledger.as_ref(),
                &self.lease,
                artifact,
                waiting,
                outcome,
            );
        }
        self.outbox.close(&self.lease);
    }
}

fn lock(in_flight: &Mutex<InFlight>) -> std::sync::MutexGuard<'_, InFlight> {
    in_flight.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
struct Publisher {
    in_flight: Arc<Mutex<InFlight>>,
    outbox: Arc<dyn ArtifactOutbox>,
    ledger: Arc<dyn LeaseLedger>,
    frames: mpsc::Sender<FromEnvironment>,
}

async fn dispatch(lease: String, mut calls: mpsc::Receiver<PublishCall>, publisher: Publisher) {
    while let Some(call) = calls.recv().await {
        let lease = lease.clone();
        let publisher = publisher.clone();
        tokio::spawn(async move {
            let answer = publisher.publish(&lease, &call.request).await;
            let _ = call.answer.send(answer);
        });
    }
}

impl Publisher {
    /// Publish one file: stage it, record it, tell the gateway, and wait for
    /// its answer, which the lease's end gives when the gateway does not.
    async fn publish(&self, lease: &str, request: &PublishRequest) -> PublishAnswer {
        let refused = |reason| PublishAnswer::NotPublished { reason };
        let artifact = {
            let mut in_flight = lock(&self.in_flight);
            if in_flight.ended {
                return refused(PublishRefusal::LeaseEnded);
            }
            // A number is held from here until the answer, staging included,
            // so the in-flight bound counts files still being copied too.
            if in_flight.reserved >= nessa_protocol::lease::MAX_ARTIFACTS_IN_FLIGHT {
                return refused(PublishRefusal::Busy);
            }
            in_flight.reserved += 1;
            let artifact = in_flight.next;
            in_flight.next = in_flight.next.wrapping_add(1);
            artifact
        };
        let answer = self.publish_as(lease, artifact, request).await;
        let mut in_flight = lock(&self.in_flight);
        in_flight.waiting.remove(&artifact);
        in_flight.reserved -= 1;
        answer
    }

    async fn publish_as(
        &self,
        lease: &str,
        artifact: u32,
        request: &PublishRequest,
    ) -> PublishAnswer {
        let refused = |reason| PublishAnswer::NotPublished { reason };
        let outbox = self.outbox.clone();
        let staging_lease = lease.to_owned();
        let staging_request = request.clone();
        let staged = match tokio::task::spawn_blocking(move || {
            outbox.stage(&staging_lease, artifact, &staging_request)
        })
        .await
        {
            Ok(Ok(staged)) => staged,
            Ok(Err(reason)) => return refused(reason),
            Err(_) => return refused(PublishRefusal::StagingFailed),
        };
        // Recorded under the same lock as the end's check, so an end settles
        // only a publish already on record, and records nothing before it.
        let (sender, answer) = oneshot::channel();
        let recorded = {
            let mut in_flight = lock(&self.in_flight);
            if in_flight.ended {
                Err(PublishRefusal::LeaseEnded)
            } else {
                let entry = LedgerEntry::Published {
                    lease: lease.to_owned(),
                    artifact,
                    digest: staged.digest.clone(),
                    size: staged.size,
                };
                match self.ledger.record(&entry) {
                    Ok(()) => {
                        in_flight.waiting.insert(artifact, sender);
                        Ok(())
                    }
                    Err(error) => {
                        tracing::error!(lease, artifact, %error, "a publish could not be recorded; it is not sent");
                        Err(PublishRefusal::AuditUnavailable)
                    }
                }
            }
        };
        if let Err(reason) = recorded {
            self.outbox.discard(lease, artifact);
            return refused(reason);
        }
        let (digest, size) = (staged.digest.clone(), staged.size);
        let frame = FromEnvironment::Published {
            lease: lease.to_owned(),
            artifact,
            file: staged,
        };
        // No frame sent: the connection is gone, and with it what the
        // gateway made of the file. Settled here unless an end already was.
        if self.frames.send(frame).await.is_err() {
            let waiting = lock(&self.in_flight).waiting.remove(&artifact);
            if let Some(waiting) = waiting {
                settle(self.ledger.as_ref(), lease, artifact, waiting, None);
            }
        }
        let settled = answer.await.unwrap_or(Settled::Unanswered);
        self.outbox.discard(lease, artifact);
        let outcome = match settled {
            Settled::Answered(outcome) => outcome,
            Settled::Unrecorded(collection) => return PublishAnswer::Unrecorded { collection },
            Settled::Unanswered => return PublishAnswer::Unanswered,
        };
        match outcome {
            Collection::Held => PublishAnswer::Held { digest, size },
            Collection::AlreadyHeld => PublishAnswer::AlreadyHeld { digest, size },
            Collection::Refused { reason } => PublishAnswer::Refused { reason },
        }
    }
}
