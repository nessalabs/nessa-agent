//! The one-shot pass over every saved conversation.
use super::outcome::{
    ConversationRetrofit, Leftover, PermanentLeftover, RetrofitRun, RetrofitSummary,
    TransientLeftover,
};
use super::ports::{
    IdentityRetrofitAudit, IdentityRetrofitMarker, RestorationIdentities,
    RestorationIdentitySource, RetrofitAuditRecord, RetrofitCause, RetrofitConversations,
    RetrofitInitiator, RetrofitMove,
};
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    conversation_session, ConversationError, ConversationRepository,
};
use crate::conversation::domain::{ConversationApprovalMode, ConversationId};
use nessa_sdk::application::agent_execution::sessions::{
    SavedProviderIdentity, SessionLoadState, SessionStorage, StorageError,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Everything one run reads and writes through.
pub struct IdentityRetrofitPorts {
    pub conversations: Arc<dyn RetrofitConversations>,
    pub metadata: Arc<dyn ConversationRepository>,
    pub storage: Arc<dyn SessionStorage>,
    pub identities: Arc<dyn RestorationIdentitySource>,
    pub audit: Arc<dyn IdentityRetrofitAudit>,
    pub marker: Arc<dyn IdentityRetrofitMarker>,
}

/// Move every conversation saved under the previous restoration fingerprint
/// to the current one, once.
///
/// Run by composition as the gateway starts, after session storage is
/// initialised and the agent resolver exists, and before the conversation
/// service is built, so nothing else holds a session lease; the registry lock
/// has already refused a second gateway on this namespace. The order for each
/// conversation is: read its record, skip a tombstone, lease and read its
/// history, resolve its agent, decide, then — for a rewrite — record the
/// intent, append, and record the outcome. The marker is written only after
/// the summary is recorded and only when nothing transient was left
/// (`docs/design/mcp-connections.md`, state table rows R1–R19).
pub async fn run_identity_retrofit(ports: &IdentityRetrofitPorts) -> RetrofitRun {
    match ports.marker.done().await {
        Ok(true) => return RetrofitRun::AlreadyDone,
        Ok(false) => {}
        // Running again is safe: every step is decided from what is saved.
        Err(_) => tracing::warn!("identity retrofit marker could not be read; running"),
    }
    let mut summary = RetrofitSummary::default();
    match ports.conversations.every_conversation().await {
        Ok(every) => {
            summary.unreadable_records = every.unreadable;
            let mut resolved = Resolved::default();
            for id in every.conversations {
                let outcome = retrofit_one(ports, &mut resolved, &id).await;
                summary.count(id, outcome);
            }
        }
        Err(error) => {
            tracing::error!(%error, "conversations could not be listed for the identity retrofit");
            summary.listing_failed = true;
        }
    }
    if let Err(error) = ports
        .audit
        .record(RetrofitAuditRecord::Summary(summary.clone()))
        .await
    {
        tracing::error!(%error, "identity retrofit summary could not be recorded");
        return RetrofitRun::Incomplete(summary);
    }
    if summary.transient_left() {
        return RetrofitRun::Incomplete(summary);
    }
    match ports.marker.mark_done().await {
        Ok(()) => RetrofitRun::Done(summary),
        Err(_) => {
            tracing::error!("identity retrofit marker could not be written");
            RetrofitRun::Incomplete(summary)
        }
    }
}

/// Each selection resolved once per run: a deadline that ran out for one
/// conversation is not waited out again for every other on the same agent.
type Resolution = Result<RestorationIdentities, Leftover>;
#[derive(Default)]
struct Resolved(HashMap<(AgentId, String, &'static str), Resolution>);

impl Resolved {
    async fn get(
        &mut self,
        source: &dyn RestorationIdentitySource,
        agent: AgentId,
        model: &str,
        mode: ConversationApprovalMode,
    ) -> Resolution {
        let key = (agent, model.to_owned(), mode.as_str());
        if let Some(found) = self.0.get(&key) {
            return found.clone();
        }
        let answer = match source.identities(agent, model, mode).await {
            Ok(Some(identities)) => Ok(identities),
            Ok(None) | Err(ConversationError::AgentNotConfigured) => {
                Err(Leftover::Permanent(PermanentLeftover::AgentNotConfigured))
            }
            Err(ConversationError::AgentUnsupported) => {
                Err(Leftover::Permanent(PermanentLeftover::UnsupportedAgent))
            }
            Err(ConversationError::ModelUnavailable) => {
                Err(Leftover::Permanent(PermanentLeftover::ModelUnavailable))
            }
            Err(ConversationError::ApprovalModeUnavailable) => Err(Leftover::Permanent(
                PermanentLeftover::ApprovalModeUnavailable,
            )),
            Err(_) => Err(Leftover::Transient(TransientLeftover::Unavailable)),
        };
        self.0.insert(key, answer.clone());
        answer
    }
}

fn storage_leftover(error: &StorageError) -> Leftover {
    match error {
        StorageError::Busy => Leftover::Transient(TransientLeftover::Busy),
        StorageError::Corrupt(_)
        | StorageError::IdentityMismatch
        | StorageError::TooLarge
        | StorageError::DiagnosticLimit
        | StorageError::ChangesRequired => Leftover::Permanent(PermanentLeftover::Corrupt),
        StorageError::Closed
        | StorageError::ReadCapacity
        | StorageError::ReadWorkerPanicked
        | StorageError::ShutdownFailures(_)
        | StorageError::Io(_)
        | StorageError::Unresolved
        | StorageError::CommittedReadUnavailable => Leftover::Transient(TransientLeftover::Storage),
    }
}

async fn retrofit_one(
    ports: &IdentityRetrofitPorts,
    resolved: &mut Resolved,
    id: &ConversationId,
) -> ConversationRetrofit {
    let record = match ports.metadata.load(id).await {
        Ok(Some(record)) => record,
        // Listed, then gone: nothing of it is left to move.
        Ok(None) => return ConversationRetrofit::NoHistory,
        Err(ConversationError::AgentUnsupported) => {
            return ConversationRetrofit::LeftPermanent(PermanentLeftover::UnsupportedAgent)
        }
        Err(_) => return ConversationRetrofit::LeftTransient(TransientLeftover::Metadata),
    };
    if record.deletion().is_some() {
        return ConversationRetrofit::Tombstoned;
    }
    let Some(agent) = record.agent() else {
        return ConversationRetrofit::LeftPermanent(PermanentLeftover::UnsupportedAgent);
    };
    let session = conversation_session(id);
    let lease = match ports.storage.open_existing(session.clone()).await {
        Ok(Some(lease)) => lease,
        Ok(None) => return ConversationRetrofit::NoHistory,
        Err(error) => return storage_leftover(&error).outcome(),
    };
    let saved = match SavedProviderIdentity::load(lease.as_ref(), &session).await {
        Ok(Some(saved)) => saved,
        Ok(None) => return ConversationRetrofit::NoHistory,
        Err(error) => return storage_leftover(&error).outcome(),
    };
    let identities = match resolved
        .get(
            ports.identities.as_ref(),
            agent,
            record.model().as_str(),
            record.approval_mode(),
        )
        .await
    {
        Ok(identities) => identities,
        Err(reason) => return reason.outcome(),
    };
    if saved.provider() == &identities.current {
        return ConversationRetrofit::AlreadyCurrent;
    }
    if identities.previous.as_ref() != Some(saved.provider()) {
        return ConversationRetrofit::Foreign;
    }
    let attempted = RetrofitMove {
        conversation_id: id.clone(),
        before: saved.provider().clone(),
        after: identities.current.clone(),
        cause: RetrofitCause::OF_RUN,
        initiator: RetrofitInitiator::OF_RUN,
    };
    if let Err(error) = ports
        .audit
        .record(RetrofitAuditRecord::Rewriting(attempted.clone()))
        .await
    {
        tracing::error!(conversation_id = %id, %error, "identity retrofit intent could not be recorded; left unchanged");
        return ConversationRetrofit::LeftTransient(TransientLeftover::Audit);
    }
    let unfinished = saved.state() == SessionLoadState::Unfinished;
    let (evidence, outcome) = match saved
        .move_to(lease.as_ref(), identities.current.clone())
        .await
    {
        Ok(()) => (
            RetrofitAuditRecord::Rewritten(attempted),
            ConversationRetrofit::Rewritten,
        ),
        Err(error) => {
            tracing::error!(conversation_id = %id, %error, "identity retrofit move was not acknowledged");
            // Only an unfinished save the writer will not complete as this
            // move stays as it is for good; any other failed append may be
            // met differently next time.
            let reason = match storage_leftover(&error) {
                Leftover::Permanent(PermanentLeftover::Corrupt) if unfinished => {
                    Leftover::Permanent(PermanentLeftover::Corrupt)
                }
                _ => Leftover::Transient(TransientLeftover::Storage),
            };
            (
                RetrofitAuditRecord::Left { attempted, reason },
                reason.outcome(),
            )
        }
    };
    match ports.audit.record(evidence).await {
        Ok(()) => outcome,
        Err(error) => {
            tracing::error!(conversation_id = %id, %error, "identity retrofit outcome could not be recorded");
            ConversationRetrofit::LeftTransient(TransientLeftover::Audit)
        }
    }
}
