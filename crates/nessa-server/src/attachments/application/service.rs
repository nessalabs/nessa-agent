use super::{
    AttachmentAudit, AttachmentAuditRecord, AttachmentStore, AuditDelivery, AuditUnavailable,
    BeginError, BlobOutcome, ConversationOwnership, ImageNormalizer, NormalizeError, ReceivedBytes,
    ReleaseCause, ReleaseError, ReleaseEvidence, StagedUpload, TicketSecret, TicketSecrets,
    UploadBody, UploadError, UploadRejection,
};
use crate::{
    attachments::domain::{
        Attachment, Caller, Hold, MediaType, Redemption, TicketBook, TicketLifetime,
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
    /// Most tickets outstanding at once, across every caller.
    pub max_tickets: usize,
    /// Most uploads in progress at once. Normalizing an image holds the whole
    /// upload in memory, so this also bounds that memory.
    pub max_uploads: usize,
    /// How long one transfer may take from its first byte to its last.
    pub upload_deadline: Duration,
    /// How long one audit record may take to be acknowledged. Every record
    /// gets this much; one slow record does not spend another's time.
    pub audit_deadline: Duration,
}
impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            max_tickets: 64,
            max_uploads: 4,
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
                book: Mutex::new(TicketBook::new(limits.max_tickets)),
                uploads: Arc::new(Semaphore::new(limits.max_uploads)),
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

    /// Answer `attachment.begin`: either the conversation already has this
    /// file, or here is a single-use ticket to upload it.
    pub async fn begin(
        &self,
        caller: AttachmentCaller,
        request: BeginUpload,
    ) -> Result<BeginOutcome, BeginError> {
        let conversation_id = ConversationId::new(&request.conversation_id)
            .map_err(|_| BeginError::InvalidRequest)?;
        let uploaded = Attachment::new(
            Sha256Digest::parse(&request.digest).map_err(|_| BeginError::InvalidRequest)?,
            MediaType::parse(&request.media_type).map_err(|_| BeginError::InvalidRequest)?,
            request.size,
        )
        .map_err(|_| BeginError::InvalidRequest)?;
        let initiator = Caller::new(
            caller.principal_id.clone(),
            &caller.surface_id,
            &request.request_id,
        )
        .map_err(|_| BeginError::InvalidRequest)?;
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
        // Tickets whose time has passed leave the book here, before capacity is
        // judged. They are gone whether or not their expiry can be recorded;
        // an expiry that could not be recorded fails this request visibly.
        let now_ms = self.inner.clock.unix_milliseconds();
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
            return Err(BeginError::Audit);
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
        self.book()
            .issue(secret.fingerprint(), ticket)
            .map_err(|_| BeginError::Capacity)?;
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
        let redemption = self.book().redeem(&secret.fingerprint(), now_ms);
        let ticket = match redemption {
            Redemption::Unknown => return Err(UploadError::TicketInvalid),
            Redemption::Expired(ticket) => {
                let evidence = self
                    .audit(AttachmentAuditRecord::TicketExpired { ticket })
                    .await;
                return Err(UploadError::TicketExpired { evidence });
            }
            Redemption::Usable(ticket) => ticket,
        };
        let (staged, hold) = match self.transfer(&ticket, declared_length, body).await {
            Ok(kept) => kept,
            Err(reason) => return Err(self.reject(ticket, reason).await),
        };
        let change = match staged.keep(hold.clone()).await {
            Ok(change) => change,
            Err(_) => {
                return Err(self
                    .reject(ticket, UploadRejection::StorageUnavailable)
                    .await)
            }
        };
        let record = AttachmentAuditRecord::HoldCreated {
            hold: hold.clone(),
            before: change.before(),
        };
        if self.audit(record).await == AuditDelivery::Unavailable {
            // A hold nobody can account for must not stay usable. Taking it back
            // is cleanup, so it happens even though the sink is not answering.
            let stored = hold.stored().digest();
            let reverted = self.inner.store.revert(hold, change).await.is_ok();
            if !reverted {
                tracing::error!(%stored, "an unaudited hold could not be taken back");
            }
            return Err(UploadError::AuditUnavailable { reverted });
        }
        Ok(hold.stored().clone())
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
        .map_err(|_| UploadRejection::DeadlineElapsed)??;
        drop(body);
        ticket
            .check_received(received.size, received.digest)
            .map_err(|mismatch| match mismatch {
                UploadMismatch::Size => UploadRejection::SizeMismatch,
                UploadMismatch::Digest => UploadRejection::DigestMismatch,
            })?;
        let (staged, stored) = if uploaded.media_type().is_image() {
            // Normalizing needs the whole image at once. It is read only now,
            // after it proved to be the file the ticket described, and the
            // upload permit bounds how many of these exist together.
            let original = staged.read().await.map_err(unavailable)?;
            drop(staged);
            let normalized = self
                .inner
                .normalizer
                .normalize(original, uploaded.media_type().as_str())
                .await
                .map_err(|error| match error {
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
            .map_err(|_| UploadRejection::BodyInterrupted)?
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
    ) -> Result<bool, super::StoreUnavailable> {
        self.inner
            .store
            .holds(organization_id, conversation_id, stored)
            .await
    }

    /// Let go of everything one conversation holds, and of what it could still
    /// have added. Every ticket and hold is tried and every transition that
    /// happened is recorded; what could not be completed
    /// is reported afterwards, and never stops the rest.
    pub async fn release(&self, request: ReleaseRequest) -> Result<(), ReleaseError> {
        let requested_at_ms = self.inner.clock.unix_milliseconds();
        // Tickets go first, so no upload can begin after the holds are gone and
        // leave a hold behind in a conversation that has closed. An upload
        // already past its ticket is not stopped; see the module's limits.
        let voided = self
            .book()
            .void(&request.organization_id, &request.conversation_id);
        // Without a report there is nothing to record about holds. The voided
        // tickets are still gone, and still accounted for below.
        let report = self
            .inner
            .store
            .release(&request.organization_id, &request.conversation_id)
            .await
            .ok();
        // A caller that cannot be written down cannot be given as the
        // initiator. The files are already released; what is lost is evidence,
        // and it is reported as lost.
        let release = Caller::new(
            request.principal_id,
            &request.surface_id,
            &request.correlation_id,
        )
        .ok()
        .map(|caller| ReleaseEvidence {
            cause: request.cause,
            caller,
            requested_at_ms,
        });
        let mut audit_failures = 0_usize;
        for ticket in voided {
            let delivered = match &release {
                Some(release) => {
                    self.audit(AttachmentAuditRecord::TicketVoided {
                        ticket,
                        release: release.clone(),
                    })
                    .await
                }
                None => AuditDelivery::Unavailable,
            };
            if delivered == AuditDelivery::Unavailable {
                audit_failures += 1;
            }
        }
        let mut storage_failures = report.as_ref().map_or(1, |report| report.failures);
        for released in report.map(|report| report.released).unwrap_or_default() {
            let mut records = Vec::with_capacity(2);
            if let Some(release) = &release {
                records.push(AttachmentAuditRecord::HoldReleased {
                    hold: released.hold.clone(),
                    release: release.clone(),
                });
            } else {
                audit_failures += 1;
            }
            match released.blob {
                BlobOutcome::StillHeld | BlobOutcome::Missing => {}
                BlobOutcome::RemovalFailed => storage_failures += 1,
                BlobOutcome::Removed => match &release {
                    Some(release) => records.push(AttachmentAuditRecord::BlobRemoved {
                        hold: released.hold,
                        release: release.clone(),
                    }),
                    None => audit_failures += 1,
                },
            }
            for record in records {
                if self.audit(record).await == AuditDelivery::Unavailable {
                    audit_failures += 1;
                }
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
        Err(ReleaseError {
            storage_failures,
            audit_failures,
        })
    }
}
