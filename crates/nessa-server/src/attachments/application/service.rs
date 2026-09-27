use super::{
    AttachmentAudit, AttachmentAuditRecord, AttachmentStore, AuditDelivery, AuditUnavailable,
    BeginError, Confirmation, ConversationOwnership, Discard, HoldClaim, ImageNormalizer, Kept,
    NormalizeError, Ownership, OwnershipUnavailable, ReceivedBytes, ReleaseCause, ReleaseError,
    ReleaseEvidence, RevertCause, StagedUpload, StoreUnavailable, TicketSecret, TicketSecrets,
    UploadBody, UploadError, UploadRejection,
};
use crate::{
    attachments::domain::{
        Attachment, Caller, Hold, MediaType, Redemption, TicketBook, TicketLifetime, TicketLimits,
        UploadMismatch, UploadTicket,
    },
    conversation::domain::ConversationId,
};
use nessa_auth::{
    application::ports::Clock,
    domain::{OrganizationId, PrincipalId},
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{
    sync::Semaphore,
    time::{timeout, Instant},
};

/// A close is admitted by the conversation context and then has to be written
/// down here, so the two bounds on a caller's identifiers are one rule. The
/// domain cannot read the SDK's application layer to say so; this layer can,
/// and a build stops here rather than letting a close reach a release that has
/// nobody to name.
const _: () = assert!(Caller::MAX_BYTES == ActionContext::MAX_IDENTITY_BYTES);

/// Verified session identity supplied by the gateway boundary.
#[derive(Clone, Debug)]
pub struct AttachmentCaller {
    pub organization_id: OrganizationId,
    pub principal_id: PrincipalId,
    pub surface_id: String,
}

/// One `attachment.begin`, exactly as submitted. Nothing here has been checked.
#[derive(Clone, Debug)]
pub struct BeginUpload {
    pub conversation_id: String,
    pub request_id: String,
    pub digest: String,
    pub media_type: String,
    pub size: u64,
}

/// What beginning an upload found.
#[derive(Debug, PartialEq, Eq)]
pub enum BeginOutcome {
    /// The conversation already uploaded exactly this file. The attachment is
    /// what it keeps for it, which is what a message must refer to.
    Stored(Attachment),
    /// Upload the bytes under this ticket, by this time.
    UploadRequired {
        ticket: TicketSecret,
        expires_at_ms: u64,
    },
}

/// A verified caller letting go of one conversation's files.
#[derive(Clone, Debug)]
pub struct ReleaseRequest {
    pub organization_id: OrganizationId,
    pub conversation_id: ConversationId,
    pub cause: ReleaseCause,
    pub principal_id: PrincipalId,
    pub surface_id: String,
    pub correlation_id: String,
}

/// Server-selected bounds. Callers cannot choose any of them.
#[derive(Clone, Copy, Debug)]
pub struct AttachmentLimits {
    /// Most tickets outstanding at once: in all, for one organization, and for
    /// one conversation.
    pub tickets: TicketLimits,
    /// Most transfers in progress at once, across every caller. It is not
    /// divided by organization: this gateway serves exactly one, a permit is
    /// held for at most the upload deadline, and a caller refused for want of
    /// one keeps its ticket.
    pub max_uploads: usize,
    /// Most images being normalized at once. Each holds a whole upload, up to
    /// [`Attachment::MAX_BYTES`], and its decoded pixels in memory, so this is
    /// smaller than the number of transfers, which stream to disk.
    pub max_normalizations: usize,
    /// How long one transfer may take from its first byte to its last.
    pub upload_deadline: Duration,
    /// How long one audit record may take to be acknowledged. Every record
    /// gets this much; one slow record does not spend another's time.
    pub audit_deadline: Duration,
    /// How long all the records of one phase may take together: a release's
    /// withdrawals, releases and removals, or one sweep of expired tickets.
    /// Those lists are as long as a conversation has holds or the book has
    /// tickets, so a phase without a budget would wait for as many deadlines
    /// as it found records. Records the budget does not reach are reported as
    /// unrecorded, and the cleanup they describe still happens.
    pub audit_budget: Duration,
}
impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            tickets: TicketLimits {
                total: 64,
                per_organization: 32,
                per_conversation: 16,
            },
            max_uploads: 4,
            max_normalizations: 2,
            upload_deadline: Duration::from_secs(120),
            audit_deadline: Duration::from_secs(5),
            audit_budget: Duration::from_secs(30),
        }
    }
}

/// Every port the service calls, constructed by composition and substituted in tests.
pub struct AttachmentDependencies {
    pub store: Arc<dyn AttachmentStore>,
    pub audit: Arc<dyn AttachmentAudit>,
    pub ownership: Arc<dyn ConversationOwnership>,
    pub secrets: Arc<dyn TicketSecrets>,
    pub normalizer: Arc<dyn ImageNormalizer>,
    pub clock: Arc<dyn Clock>,
}

struct Inner {
    store: Arc<dyn AttachmentStore>,
    audit: Arc<dyn AttachmentAudit>,
    ownership: Arc<dyn ConversationOwnership>,
    secrets: Arc<dyn TicketSecrets>,
    normalizer: Arc<dyn ImageNormalizer>,
    clock: Arc<dyn Clock>,
    limits: AttachmentLimits,
    book: Mutex<TicketBook>,
    uploads: Arc<Semaphore>,
    normalizations: Semaphore,
}

/// Issues tickets, receives uploads under them, and releases holds. Clones
/// share one ticket book, so the socket that issues a ticket and the route
/// that redeems it agree on what is outstanding.
#[derive(Clone)]
pub struct AttachmentService {
    inner: Arc<Inner>,
}

impl AttachmentService {
    pub fn new(dependencies: AttachmentDependencies, limits: AttachmentLimits) -> Self {
        let AttachmentDependencies {
            store,
            audit,
            ownership,
            secrets,
            normalizer,
            clock,
        } = dependencies;
        Self {
            inner: Arc::new(Inner {
                store,
                audit,
                ownership,
                secrets,
                normalizer,
                clock,
                limits,
                book: Mutex::new(TicketBook::new(limits.tickets)),
                uploads: Arc::new(Semaphore::new(limits.max_uploads)),
                normalizations: Semaphore::new(limits.max_normalizations),
            }),
        }
    }

    /// A poisoned book is still a consistent book: every method on it either
    /// completes or changes nothing, so the tickets in it remain true.
    fn book(&self) -> MutexGuard<'_, TicketBook> {
        self.inner
            .book
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Give one record its own bounded delivery attempt. The sink owns the
    /// write once started, so running out of time here abandons the wait, not
    /// the record.
    async fn audit(&self, record: AttachmentAuditRecord) -> AuditDelivery {
        match timeout(
            self.inner.limits.audit_deadline,
            self.inner.audit.record(record),
        )
        .await
        {
            Ok(Ok(())) => AuditDelivery::Recorded,
            Ok(Err(AuditUnavailable)) | Err(_) => AuditDelivery::Unavailable,
        }
    }

    /// Hand one phase's records to the sink, one at a time, inside one budget
    /// for the whole phase. Each record still gets its own attempt, shortened
    /// by whatever the budget has left. Returns how many were not acknowledged,
    /// the ones the budget did not reach included: those are reported as lost
    /// evidence rather than presented as recorded.
    async fn audit_all(&self, records: Vec<AttachmentAuditRecord>) -> usize {
        let limits = self.inner.limits;
        let closes_at = Instant::now() + limits.audit_budget;
        let mut unrecorded = 0_usize;
        for record in records {
            let left = closes_at.saturating_duration_since(Instant::now());
            if left.is_zero() {
                unrecorded += 1;
                continue;
            }
            let attempt = timeout(
                limits.audit_deadline.min(left),
                self.inner.audit.record(record),
            );
            if !matches!(attempt.await, Ok(Ok(()))) {
                unrecorded += 1;
            }
        }
        unrecorded
    }

    /// Remove every ticket whose time has passed and record each expiry. They
    /// are gone whether or not that can be recorded. Returns how many could not.
    async fn sweep_expired(&self, now_ms: u64) -> usize {
        let expired = self.book().expire(now_ms);
        if expired.is_empty() {
            return 0;
        }
        let unrecorded = self
            .audit_all(
                expired
                    .into_iter()
                    .map(|ticket| AttachmentAuditRecord::TicketExpired { ticket })
                    .collect(),
            )
            .await;
        if unrecorded != 0 {
            tracing::error!(
                unrecorded,
                "expired upload tickets were removed without audit evidence"
            );
        }
        unrecorded
    }

    /// Answer `attachment.begin`: either the conversation already has this
    /// file, or here is a single-use ticket to upload it.
    ///
    /// Repeating the same request (caller, action identifier, conversation and
    /// file) does not add a ticket. It replaces the earlier one, which stops
    /// working; only a fingerprint of a secret is kept, so the earlier ticket
    /// could not be handed out again.
    pub async fn begin(
        &self,
        caller: AttachmentCaller,
        request: BeginUpload,
    ) -> Result<BeginOutcome, BeginError> {
        // Tickets whose time has passed leave the book first, on every begin,
        // whoever asks and whether or not the rest of the request is any good:
        // stale tickets must not be what fills the book. An expiry that could
        // not be recorded fails this request visibly.
        let now_ms = self.inner.clock.unix_milliseconds();
        let unrecorded = self.sweep_expired(now_ms).await;
        let upload = describe_upload(&caller, &request)?;
        if unrecorded != 0 {
            return Err(BeginError::Audit);
        }
        match self.owns(&caller, &upload.conversation_id).await? {
            Ownership::Owned => {}
            Ownership::NotFound => return Err(BeginError::ConversationNotFound),
            Ownership::Deleted => return Err(BeginError::ConversationDeleted),
        }
        // An image nothing will ever be prepared from is refused here, before
        // a ticket for it exists. Issuing one would invite bytes to be
        // transferred, normalized and held that no message could ever name.
        if upload.uploaded.media_type().is_image() && !self.inner.normalizer.offers_images() {
            return Err(BeginError::ImagesUnsupported);
        }
        if let Some(hold) = self
            .inner
            .store
            .find_upload(
                &caller.organization_id,
                &upload.conversation_id,
                &upload.uploaded,
            )
            .await
            .map_err(|_| BeginError::Storage)?
        {
            return Ok(BeginOutcome::Stored(hold.stored().clone()));
        }
        self.issue_and_record(caller.organization_id, upload, now_ms)
            .await
    }

    /// Whether this conversation exists and belongs to this caller. Another
    /// owner's conversation and one that does not exist are the same answer.
    async fn owns(
        &self,
        caller: &AttachmentCaller,
        conversation_id: &ConversationId,
    ) -> Result<Ownership, BeginError> {
        self.inner
            .ownership
            .owns(
                &caller.organization_id,
                &caller.principal_id,
                conversation_id,
            )
            .await
            .map_err(|_| BeginError::Unavailable)
    }

    /// Make one single-use ticket, put it in the book, and record it before it
    /// leaves this process. A ticket is permission for a verified caller to
    /// write into a conversation, so both the issuance and any replacement it
    /// caused are attempted; unrecorded, the ticket is taken back, which undoes
    /// nothing anyone could have relied on because nobody has seen its secret.
    async fn issue_and_record(
        &self,
        organization_id: OrganizationId,
        upload: DescribedUpload,
        now_ms: u64,
    ) -> Result<BeginOutcome, BeginError> {
        let secret = TicketSecret::from_bytes(
            self.inner
                .secrets
                .fresh()
                .map_err(|_| BeginError::Unavailable)?,
        );
        let lifetime = TicketLifetime::starting(now_ms).map_err(|_| BeginError::Unavailable)?;
        let ticket = UploadTicket::new(
            organization_id,
            upload.conversation_id,
            upload.uploaded,
            upload.initiator,
            lifetime,
        );
        let issued = self
            .book()
            .issue(secret.fingerprint(), ticket.clone())
            .map_err(|_| BeginError::Capacity)?;
        let mut recorded = true;
        if let Some(replaced) = issued.replaced {
            recorded &= self
                .audit(AttachmentAuditRecord::TicketReplaced { ticket: replaced })
                .await
                == AuditDelivery::Recorded;
        }
        recorded &= self
            .audit(AttachmentAuditRecord::TicketIssued { ticket })
            .await
            == AuditDelivery::Recorded;
        if !recorded {
            self.book().withdraw_unissued(&secret.fingerprint());
            return Err(BeginError::Audit);
        }
        Ok(BeginOutcome::UploadRequired {
            ticket: secret,
            expires_at_ms: lifetime.expires_at_ms(),
        })
    }

    /// Receive one upload under `presented`. On success the result is the file
    /// the conversation now keeps, which is what a message must refer to: for
    /// an image it is the normalized image, not the bytes that were sent.
    ///
    /// The ticket is used up the moment it is found, whatever happens next.
    /// The work runs in its own task, so a caller that goes away cannot take
    /// the evidence of a used ticket with it.
    pub async fn receive(
        &self,
        presented: &str,
        declared_length: Option<u64>,
        body: Box<dyn UploadBody>,
    ) -> Result<Attachment, UploadError> {
        let secret = TicketSecret::parse(presented).ok_or(UploadError::TicketInvalid)?;
        let permit = self
            .inner
            .uploads
            .clone()
            .try_acquire_owned()
            .map_err(|_| UploadError::Busy)?;
        // The ticket leaves the book here rather than inside the task, so that
        // whatever becomes of that task this gateway still knows which ticket
        // was used up and can say so.
        let now_ms = self.inner.clock.unix_milliseconds();
        let redemption = self.book().redeem(&secret.fingerprint(), now_ms);
        let used = match &redemption {
            Redemption::Usable(ticket) | Redemption::Expired(ticket) => Some(ticket.clone()),
            Redemption::Unknown => None,
        };
        let pending: Arc<PendingHold> = Arc::default();
        let service = self.clone();
        let written = pending.clone();
        let work = tokio::spawn(async move {
            let _permit = permit;
            service
                .redeem(redemption, now_ms, declared_length, body, &written)
                .await
        });
        match work.await {
            Ok(outcome) => outcome,
            Err(_) => self.unresolved(used, &pending).await,
        }
    }

    /// The work of one upload stopped without an answer. What it had already
    /// done is still this gateway's to account for: a hold it left pending is
    /// taken back by its own claim and no other, and the ticket it used up is
    /// recorded as used without a hold, for the reason that actually applies.
    async fn unresolved(
        &self,
        used: Option<UploadTicket>,
        pending: &PendingHold,
    ) -> Result<Attachment, UploadError> {
        if let Some((hold, claim)) = written_hold(pending) {
            self.take_back(&hold, &claim, RevertCause::UploadUnresolved)
                .await;
        }
        match used {
            Some(ticket) => {
                tracing::error!(
                    conversation_id = %ticket.conversation_id(),
                    "an upload's own work stopped without an answer"
                );
                Err(self.reject(ticket, UploadRejection::Unresolved).await)
            }
            // Nothing left the book, so nothing was used up and there is nobody
            // to attribute anything to: the presented secret matched no ticket.
            None => Err(UploadError::TicketInvalid),
        }
    }

    async fn redeem(
        &self,
        redemption: Redemption,
        now_ms: u64,
        declared_length: Option<u64>,
        body: Box<dyn UploadBody>,
        pending: &PendingHold,
    ) -> Result<Attachment, UploadError> {
        // An expired ticket is refused and recorded as itself, before the
        // sweep that would otherwise account for it as nobody's doing.
        let expired = match &redemption {
            Redemption::Expired(ticket) => Some(
                self.audit(AttachmentAuditRecord::TicketExpired {
                    ticket: ticket.clone(),
                })
                .await,
            ),
            Redemption::Usable(_) | Redemption::Unknown => None,
        };
        // Every upload also clears out the tickets nobody came back for, so
        // stale tickets cannot fill the book while nobody begins anything. A
        // sweep that could not be recorded is logged, and does not fail an
        // upload that had nothing to do with those tickets.
        self.sweep_expired(now_ms).await;
        let ticket = match (redemption, expired) {
            (Redemption::Usable(ticket), _) => ticket,
            (_, Some(evidence)) => return Err(UploadError::TicketExpired { evidence }),
            (_, None) => return Err(UploadError::TicketInvalid),
        };
        let (staged, hold) = match self.transfer(&ticket, declared_length, body).await {
            Ok(kept) => kept,
            Err(reason) => return Err(self.reject(ticket, reason).await),
        };
        match staged.keep(hold.clone()).await {
            Err(StoreUnavailable) => Err(self
                .reject(ticket, UploadRejection::StorageUnavailable)
                .await),
            // The conversation keeps this file already. Nothing was written, so
            // the record says the file was already held rather than claiming a
            // hold was created.
            Ok(Kept::Existing(existing)) => {
                let stored = existing.stored().clone();
                let record = AttachmentAuditRecord::AlreadyHeld {
                    ticket,
                    hold: existing,
                };
                match self.audit(record).await {
                    AuditDelivery::Recorded => Ok(stored),
                    // The hold that was already there is untouched: nothing was
                    // written and nothing was taken back. Only this arrival at
                    // it is missing, which is enough to refuse the upload.
                    AuditDelivery::Unavailable => Err(UploadError::AuditUnavailable),
                }
            }
            Ok(Kept::Pending(claim)) => self.record_then_confirm(hold, claim, pending).await,
        }
    }

    /// The one owner of a hold's creation. The hold is written pending, which
    /// nothing can use; its creation is recorded; only then is it made usable.
    /// Whatever goes wrong, this upload takes back its own pending hold and no
    /// other, and a creation that may have reached the trail is followed by a
    /// record saying the hold did not last.
    ///
    /// The hold is left in `pending` for as long as this upload owns it, so
    /// that work stopping here without an answer does not strand it.
    async fn record_then_confirm(
        &self,
        hold: Hold,
        claim: HoldClaim,
        pending: &PendingHold,
    ) -> Result<Attachment, UploadError> {
        *lock(pending) = Some((hold.clone(), claim.clone()));
        let settled = self.settle(hold, claim).await;
        *lock(pending) = None;
        settled
    }

    async fn settle(&self, hold: Hold, claim: HoldClaim) -> Result<Attachment, UploadError> {
        let created = AttachmentAuditRecord::HoldCreated { hold: hold.clone() };
        if self.audit(created).await == AuditDelivery::Unavailable {
            self.take_back(&hold, &claim, RevertCause::AuditUnconfirmed)
                .await;
            return Err(UploadError::AuditUnavailable);
        }
        // Asked again now that the hold is written. A delete writes its
        // tombstone before it lets go of the conversation's holds, so either
        // this sees the tombstone, or the hold was written before the release
        // that removes it: an upload still transferring when its conversation
        // was deleted cannot leave a hold under it
        // (`an_upload_finishing_after_its_conversation_was_deleted_keeps_nothing`).
        match self
            .inner
            .ownership
            .owns(
                hold.organization_id(),
                hold.uploaded_by().principal_id(),
                hold.conversation_id(),
            )
            .await
        {
            Ok(Ownership::Owned) => {}
            Ok(Ownership::Deleted) => {
                self.take_back(&hold, &claim, RevertCause::ConversationDeleted)
                    .await;
                return Err(UploadError::NotKept);
            }
            // Not a deletion: nothing says one happened, so the record says
            // what was found instead (`an_upload_whose_conversation_is_no_longer_found_is_reverted_as_not_found`).
            Ok(Ownership::NotFound) => {
                self.take_back(&hold, &claim, RevertCause::ConversationNotFound)
                    .await;
                return Err(UploadError::NotKept);
            }
            Err(OwnershipUnavailable) => return Err(self.not_confirmed(&hold, &claim).await),
        }
        match self.inner.store.confirm(&hold, &claim).await {
            Ok(Confirmation::Confirmed | Confirmation::AlreadyKept) => Ok(hold.stored().clone()),
            Ok(Confirmation::Gone) => {
                // Its conversation let go of its files meanwhile, or an upload
                // of the same file that had taken this hold over took it back.
                // Either way the creation on record did not last.
                let reverted = AttachmentAuditRecord::HoldReverted {
                    hold,
                    cause: RevertCause::RemovedBeforeUsable,
                };
                if self.audit(reverted).await == AuditDelivery::Unavailable {
                    tracing::error!("a hold removed before it was usable went unrecorded");
                }
                Err(UploadError::NotKept)
            }
            Err(StoreUnavailable) => Err(self.not_confirmed(&hold, &claim).await),
        }
    }

    /// The hold could not be made usable, or whether its conversation may
    /// still keep it could not be asked: take it back, and say how far that
    /// got.
    async fn not_confirmed(&self, hold: &Hold, claim: &HoldClaim) -> UploadError {
        match self
            .take_back(hold, claim, RevertCause::ConfirmationFailed)
            .await
        {
            TakenBack::Removed => UploadError::Rejected {
                reason: UploadRejection::StorageUnavailable,
                evidence: AuditDelivery::Recorded,
            },
            TakenBack::RemovedUnrecorded | TakenBack::Stranded => UploadError::Rejected {
                reason: UploadRejection::StorageUnavailable,
                evidence: AuditDelivery::Unavailable,
            },
            // The hold is not this upload's any more, so this upload
            // neither kept it nor has anything of its own to undo.
            // Whoever owns it now records that for itself.
            TakenBack::NothingLeft => UploadError::NotKept,
        }
    }

    /// Take back this claim's pending hold and say what became of it. Each
    /// answer is a different thing to say about the hold, and none of them is
    /// evidence that some other upload's record reached the sink.
    async fn take_back(&self, hold: &Hold, claim: &HoldClaim, cause: RevertCause) -> TakenBack {
        match self.inner.store.discard(hold, claim).await {
            Ok(Discard::Discarded) => {
                // Attempted with its own bound even when the sink just failed:
                // that failure may have been a late success.
                let reverted = AttachmentAuditRecord::HoldReverted {
                    hold: hold.clone(),
                    cause,
                };
                if self.audit(reverted).await == AuditDelivery::Recorded {
                    TakenBack::Removed
                } else {
                    tracing::error!(
                        stored = %hold.stored().digest(),
                        "a hold was taken back without audit evidence"
                    );
                    TakenBack::RemovedUnrecorded
                }
            }
            // Another upload owns the hold now, or a release removed it; each
            // of those records itself. Nothing of this claim is left to undo,
            // and this upload wrote nothing that is missing from the trail.
            Ok(Discard::NotMine) => TakenBack::NothingLeft,
            Err(StoreUnavailable) => {
                tracing::error!(
                    stored = %hold.stored().digest(),
                    "a pending hold could not be taken back"
                );
                TakenBack::Stranded
            }
        }
    }

    async fn reject(&self, ticket: UploadTicket, reason: UploadRejection) -> UploadError {
        let evidence = self
            .audit(AttachmentAuditRecord::UploadRejected { ticket, reason })
            .await;
        UploadError::Rejected { reason, evidence }
    }

    /// Bring the body to durable temporary storage, verify it is the file the
    /// ticket described, and produce what will be kept: the same bytes, or for
    /// an image its normalized form staged under its own digest.
    async fn transfer(
        &self,
        ticket: &UploadTicket,
        declared_length: Option<u64>,
        mut body: Box<dyn UploadBody>,
    ) -> Result<(Box<dyn StagedUpload>, Hold), UploadRejection> {
        let uploaded = ticket.attachment();
        if declared_length.is_some_and(|length| length != uploaded.size()) {
            return Err(UploadRejection::SizeMismatch);
        }
        let store = &self.inner.store;
        let unavailable = |_| UploadRejection::StorageUnavailable;
        let mut staged = store.stage().await.map_err(unavailable)?;
        // The deadline covers the transfer, which a caller can stall. What
        // follows is local work the gateway itself bounds.
        let received = timeout(
            self.inner.limits.upload_deadline,
            Self::stream(ticket, staged.as_mut(), body.as_mut()),
        )
        .await
        .map_err(|_| UploadRejection::UploadTimeout)??;
        drop(body);
        ticket
            .check_received(received.size, received.digest)
            .map_err(|mismatch| match mismatch {
                UploadMismatch::Size => UploadRejection::SizeMismatch,
                UploadMismatch::Digest => UploadRejection::DigestMismatch,
            })?;
        let (staged, stored) = if uploaded.media_type().is_image() {
            self.fit_image(staged).await?
        } else {
            (staged, uploaded.clone())
        };
        // The normalizer is outside code. What it answered is kept only if it
        // is an image a message can name: the hold's own rule decides, so a
        // result of another type, or over a message's per-image size, is a
        // failed normalization and not a file nothing could ever refer to.
        let hold = Hold::from_upload(ticket, stored, self.inner.clock.unix_milliseconds())
            .ok_or(UploadRejection::NormalizationFailed)?;
        Ok((staged, hold))
    }

    /// Bring a verified image to the form the conversation will keep, staged
    /// under its own digest.
    ///
    /// Normalizing needs the whole image at once, and its pixels. It is read
    /// only here, after the bytes proved to be the file the ticket described,
    /// and only as many are in memory together as the permit allows; the rest
    /// wait where they are, already safe on disk.
    async fn fit_image(
        &self,
        mut staged: Box<dyn StagedUpload>,
    ) -> Result<(Box<dyn StagedUpload>, Attachment), UploadRejection> {
        let unavailable = |_| UploadRejection::StorageUnavailable;
        let _normalizing = self
            .inner
            .normalizations
            .acquire()
            .await
            .map_err(|_| UploadRejection::NormalizationFailed)?;
        let original = staged.read().await.map_err(unavailable)?;
        drop(staged);
        let normalized = self
            .inner
            .normalizer
            .normalize(original)
            .await
            .map_err(|error| match error {
                NormalizeError::NotOffered => UploadRejection::ImageInputUnsupported,
                NormalizeError::Unsupported => UploadRejection::UnsupportedImage,
                NormalizeError::TooLarge => UploadRejection::ImageTooLarge,
                NormalizeError::Failed => UploadRejection::NormalizationFailed,
            })?;
        let media_type = MediaType::parse(&normalized.media_type)
            .map_err(|_| UploadRejection::NormalizationFailed)?;
        let mut kept = self.inner.store.stage().await.map_err(unavailable)?;
        kept.write(normalized.bytes).await.map_err(unavailable)?;
        let written = kept.finish().await.map_err(unavailable)?;
        let stored = Attachment::new(written.digest, media_type, written.size)
            .map_err(|_| UploadRejection::NormalizationFailed)?;
        Ok((kept, stored))
    }

    async fn stream(
        ticket: &UploadTicket,
        staged: &mut dyn StagedUpload,
        body: &mut dyn UploadBody,
    ) -> Result<ReceivedBytes, UploadRejection> {
        let mut received = 0_u64;
        while let Some(chunk) = body
            .next()
            .await
            .map_err(|_| UploadRejection::UploadInterrupted)?
        {
            received = received.saturating_add(chunk.len() as u64);
            if ticket.is_exceeded_by(received) {
                return Err(UploadRejection::SizeMismatch);
            }
            staged
                .write(chunk)
                .await
                .map_err(|_| UploadRejection::StorageUnavailable)?;
        }
        staged
            .finish()
            .await
            .map_err(|_| UploadRejection::StorageUnavailable)
    }

    /// Whether the conversation keeps exactly this stored file.
    pub async fn holds(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
        stored: &Attachment,
    ) -> Result<bool, StoreUnavailable> {
        self.inner
            .store
            .holds(organization_id, conversation_id, stored)
            .await
    }

    /// Let go of everything one conversation holds, and of the uploads it had
    /// been permitted and not begun. Every ticket and hold is tried and every
    /// transition that happened is recorded; what could not be completed is
    /// reported afterwards, and never stops the rest.
    ///
    /// Who is letting go is settled before anything is touched. A caller that
    /// cannot be written down changes nothing here.
    pub async fn release(&self, request: ReleaseRequest) -> Result<(), ReleaseError> {
        let caller = Caller::new(
            request.principal_id,
            &request.surface_id,
            &request.correlation_id,
        )
        .map_err(|_| ReleaseError::Unattributable)?;
        let release = ReleaseEvidence {
            cause: request.cause,
            caller,
            requested_at_ms: self.inner.clock.unix_milliseconds(),
        };
        // Tickets go first, so an upload that has not begun cannot begin after
        // the holds are gone.
        let withdrawn = self
            .book()
            .withdraw(&request.organization_id, &request.conversation_id);
        // Without a report there is nothing to record about holds. The
        // withdrawn tickets are still gone, and still accounted for below.
        let report = self
            .inner
            .store
            .release(&request.organization_id, &request.conversation_id)
            .await
            .ok();
        let storage_failures = report.as_ref().map_or(1, |report| report.failures);
        let mut records: Vec<_> = withdrawn
            .into_iter()
            .map(|ticket| AttachmentAuditRecord::TicketWithdrawn {
                ticket,
                release: release.clone(),
            })
            .collect();
        if let Some(report) = report {
            records.extend(report.released.into_iter().map(|released| {
                AttachmentAuditRecord::HoldReleased {
                    hold: released.hold,
                    was: released.was,
                    release: release.clone(),
                }
            }));
            records.extend(report.removed.into_iter().map(|hold| {
                AttachmentAuditRecord::BlobRemoved {
                    hold,
                    release: release.clone(),
                }
            }));
        }
        // One budget for the whole release: a conversation's holds are not
        // bounded, and a sink that answers slowly must not keep a close open
        // for one deadline per hold. Cleanup is already done by this point.
        let audit_failures = self.audit_all(records).await;
        if storage_failures == 0 && audit_failures == 0 {
            return Ok(());
        }
        tracing::error!(
            conversation_id = %request.conversation_id,
            storage_failures,
            audit_failures,
            "releasing a conversation's attachments did not complete"
        );
        Err(ReleaseError::Incomplete {
            storage_failures,
            audit_failures,
        })
    }
}

/// A hold one upload has written and has not finished with. Its own work
/// clears it; whoever was waiting on that work reads it when the work stops
/// without an answer, and takes the hold back by this claim and no other.
type PendingHold = Mutex<Option<(Hold, HoldClaim)>>;

/// A pending hold is a fact about the store, not about the lock that guards
/// it, so a panic elsewhere never makes it unreadable.
fn lock(pending: &PendingHold) -> MutexGuard<'_, Option<(Hold, HoldClaim)>> {
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn written_hold(pending: &PendingHold) -> Option<(Hold, HoldClaim)> {
    lock(pending).take()
}

/// What became of a pending hold its own upload tried to take back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TakenBack {
    /// Removed by this upload, and the removal is on record.
    Removed,
    /// Removed by this upload; the removal itself was not acknowledged.
    RemovedUnrecorded,
    /// Nothing of this claim was left: another upload of the same file owns
    /// the hold now, or a release removed it. Each of those records itself,
    /// and this upload wrote nothing that is missing from the trail.
    NothingLeft,
    /// The hold could not be removed. It stays pending, which nothing can use,
    /// and goes when its conversation lets go of its files.
    Stranded,
}

/// One `attachment.begin` as this context understands it: a conversation, the
/// file, and the verified caller every later record of it will name.
struct DescribedUpload {
    conversation_id: ConversationId,
    uploaded: Attachment,
    initiator: Caller,
}

/// Read one submitted request, before anything is looked up or changed.
fn describe_upload(
    caller: &AttachmentCaller,
    request: &BeginUpload,
) -> Result<DescribedUpload, BeginError> {
    Ok(DescribedUpload {
        conversation_id: ConversationId::new(&request.conversation_id)
            .map_err(|_| BeginError::InvalidRequest)?,
        uploaded: Attachment::new(
            Sha256Digest::parse(&request.digest).map_err(|_| BeginError::InvalidRequest)?,
            MediaType::parse(&request.media_type).map_err(|_| BeginError::InvalidRequest)?,
            request.size,
        )
        .map_err(|_| BeginError::InvalidRequest)?,
        initiator: Caller::new(
            caller.principal_id.clone(),
            &caller.surface_id,
            &request.request_id,
        )
        .map_err(|_| BeginError::InvalidRequest)?,
    })
}
