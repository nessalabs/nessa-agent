//! Root admission retains transaction ownership until the caller claims its ID.
use std::sync::Arc;
use tokio::sync::oneshot;

use super::{
    coordinator::{mint_lifetime, Shared},
    publication::{PublicationTarget, PublishedState},
    OwnershipFailure, PortFailure,
};
use crate::domain::agent_execution::{
    sessions::SessionId,
    subagents::{AgentLifetimeId, EvidenceFact, Initiator},
};

struct DeliveryTicket {
    shared: Arc<Shared>,
    lifetime: Option<AgentLifetimeId>,
}
impl DeliveryTicket {
    fn claim(mut self) -> AgentLifetimeId {
        self.lifetime.take().expect("root delivery ticket")
    }
}
impl Drop for DeliveryTicket {
    fn drop(&mut self) {
        if let Some(lifetime) = self.lifetime.take() {
            let shared = Arc::clone(&self.shared);
            tokio::spawn(async move {
                reconcile(&shared, &lifetime).await;
            });
        }
    }
}

pub(super) async fn open(
    shared: Arc<Shared>,
    session: SessionId,
    initiator: Initiator,
) -> Result<AgentLifetimeId, OwnershipFailure> {
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let result = admit(&shared, session, initiator).await;
        // A successful queued result still owns the ID until synchronously claimed.
        let result = result.map(|lifetime| DeliveryTicket {
            shared,
            lifetime: Some(lifetime),
        });
        let _ = sender.send(result);
    });
    receiver
        .await
        .map_err(|_| OwnershipFailure::Incomplete)?
        .map(DeliveryTicket::claim)
}

async fn admit(
    shared: &Arc<Shared>,
    session: SessionId,
    initiator: Initiator,
) -> Result<AgentLifetimeId, OwnershipFailure> {
    let lifetime = mint_lifetime();
    let (evidence, token) = shared.with_graph(|graph| {
        let evidence = graph
            .open_root(session, lifetime.clone(), initiator)
            .map_err(OwnershipFailure::Domain)?;
        shared.remember(Arc::downgrade(shared), lifetime.clone(), false);
        let token = shared
            .publication
            .lock()
            .expect("ownership publication")
            .begin(
                PublicationTarget::Root(lifetime.clone()),
                PublishedState::Root,
            );
        Ok::<_, OwnershipFailure>((evidence, token))
    })?;
    if let Err(error) = shared.publish_evidence(&evidence, &token, None).await {
        let removed = error == OwnershipFailure::Audit(PortFailure::Rejected)
            && shared.drop_unpublished_root(&lifetime);
        if !removed {
            reconcile(shared, &lifetime).await;
        }
        return Err(error);
    }
    Ok(lifetime)
}

async fn reconcile(shared: &Arc<Shared>, lifetime: &AgentLifetimeId) {
    if let Ok(evidence) = shared.retain_and_seal_root(lifetime) {
        let _ = shared.persist_evidence(&evidence).await;
        shared.start_drain(lifetime.clone(), false);
    }
}

/// Physical absence is independent from the audit acknowledgment.
pub(super) async fn settle_never_bound(
    shared: &Shared,
    root: &AgentLifetimeId,
) -> Result<(), OwnershipFailure> {
    let token = shared
        .with_graph(|graph| {
            if shared.bound.lock().expect("bound lifetimes").contains(root) {
                return Ok(None);
            }
            let operation = graph
                .close_operation(root)
                .cloned()
                .ok_or(OwnershipFailure::Incomplete)?;
            shared
                .absence_claimed
                .lock()
                .expect("absence claims")
                .insert(root.clone());
            let token = graph
                .note_unbound_root(root, &operation)
                .map_err(OwnershipFailure::Domain)?;
            Ok::<_, OwnershipFailure>(Some(token))
        })?
        .ok_or(OwnershipFailure::Incomplete)?;
    let audit = shared.audit.record(token.evidence()).await;
    shared
        .with_graph(|graph| {
            graph.acknowledge_unbound_root(
                token,
                if audit.is_ok() {
                    EvidenceFact::Acknowledged
                } else {
                    EvidenceFact::Failed
                },
            )
        })
        .map_err(OwnershipFailure::Domain)?;
    let stored = shared.commit_snapshot().await;
    match (audit, stored) {
        (Err(failure), _) => Err(OwnershipFailure::Audit(failure)),
        (_, Err(failure)) => Err(OwnershipFailure::Store(failure)),
        _ => Ok(()),
    }
}
