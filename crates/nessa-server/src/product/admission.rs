//! The gateway's admission: one capacity pool per lane, and the stage types
//! a request passes on its way to a handler.
//!
//! Which lane a method waits in is the manifest's (`lane` in
//! `protocol/product/manifest.json`, generated as [`ProductMethod::lane`]);
//! how many permits a lane holds is [`OperationalLimits`]'s. Nothing else
//! names a lane for a method.
//!
//! ```text
//! frame ──decode──▶ Request ──Admission::admit──▶ Admitted ──authorize──▶ Authorized ──▶ handler
//! ```
//!
//! A request is admitted to its lane before it is authorized: taking a
//! permit is a cheap local check, and a burst refused for capacity never
//! reaches the policy. Each stage's value is made only by that stage, so a
//! handler, which takes [`Authorized`], cannot be reached by a frame that was
//! not admitted and authorized.

use std::sync::Arc;

use nessa_protocol::product::generated::{ProductLane, ProductMethod};
use nessa_protocol::protocol::RequestFrame;
use nessa_sdk::domain::agent_execution::executions::Initiation;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::operational_limits::OperationalLimits;
use crate::attachments::application::HeldReads;

/// One capacity pool per gateway lane, shared by every socket.
///
/// The app-call lane has no pool here: an app call's capacity across sockets
/// is the conversation service's, held by the call's own task until it ends.
/// A permit held by the gateway would be let go when the socket went, while
/// the call ran on.
#[derive(Clone)]
pub(crate) struct Admission {
    requests: Arc<Semaphore>,
    /// What ends or answers work is never behind ordinary requests: closing,
    /// stopping, cancelling and answering a turn; reading a long review or
    /// question, since a person reads it before they answer; releasing an
    /// app, which ends its held calls; and ending an enrollment (design row
    /// O7). An app call has a lane of its own for the same reason: a
    /// destructive one, or a message, can wait minutes on the person's review.
    controls: Arc<Semaphore>,
    /// Physical record reads or pending record replies, kept separate from
    /// command and control admission.
    record_reads: Arc<Semaphore>,
    /// Beginning an upload opens no provider, so it does not belong behind
    /// reads and opens; and it sweeps tickets, reads holds, and writes audit
    /// records, so it must never be what keeps a permission answer or a close
    /// from being admitted.
    upload_begins: Arc<Semaphore>,
    /// Ranges waiting on the held-file store's read slots never hold the
    /// requests every other ordinary method needs.
    artifact_reads: Arc<Semaphore>,
    /// A delete can take minutes — it may launch the agent to ask it about
    /// its own record of the session — and a burst of them must never be what
    /// keeps a permission answer, a cancel, or a close from being admitted
    /// (`deletes_and_controls_never_take_each_others_place`). How many agents
    /// are asked at once is bounded again, and lower, by the conversation
    /// service.
    deletions: Arc<Semaphore>,
    /// One agent install at a time, taken by `agents.install` inside its
    /// `requests` permit.
    installs: Arc<Semaphore>,
}

impl Admission {
    pub(crate) fn new(limits: OperationalLimits) -> Self {
        Self {
            requests: Arc::new(Semaphore::new(limits.requests())),
            controls: Arc::new(Semaphore::new(limits.controls())),
            record_reads: Arc::new(Semaphore::new(limits.record_reads())),
            upload_begins: Arc::new(Semaphore::new(limits.upload_begins())),
            artifact_reads: Arc::new(Semaphore::new(HeldReads::CONCURRENT_READS)),
            deletions: Arc::new(Semaphore::new(limits.deletions())),
            installs: Arc::new(Semaphore::new(1)),
        }
    }

    /// The pool `lane` waits in, or `None` for the app-call lane.
    pub(crate) fn pool(&self, lane: ProductLane) -> Option<&Arc<Semaphore>> {
        match lane {
            ProductLane::Requests => Some(&self.requests),
            ProductLane::Controls => Some(&self.controls),
            ProductLane::RecordReads => Some(&self.record_reads),
            ProductLane::AppCalls => None,
            ProductLane::Deletions => Some(&self.deletions),
            ProductLane::UploadBegins => Some(&self.upload_begins),
            ProductLane::ArtifactReads => Some(&self.artifact_reads),
        }
    }

    /// The limit a refusal for `lane`'s capacity is noted under.
    pub(crate) const fn limit_id(lane: ProductLane) -> &'static str {
        match lane {
            ProductLane::Requests => "server.requests",
            ProductLane::Controls => "server.controls",
            ProductLane::RecordReads => "server.record_reads",
            ProductLane::AppCalls => "socket.app_calls",
            ProductLane::Deletions => "server.deletions",
            ProductLane::UploadBegins => "server.upload_begins",
            ProductLane::ArtifactReads => "server.artifact_reads",
        }
    }

    /// Admit `request` to its lane without waiting, or refuse it, naming the
    /// limit it found full.
    pub(crate) fn admit(&self, request: Request) -> Result<Admitted, LaneFull> {
        let lane = request.lane();
        let permit = match self.pool(lane) {
            Some(pool) => match pool.clone().try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    return Err(LaneFull {
                        request_id: request.frame.id,
                        limit_id: Self::limit_id(lane),
                    })
                }
            },
            None => None,
        };
        Ok(Admitted { request, permit })
    }

    /// Ordinary requests' pool: subscriptions and the browser session's HTTP
    /// routes take it themselves.
    pub(crate) fn requests(&self) -> &Arc<Semaphore> {
        &self.requests
    }

    pub(crate) fn record_reads(&self) -> &Arc<Semaphore> {
        &self.record_reads
    }

    pub(crate) fn installs(&self) -> &Arc<Semaphore> {
        &self.installs
    }

    #[cfg(test)]
    pub(crate) fn controls(&self) -> &Arc<Semaphore> {
        &self.controls
    }

    #[cfg(test)]
    pub(crate) fn deletions(&self) -> &Arc<Semaphore> {
        &self.deletions
    }

    #[cfg(test)]
    pub(crate) fn upload_begins(&self) -> &Arc<Semaphore> {
        &self.upload_begins
    }

    /// Put `pool` in `lane`'s place, for a test that closes one lane.
    #[cfg(test)]
    pub(crate) fn replace(&mut self, lane: ProductLane, pool: Arc<Semaphore>) {
        match lane {
            ProductLane::Requests => self.requests = pool,
            ProductLane::Controls => self.controls = pool,
            ProductLane::RecordReads => self.record_reads = pool,
            ProductLane::AppCalls => panic!("the app-call lane has no gateway pool"),
            ProductLane::Deletions => self.deletions = pool,
            ProductLane::UploadBegins => self.upload_begins = pool,
            ProductLane::ArtifactReads => self.artifact_reads = pool,
        }
    }
}

/// A request refused because its lane's pool was full.
pub(crate) struct LaneFull {
    /// The refused frame's id, to answer it by.
    pub(crate) request_id: String,
    /// The limit the refusal is noted under.
    pub(crate) limit_id: &'static str,
}

/// A frame that was read, with the manifest method it names.
///
/// A frame naming no manifest method is still a request: it waits in the
/// `requests` lane, as any unknown method always has, and authorization
/// refuses it `unknown_method`.
pub(crate) struct Request {
    frame: RequestFrame,
    method: Option<ProductMethod>,
}

impl Request {
    pub(crate) fn decode(frame: RequestFrame) -> Self {
        let method = ProductMethod::parse(&frame.method);
        Self { frame, method }
    }

    pub(crate) fn lane(&self) -> ProductLane {
        self.method
            .map(ProductMethod::lane)
            .unwrap_or(ProductLane::Requests)
    }

    pub(crate) fn frame(&self) -> &RequestFrame {
        &self.frame
    }

    pub(crate) fn into_frame(self) -> RequestFrame {
        self.frame
    }
}

/// A request holding its lane's permit, which it keeps until it is answered.
/// Made only by [`Admission::admit`].
pub(crate) struct Admitted {
    request: Request,
    permit: Option<OwnedSemaphorePermit>,
}

impl Admitted {
    /// A request with no permit to hold, for the tests' direct way in.
    #[cfg(test)]
    pub(crate) fn unmetered(request: Request) -> Self {
        Self {
            request,
            permit: None,
        }
    }

    pub(crate) fn request(&self) -> &Request {
        &self.request
    }

    pub(crate) fn into_parts(self) -> (Request, Option<OwnedSemaphorePermit>) {
        (self.request, self.permit)
    }
}

/// A request whose method the session may call under current policy.
///
/// Made only by the socket's authorize step, once the method's grant (or a
/// paired session's own check) admitted it; a method the manifest does not
/// declare is never authorized.
pub(crate) struct Authorized {
    method: ProductMethod,
    frame: RequestFrame,
    initiation: Initiation,
}

impl Authorized {
    /// `request` authorized as `initiation`, or `None` when it names no
    /// manifest method.
    pub(super) fn new(request: Request, initiation: Initiation) -> Option<Self> {
        Some(Self {
            method: request.method?,
            frame: request.frame,
            initiation,
        })
    }

    pub(crate) fn method(&self) -> ProductMethod {
        self.method
    }

    pub(crate) fn into_parts(self) -> (RequestFrame, Initiation) {
        (self.frame, self.initiation)
    }
}
