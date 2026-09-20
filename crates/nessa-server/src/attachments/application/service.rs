use super::{
    AttachmentAudit, AttachmentAuditRecord, AttachmentStore, AuditDelivery, AuditUnavailable,
    BeginError, Confirmation, ConversationOwnership, Discard, HoldClaim, ImageNormalizer, Kept,
    NormalizeError, ReceivedBytes, ReleaseCause, ReleaseError, ReleaseEvidence, RevertCause,
    StagedUpload, StoreUnavailable, TicketSecret, TicketSecrets, UploadBody, UploadError,
    UploadRejection,
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
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{sync::Semaphore, time::timeout};

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

    /// Remove every ticket whose time has passed and record each expiry. They
    /// are gone whether or not that can be recorded. Returns how many could not.
    async fn sweep(&self, now_ms: u64) -> usize {
        let expired = self.book().expire(now_ms);
        let mut unrecorded = 0_usize;
        for ticket in expired {
            if self
                .audit(AttachmentAuditRecord::TicketExpired { ticket })
                .await
                == AuditDelivery::Unavailable
            {
                unrecorded += 1;
            }
        }
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
        let unrecorded = self.sweep(now_ms).await;
        let conversation_id = ConversationId::new(&request.conversation_id)
            .map_err(|_| BeginError::InvalidRequest)?;
        let uploaded = Attachment::new(
            Sha256Digest::parse(&request.digest).map_err(|_| BeginError::InvalidRequest)?,
            MediaType::parse(&request.media_type).map_err(|_| BeginError::InvalidRequest)?,
            request.size,
        )
        .map_err(|_| BeginError::InvalidRequest)?;
        // Who is asking is settled here, before anything is read or changed,
        // so every later record of this ticket has an initiator to name.
        let initiator = Caller::new(
            caller.principal_id.clone(),
            &caller.surface_id,
            &request.request_id,
        )
        .map_err(|_| BeginError::InvalidRequest)?;
        if unrecorded != 0 {
            return Err(BeginError::Audit);
        }
        let owns = self
            .inner
            .ownership
            .owns(
                &caller.organization_id,
                &caller.principal_id,
                &conversation_id,
            )
            .await
            .map_err(|_| BeginError::Unavailable)?;
        if !owns {
            return Err(BeginError::ConversationNotFound);
        }
        if let Some(hold) = self
            .inner
            .store
            .find_upload(&caller.organization_id, &conversation_id, &uploaded)
            .await
            .map_err(|_| BeginError::Storage)?
        {
            return Ok(BeginOutcome::Stored(hold.stored().clone()));
        }
        let secret = TicketSecret::from_bytes(
            self.inner
                .secrets
                .fresh()
                .map_err(|_| BeginError::Unavailable)?,
        );
        let lifetime = TicketLifetime::starting(now_ms).map_err(|_| BeginError::Unavailable)?;
        let ticket = UploadTicket::new(
            caller.organization_id,
            conversation_id,
            uploaded,
            initiator,
            lifetime,
        );
        let issued = self
            .book()
            .issue(secret.fingerprint(), ticket.clone())
            .map_err(|_| BeginError::Capacity)?;
        // A ticket is permission for a verified caller to write into a
        // conversation, so issuing one is recorded, and recorded before the
        // ticket leaves this process. Both records are attempted.
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
            // Nobody has seen this ticket's secret, so taking it back undoes
            // nothing anyone could have relied on.
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
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            service.redeem(secret, declared_length, body).await
        })
        .await
        .unwrap_or(Err(UploadError::Rejected {
            reason: UploadRejection::StorageUnavailable,
            evidence: AuditDelivery::Unavailable,
        }))
    }

    async fn redeem(
        &self,
        secret: TicketSecret,
        declared_length: Option<u64>,
        body: Box<dyn UploadBody>,
    ) -> Result<Attachment, UploadError> {
        let now_ms = self.inner.clock.unix_milliseconds();
        // The presented ticket is looked at first so that, expired, it is
        // refused and recorded as itself.
        let redemption = self.book().redeem(&secret.fingerprint(), now_ms);
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
        self.sweep(now_ms).await;
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
            // The conversation keeps this file already. Nothing changed, and the
            // record says so rather than claiming a hold was created.
            Ok(Kept::Existing(existing)) => {
                let stored = existing.stored().clone();
                let record = AttachmentAuditRecord::AlreadyHeld {
                    ticket,
                    hold: existing,
                };
                match self.audit(record).await {
                    AuditDelivery::Recorded => Ok(stored),
                    AuditDelivery::Unavailable => {
                        Err(UploadError::AuditUnavailable { reverted: true })
                    }
                }
            }
            Ok(Kept::Pending(claim)) => self.record_then_confirm(hold, claim).await,
        }
    }

    /// The one owner of a hold's creation. The hold is written pending, which
    /// nothing can use; its creation is recorded; only then is it made usable.
    /// Whatever goes wrong, this upload takes back its own pending hold and no
    /// other, and a creation that may have reached the trail is followed by a
    /// record saying the hold did not last.
    async fn record_then_confirm(
        &self,
        hold: Hold,
        claim: HoldClaim,
    ) -> Result<Attachment, UploadError> {
        let created = AttachmentAuditRecord::HoldCreated { hold: hold.clone() };
        if self.audit(created).await == AuditDelivery::Unavailable {
            let reverted = self
                .take_back(&hold, &claim, RevertCause::AuditUnconfirmed)
                .await
                .is_some();
            return Err(UploadError::AuditUnavailable { reverted });
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
            Err(StoreUnavailable) => {
                let evidence = self
                    .take_back(&hold, &claim, RevertCause::ConfirmationFailed)
                    .await
                    .unwrap_or(AuditDelivery::Unavailable);
                Err(UploadError::Rejected {
                    reason: UploadRejection::StorageUnavailable,
                    evidence,
                })
            }
        }
    }

    /// Take back this claim's pending hold and say so. `None` when it could
    /// not be taken back; the hold then stays pending, which nothing can use.
    async fn take_back(
        &self,
        hold: &Hold,
        claim: &HoldClaim,
        cause: RevertCause,
    ) -> Option<AuditDelivery> {
        match self.inner.store.discard(hold, claim).await {
            Ok(Discard::Discarded) => {
                // Attempted with its own bound even when the sink just failed:
                // that failure may have been a late success.
                let reverted = AttachmentAuditRecord::HoldReverted {
                    hold: hold.clone(),
                    cause,
                };
                let delivery = self.audit(reverted).await;
                if delivery == AuditDelivery::Unavailable {
                    tracing::error!(
                        stored = %hold.stored().digest(),
                        "a hold was taken back without audit evidence"
                    );
                }
                Some(delivery)
            }
            // Another upload owns the hold now, or a release removed it; each
            // of those records itself. Nothing of this claim is left to undo.
            Ok(Discard::NotMine) => Some(AuditDelivery::Recorded),
            Err(StoreUnavailable) => {
                tracing::error!(
                    stored = %hold.stored().digest(),
                    "a pending hold could not be taken back"
                );
                None
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
            // Normalizing needs the whole image at once, and its pixels. It is
            // read only now, after it proved to be the file the ticket
            // described, and only as many are in memory together as this
            // permit allows; the rest wait here, already safe on disk.
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
                .normalize(original, uploaded.media_type().as_str())
                .await
                .map_err(|error| match error {
                    NormalizeError::NotOffered => UploadRejection::ImageInputUnsupported,
                    NormalizeError::Unsupported => UploadRejection::UnsupportedImage,
                    NormalizeError::TooLarge => UploadRejection::ImageTooLarge,
                    NormalizeError::Failed => UploadRejection::NormalizationFailed,
                })?;
            let media_type = MediaType::parse(&normalized.media_type)
                .map_err(|_| UploadRejection::NormalizationFailed)?;
            let mut kept = store.stage().await.map_err(unavailable)?;
            kept.write(normalized.bytes).await.map_err(unavailable)?;
            let written = kept.finish().await.map_err(unavailable)?;
            let stored = Attachment::new(written.digest, media_type, written.size)
                .map_err(|_| UploadRejection::NormalizationFailed)?;
            (kept, stored)
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
        let mut audit_failures = 0_usize;
        for record in records {
            if self.audit(record).await == AuditDelivery::Unavailable {
                audit_failures += 1;
            }
        }
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
