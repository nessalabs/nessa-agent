//! One unpaired review selection: checked metadata, fresh recipient facts,
//! synchronous Cedar decision, then independently owned durable acknowledgement.
//! Only this transition can construct an `AuditedReviewSelection`. Product owns
//! whole-candidate source/revision coherence and admission to its UI lifetime.
use super::{
    AccessAction, AccessAsk, AccessDecision, AccessResource, AccessSubject,
    ConversationAccessPolicyEvaluator, ConversationMetadataReader, RequestShape,
    ReviewNoticeAuditCause, ReviewNoticeAuditFailure, ReviewNoticeAuditRecord, ReviewNoticeAuditor,
};
use nessa_auth::application::{
    ports::{AccessReader, Clock},
    session::{AuthenticatedSession, ReadCurrentSession},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::{executions::ExecutionId, permissions::PermissionId};
use std::sync::Arc;

/// A full policy selection whose exact immutable record was acknowledged.
/// Its constructor stays private to the application transition, so product
/// cannot substitute a boolean for audit delivery or manufacture a receipt.
#[must_use]
pub(crate) struct AuditedReviewSelection {
    record: Arc<ReviewNoticeAuditRecord>,
}

impl AuditedReviewSelection {
    pub(crate) fn decision(&self) -> &AccessDecision {
        self.record.decision()
    }
    pub(crate) fn authority_revision(&self) -> u64 {
        self.record.authority_revision()
    }
}

/// Narrow borrowed dependencies for the complete per-review transition.
/// Constructing it has no effects and creates no independent worker.
pub(crate) struct SelectReviewNotice<'a> {
    metadata: &'a ConversationMetadataReader,
    access: &'a dyn AccessReader,
    clock: &'a dyn Clock,
    policy: &'a dyn ConversationAccessPolicyEvaluator,
    auditor: &'a ReviewNoticeAuditor,
}

impl<'a> SelectReviewNotice<'a> {
    pub(crate) fn new(
        metadata: &'a ConversationMetadataReader,
        access: &'a dyn AccessReader,
        clock: &'a dyn Clock,
        policy: &'a dyn ConversationAccessPolicyEvaluator,
        auditor: &'a ReviewNoticeAuditor,
    ) -> Self {
        Self {
            metadata,
            access,
            clock,
            policy,
            auditor,
        }
    }

    /// Missing/mismatched metadata or unverifiable current membership produces
    /// no policy answer. Every actual Cedar answer, including deny/Unavailable,
    /// is acknowledged before its sealed selection is returned.
    pub(crate) async fn execute(
        &self,
        session: &AuthenticatedSession,
        conversation_id: &ConversationId,
        execution_id: ExecutionId,
        permission_id: PermissionId,
        cause: ReviewNoticeAuditCause,
    ) -> Result<Option<AuditedReviewSelection>, ReviewNoticeAuditFailure> {
        let admitted = self.auditor.admit().await?;
        let Ok(Some(conversation)) = self.metadata.load(conversation_id).await else {
            return Ok(None);
        };
        let Ok(current) = (ReadCurrentSession {
            access: self.access,
            clock: self.clock,
        })
        .execute(session)
        .await
        else {
            return Ok(None);
        };
        let recipient = session.context();
        let decision = self.policy.evaluate(&AccessAsk {
            subject: AccessSubject::ReviewRecipient {
                organization: recipient.organization_id(),
                principal: recipient.principal_id(),
                membership: current.membership.role(),
            },
            action: AccessAction::ReviewNotice,
            resource: AccessResource::of(&conversation),
            held: None,
            request: RequestShape::default(),
            target: None,
        });
        let record = Arc::new(ReviewNoticeAuditRecord::new(
            &conversation,
            execution_id,
            permission_id,
            recipient,
            &current,
            cause,
            decision,
        ));
        // No await separates the policy answer from transfer of its immutable
        // record to the admitted owner. Caller/UI loss cannot cancel delivery.
        let acknowledgement = admitted.record(record.clone());
        acknowledgement.await?;
        Ok(Some(AuditedReviewSelection { record }))
    }
}
