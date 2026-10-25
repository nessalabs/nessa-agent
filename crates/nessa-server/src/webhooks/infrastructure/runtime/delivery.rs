//! One serial, retained canonical-source/queue/signed-send worker.
use super::WebhookRuntime;
use crate::conversation::{
    application::{
        ConversationAccessPolicyEvaluator, PrincipalRecordReadSource, RecordReadError,
        RecordReadOperation, RecordReadValue,
    },
    infrastructure::LocalConversationStore,
};
use crate::webhooks::{
    application::{
        observation, AdmissionFailure, AdmitDelivery, DeliveryCause, ExpiryObserver, Message,
        ObserveRegistrationStop, Outbox, OutboxError, PendingDelivery, PrepareFailure,
        QueueInitiator, QueuedRecord, ReadWebhookRecords, Registration, WebhookHttp,
        WebhookReadFailure, MAX_CURSORS, MAX_OUTBOX_WORKERS,
    },
    domain::{Delivery, Observation},
    infrastructure::record_message,
};
use nessa_auth::{application::credential_admin::CredentialAdminError, domain::Resource};
use nessa_protocol::{
    conversation::domain::ConversationId,
    product::generated::{
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
    },
};
use nessa_sync::replication::domain::{Limits, PageRequest};
use serde::Deserialize;
use serde_json::value::RawValue;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::{
    sync::{Arc, PoisonError},
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub(crate) struct DeliveryDependencies {
    pub metadata: Arc<LocalConversationStore>,
    pub source: Arc<dyn PrincipalRecordReadSource>,
    pub policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    pub capacity: Arc<Semaphore>,
    pub gateway: Resource,
    pub http: Arc<dyn WebhookHttp>,
}
/// Captured before clocks or persistence are called after transport returns.
/// This is not a durable receipt and is never used to infer a missing ACK.
pub(super) struct KnownObservation {
    before: Delivery,
    observed: Observation,
    at_ms: Option<u64>,
}
#[derive(Deserialize)]
struct EncodedPage {
    payload: EncodedRecords,
}
#[derive(Deserialize)]
struct EncodedRecords {
    records: Vec<Box<RawValue>>,
}

impl WebhookRuntime {
    fn accepting_delivery(&self) -> bool {
        self.owner
            .commands
            .lock()
            .map(|commands| commands.accepting)
            .unwrap_or(false)
    }
    fn reader<'a>(&'a self, dependencies: &'a DeliveryDependencies) -> ReadWebhookRecords<'a> {
        ReadWebhookRecords {
            admission: AdmitDelivery {
                credentials: self.owner.credentials.as_ref(),
                grants: dependencies.metadata.as_ref(),
                conversations: dependencies.metadata.as_ref(),
                policy: dependencies.policy.as_ref(),
                clock: self.owner.clock.as_ref(),
                gateway: &dependencies.gateway,
            },
            source: dependencies.source.as_ref(),
            capacity: &dependencies.capacity,
            limits: Limits::new(
                MAX_RECORD_PAGE_RECORDS,
                MAX_RECORD_PAGE_PAYLOAD_BYTES,
                MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            )
            .expect("canonical positive record ceilings"),
        }
    }
    pub(super) async fn delivery_loop(
        &self,
        dependencies: DeliveryDependencies,
    ) -> Result<(), CredentialAdminError> {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut initiator = QueueInitiator::Restart;
        loop {
            if !self.accepting_delivery() {
                return self.settle_known().await;
            }
            self.delivery_cycle(&dependencies, initiator).await?;
            initiator = QueueInitiator::Live;
            #[cfg(test)]
            {
                self.owner.delivery_cycles.fetch_add(1, Ordering::SeqCst);
                self.owner.cycle_finished.notify_waiters();
            }
            tokio::select! {_=self.owner.wake.notified()=>{},_=interval.tick()=>{}}
        }
    }
    async fn delivery_cycle(
        &self,
        dependencies: &DeliveryDependencies,
        initiator: QueueInitiator,
    ) -> Result<(), CredentialAdminError> {
        // Drop the synchronous guard before any settlement await.
        let pending = self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if pending && self.settle_known().await.is_err() {
            return Ok(());
        }
        let intents = self
            .owner
            .store
            .intents()
            .await
            .map_err(|_| CredentialAdminError::Unavailable)?;
        for intent in intents {
            if !self.accepting_delivery() {
                return Ok(());
            }
            let credential = nessa_auth::domain::CredentialId::new(&intent.issue.credential_id)
                .map_err(|_| CredentialAdminError::Unavailable)?;
            let Some(registration) = self
                .owner
                .store
                .registration(credential)
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
            else {
                continue;
            };
            let conversations = dependencies
                .metadata
                .principal_conversations(
                    registration.target.principal.clone(),
                    registration.target.credential.clone(),
                    MAX_CURSORS,
                )
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?;
            for (conversation, epoch) in conversations {
                if !self.accepting_delivery() {
                    return Ok(());
                }
                self.capture(dependencies, &registration, &conversation, epoch, initiator)
                    .await?;
            }
        }
        for _ in 0..MAX_OUTBOX_WORKERS {
            if !self.accepting_delivery() {
                break;
            }
            // A prior attempt can retire an entire registration and its queue.
            // Re-read bounded current work instead of reusing the old batch.
            let Some(pending) = self
                .owner
                .store
                .due(self.owner.clock.unix_milliseconds())
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?
                .into_iter()
                .next()
            else {
                break;
            };
            self.send_pending(dependencies, pending).await?;
            if self
                .owner
                .known
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
            {
                break;
            }
        }
        Ok(())
    }
    async fn capture(
        &self,
        dependencies: &DeliveryDependencies,
        registration: &Registration,
        conversation: &ConversationId,
        epoch: u64,
        initiator: QueueInitiator,
    ) -> Result<(), CredentialAdminError> {
        let reader = self.reader(dependencies);
        let head = match reader
            .execute(registration, conversation, epoch, RecordReadOperation::Head)
            .await
        {
            Ok(head) => head,
            Err(WebhookReadFailure::Admission(AdmissionFailure::Refused)) => {
                self.observe_stop(registration, ExpiryObserver::Source(registration.id))
                    .await?;
                return Ok(());
            }
            Err(error) if transient_source(error) => return Ok(()),
            Err(_) => return Err(CredentialAdminError::Unavailable),
        };
        let (scope, target) = match head.value() {
            Ok(RecordReadValue::Head(value)) => (value.scope.clone(), value.head),
            _ => return Err(CredentialAdminError::Unavailable),
        };
        let after = self
            .owner
            .store
            .source_after(
                registration.id,
                conversation.clone(),
                scope.clone(),
                initiator,
                self.owner.clock.unix_milliseconds(),
            )
            .await;
        drop(head); // Never acquire a second Auth/grant hold while retaining this one.
        let after = match after {
            Ok(after) => after,
            Err(OutboxError::ResetRequired | OutboxError::Capacity | OutboxError::Conflict) => {
                return Ok(())
            }
            Err(_) => return Err(CredentialAdminError::Unavailable),
        };
        if after == target {
            return Ok(());
        }
        if after > target {
            return Err(CredentialAdminError::Unavailable);
        }
        let request = PageRequest {
            scope,
            after,
            target,
            max_records: MAX_RECORD_PAGE_RECORDS,
            max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        let page = match reader
            .execute(
                registration,
                conversation,
                epoch,
                RecordReadOperation::Page(request.clone()),
            )
            .await
        {
            Ok(page) => page,
            Err(WebhookReadFailure::Admission(AdmissionFailure::Refused)) => return Ok(()),
            Err(error) if transient_source(error) => return Ok(()),
            Err(_) => return Err(CredentialAdminError::Unavailable),
        };
        let value = match page.value() {
            Ok(RecordReadValue::Page(value)) => value,
            _ => return Ok(()),
        };
        let encoded = nessa_protocol::product::record_read::encode_page(
            "webhook-source",
            &request,
            value.clone(),
        )
        .map_err(|_| CredentialAdminError::Unavailable)?;
        let wire: EncodedPage =
            serde_json::from_str(&encoded).map_err(|_| CredentialAdminError::Unavailable)?;
        if wire.payload.records.len() != value.records.len() {
            return Err(CredentialAdminError::Unavailable);
        }
        for (record, raw) in value.records.iter().zip(wire.payload.records.iter()) {
            if !self.accepting_delivery() || page.value().is_err() {
                break;
            }
            let at = self.owner.clock.unix_milliseconds();
            let message = record_message(
                Uuid::new_v4(),
                at,
                &conversation.to_string(),
                request.scope.incarnation().as_str(),
                record.position,
                raw.get(),
            )
            .map_err(|_| CredentialAdminError::Unavailable)?;
            match self
                .owner
                .store
                .queue(QueuedRecord {
                    registration: registration.id,
                    conversation: conversation.clone(),
                    scope: request.scope.clone(),
                    expected_after: record.position - 1,
                    position: record.position,
                    epoch,
                    body: message.body().to_vec(),
                    id: message.id(),
                    observed_at_ms: at,
                    initiator,
                })
                .await
            {
                Ok(_) => {}
                Err(OutboxError::Capacity | OutboxError::Conflict | OutboxError::ResetRequired) => {
                    break
                }
                Err(_) => return Err(CredentialAdminError::Unavailable),
            }
        }
        Ok(())
    }
    async fn send_pending(
        &self,
        dependencies: &DeliveryDependencies,
        pending: PendingDelivery,
    ) -> Result<(), CredentialAdminError> {
        let prepared = match dependencies
            .http
            .prepare(&pending.registration.destination)
            .await
        {
            Ok(prepared) => prepared,
            Err(PrepareFailure::Busy) => return Ok(()),
            Err(_) => return Err(CredentialAdminError::Unavailable),
        };
        let reader = self.reader(dependencies);
        let head = match reader
            .execute(
                &pending.registration,
                &pending.conversation,
                pending.delivery.epoch(),
                RecordReadOperation::Head,
            )
            .await
        {
            Ok(head) => head,
            Err(WebhookReadFailure::Admission(AdmissionFailure::Refused)) => {
                let refused = pending
                    .delivery
                    .refused()
                    .map_err(|_| CredentialAdminError::Unavailable)?;
                let refused = self
                    .owner
                    .store
                    .transition(
                        &pending.delivery,
                        &refused,
                        DeliveryCause::Refused,
                        self.owner.clock.unix_milliseconds(),
                    )
                    .await;
                // No dispatch began. A concurrent stop can already have retired
                // this queued snapshot; a conflict is not an acknowledgement.
                match refused {
                    Ok(()) | Err(OutboxError::Conflict) => {}
                    Err(_) => return Err(CredentialAdminError::Unavailable),
                }
                self.observe_stop(
                    &pending.registration,
                    ExpiryObserver::Delivery(pending.delivery.id()),
                )
                .await?;
                return Ok(());
            }
            Err(error) if transient_source(error) => return Ok(()),
            Err(_) => return Err(CredentialAdminError::Unavailable),
        };
        let value = match head.value() {
            Ok(RecordReadValue::Head(value)) => value,
            _ => return Err(CredentialAdminError::Unavailable),
        };
        match self
            .owner
            .store
            .source_after(
                pending.registration.id,
                pending.conversation.clone(),
                value.scope.clone(),
                QueueInitiator::Live,
                self.owner.clock.unix_milliseconds(),
            )
            .await
        {
            Ok(_) => {}
            Err(OutboxError::ResetRequired | OutboxError::Conflict) => return Ok(()),
            Err(_) => return Err(CredentialAdminError::Unavailable),
        }
        if value.scope != pending.scope || value.head < pending.position {
            return Err(CredentialAdminError::Unavailable);
        }
        let at = self.owner.clock.unix_milliseconds();
        let sending = pending
            .delivery
            .admit(pending.delivery.epoch(), at)
            .map_err(|_| CredentialAdminError::Unavailable)?;
        self.owner
            .store
            .transition(&pending.delivery, &sending, DeliveryCause::Admission, at)
            .await
            .map_err(|_| CredentialAdminError::Unavailable)?;
        if head.value().is_err() {
            let after = sending
                .not_dispatched()
                .map_err(|_| CredentialAdminError::Unavailable)?;
            self.owner
                .store
                .transition(
                    &sending,
                    &after,
                    DeliveryCause::AdmissionExpired,
                    self.owner.clock.unix_milliseconds(),
                )
                .await
                .map_err(|_| CredentialAdminError::Unavailable)?;
            drop(head);
            self.observe_stop(
                &pending.registration,
                ExpiryObserver::Delivery(sending.id()),
            )
            .await?;
            return Ok(());
        }
        let message = Message::encoded(
            sending.id(),
            sending
                .timestamp_ms()
                .ok_or(CredentialAdminError::Unavailable)?,
            pending.body,
        )
        .map_err(|_| CredentialAdminError::Unavailable)?;
        let observed = observation(prepared.post(&pending.registration.key, &message).await);
        *self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(KnownObservation {
            before: sending,
            observed,
            at_ms: None,
        });
        let settled = self.settle_known().await;
        drop(head); // Physical POST and first settlement operation have ended.
        if settled.is_err() {
            tracing::warn!("webhook response retained for local settlement; no repeat POST");
        }
        Ok(())
    }
    async fn observe_stop(
        &self,
        registration: &Registration,
        reader: ExpiryObserver,
    ) -> Result<(), CredentialAdminError> {
        ObserveRegistrationStop {
            credentials: self.owner.credentials.as_ref(),
            stops: self.owner.store.as_ref(),
            clock: self.owner.clock.as_ref(),
        }
        .execute(&registration.target.credential, reader)
        .await
        .map(|_| ())
        .map_err(|_| CredentialAdminError::Unavailable)
    }
    pub(super) async fn settle_known(&self) -> Result<(), CredentialAdminError> {
        let selected = self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|known| (known.before.clone(), known.observed, known.at_ms));
        let Some((before, observed, at)) = selected else {
            return Ok(());
        };
        let at = at.unwrap_or_else(|| self.owner.clock.unix_milliseconds());
        if let Some(known) = self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            known.at_ms = Some(at);
        }
        let after = before
            .observed(observed, at)
            .map_err(|_| CredentialAdminError::Unavailable)?;
        self.owner
            .store
            .settle_response(&before, &after, at)
            .await
            .map_err(|_| CredentialAdminError::Unavailable)?;
        self.owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        Ok(())
    }
}

// Canonical discovery reports bounded progress, not a failed supervisor.
fn transient_source(error: WebhookReadFailure) -> bool {
    matches!(
        error,
        WebhookReadFailure::Source(
            RecordReadError::TemporarilyUnavailable | RecordReadError::SourcePreparing
        )
    )
}
