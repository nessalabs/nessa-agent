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
        observation, AdmissionFailure, AdmitDelivery, DeliveryCause, DispatchAdmission,
        ExpiryObserver, Message, ObserveRegistrationStop, Outbox, OutboxError, PendingDelivery,
        PrepareFailure, QueueInitiator, QueuedRecord, ReadWebhookRecords, Registration,
        StopObservationResult, WebhookHttp, WebhookReadFailure, MAX_CURSORS, MAX_OUTBOX_WORKERS,
        POST_DEADLINE,
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
#[derive(Clone)]
pub(super) struct PendingSettlement {
    registration: Uuid,
    before: Delivery,
    outcome: PendingOutcome,
    at_ms: Option<u64>,
}
#[derive(Clone)]
enum PendingOutcome {
    Response(Observation),
    Unposted { sending: Delivery, expired: bool },
}
#[derive(Debug)]
enum CycleFailure {
    Retry,
    Fatal,
}
impl From<CredentialAdminError> for CycleFailure {
    fn from(_: CredentialAdminError) -> Self {
        Self::Fatal
    }
}
fn store_failure(error: OutboxError) -> CycleFailure {
    match error {
        OutboxError::Unavailable | OutboxError::Capacity | OutboxError::Conflict => {
            CycleFailure::Retry
        }
        OutboxError::Invalid | OutboxError::ResetRequired => CycleFailure::Fatal,
    }
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
            match self.delivery_cycle(&dependencies, initiator).await {
                Ok(()) => {}
                Err(CycleFailure::Retry) => tracing::warn!(
                    "webhook local operation unavailable; retained sender will retry"
                ),
                Err(CycleFailure::Fatal) => return Err(CredentialAdminError::Unavailable),
            }
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
    ) -> Result<(), CycleFailure> {
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
        // This worker is serial: prior physical SQL/POST has joined, and a
        // retained actual outcome was settled above. Unowned Sending is Unknown.
        self.owner
            .store
            .recover_live(self.owner.clock.unix_milliseconds())
            .await
            .map_err(store_failure)?;
        let intents = self.owner.store.intents().await.map_err(store_failure)?;
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
                .map_err(store_failure)?
            else {
                continue;
            };
            match self
                .observe_stop(&registration, ExpiryObserver::Source(registration.id))
                .await?
            {
                StopObservationResult::StopAcknowledged => continue,
                StopObservationResult::NoStopObserved => {}
            }
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
                .map_err(store_failure)?
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
    ) -> Result<(), CycleFailure> {
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
            Err(_) => return Err(CycleFailure::Fatal),
        };
        let (scope, target) = match head.value() {
            Ok(RecordReadValue::Head(value)) => (value.scope.clone(), value.head),
            _ => return Err(CycleFailure::Fatal),
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
            Err(error) => return Err(store_failure(error)),
        };
        if after == target {
            return Ok(());
        }
        if after > target {
            return Err(CycleFailure::Fatal);
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
            Err(_) => return Err(CycleFailure::Fatal),
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
            return Err(CycleFailure::Fatal);
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
                Err(error) => return Err(store_failure(error)),
            }
        }
        Ok(())
    }
    async fn send_pending(
        &self,
        dependencies: &DeliveryDependencies,
        pending: PendingDelivery,
    ) -> Result<(), CycleFailure> {
        let prepared = match dependencies
            .http
            .prepare(&pending.registration.destination)
            .await
        {
            Ok(prepared) => prepared,
            Err(PrepareFailure::Busy) => return Ok(()),
            Err(_) => return Err(CycleFailure::Fatal),
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
                    Err(error) => return Err(store_failure(error)),
                }
                self.observe_stop(
                    &pending.registration,
                    ExpiryObserver::Delivery(pending.delivery.id()),
                )
                .await?;
                return Ok(());
            }
            Err(error) if transient_source(error) => return Ok(()),
            Err(_) => return Err(CycleFailure::Fatal),
        };
        let (scope, source_head) = match head.value() {
            Ok(RecordReadValue::Head(value)) => (value.scope.clone(), value.head),
            _ => return Err(CycleFailure::Fatal),
        };
        drop(head); // Physical SDK read joined; private I/O owns no send permit.
        match self
            .owner
            .store
            .source_after(
                pending.registration.id,
                pending.conversation.clone(),
                scope.clone(),
                QueueInitiator::Live,
                self.owner.clock.unix_milliseconds(),
            )
            .await
        {
            Ok(_) => {}
            Err(OutboxError::ResetRequired | OutboxError::Conflict) => return Ok(()),
            Err(error) => return Err(store_failure(error)),
        }
        if scope != pending.scope || source_head < pending.position {
            return Err(CycleFailure::Fatal);
        }
        // Canonical physical read and cursor work ended. Re-admit against the
        // actual current credential/grant; the prior read is not dispatch authority.
        let deadline = tokio::time::Instant::now() + POST_DEADLINE;
        let hold = match self
            .admit_dispatch_before(
                &reader.admission,
                &pending.registration.target,
                &pending.conversation,
                pending.delivery.epoch(),
                deadline,
            )
            .await
        {
            Ok(Some(hold)) => hold,
            Ok(None) => return Ok(()),
            Err(AdmissionFailure::Refused) => {
                self.observe_stop(
                    &pending.registration,
                    ExpiryObserver::Delivery(pending.delivery.id()),
                )
                .await?;
                return Ok(());
            }
            Err(AdmissionFailure::Unavailable) => return Err(CycleFailure::Retry),
        };
        let at = self.owner.clock.unix_milliseconds();
        let sending = pending
            .delivery
            .admit(pending.delivery.epoch(), at)
            .map_err(|_| CredentialAdminError::Unavailable)?;
        *self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(PendingSettlement {
            registration: pending.registration.id,
            before: pending.delivery.clone(),
            outcome: PendingOutcome::Unposted {
                sending: sending.clone(),
                expired: false,
            },
            at_ms: None,
        });
        let write = self
            .owner
            .store
            .admit_before(&pending.delivery, &sending, at, deadline);
        tokio::pin!(write);
        let receipt = tokio::select! {
            result = &mut write => Some(result),
            _ = tokio::time::sleep_until(deadline) => None,
        };
        if receipt.is_none() {
            // The future stays pinned/alive: any real spawn_blocking SQL remains
            // owned until joined. No expired attempt retains a POST reservation.
            drop(prepared);
            hold.cancel();
            #[cfg(test)]
            self.owner.dispatch_deadline.notify_one();
            let _physical_result = write.await;
            let _ = self.settle_known().await;
            return Ok(());
        }
        if let Some(Err(error)) = receipt {
            drop(prepared);
            // Try W-audit-fail under the original authority before releasing it.
            // If that write is also pending at deadline, join it without authority.
            let settlement = self.settle_known();
            tokio::pin!(settlement);
            let settled = tokio::select! {
                result = &mut settlement => Some(result),
                _ = tokio::time::sleep_until(deadline) => None,
            };
            hold.cancel();
            if settled.is_none() {
                let _ = settlement.await;
            }
            return if error == OutboxError::Invalid {
                Err(CycleFailure::Fatal)
            } else {
                Ok(())
            };
        }
        let expired = !hold.is_valid_at(self.owner.clock.unix_seconds());
        if expired || tokio::time::Instant::now() >= deadline {
            if expired {
                if let Some(known) = self
                    .owner
                    .known
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_mut()
                {
                    known.outcome = PendingOutcome::Unposted {
                        sending: sending.clone(),
                        expired: true,
                    };
                }
            }
            drop(prepared);
            hold.cancel();
            let _ = self.settle_known().await;
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
        let observed = {
            let post = prepared.post(&pending.registration.key, &message);
            tokio::pin!(post);
            tokio::select! {
                biased;
                actual = &mut post => observation(actual),
                _ = tokio::time::sleep_until(deadline) => Observation::TimedOut,
            }
        }; // The POST future is gone before authority can be released.
        *self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(PendingSettlement {
            registration: pending.registration.id,
            before: sending.clone(),
            outcome: PendingOutcome::Response(observed),
            at_ms: None,
        });
        hold.cancel(); // Actual response captured; settlement needs no authority.
        let settled = self.settle_known().await;
        if settled.is_err() {
            tracing::warn!("webhook response retained for local settlement; no repeat POST");
        }
        Ok(())
    }
    /// Caller retains this future through cancellation/deadline until the actual
    /// native read/acquisition joins. Partial fences have an independent owner.
    pub(crate) async fn admit_dispatch_before(
        &self,
        admission: &AdmitDelivery<'_>,
        target: &crate::webhooks::application::Target,
        conversation: &ConversationId,
        epoch: u64,
        deadline: tokio::time::Instant,
    ) -> Result<Option<DispatchAdmission>, AdmissionFailure> {
        let hold = DispatchAdmission::new();
        let admitted = {
            let work = admission.execute_dispatch(&hold, target, conversation, epoch);
            tokio::pin!(work);
            let admitted = tokio::select! {
                result = &mut work => Some(result),
                _ = tokio::time::sleep_until(deadline) => None,
            };
            match admitted {
                None => {
                    hold.cancel();
                    #[cfg(test)]
                    self.owner.dispatch_deadline.notify_one();
                    let _physical_result = work.await;
                    return Ok(None);
                }
                Some(result) => result,
            }
        }; // The borrowed admission future is joined and destroyed here.
        match admitted {
            Err(error) => {
                hold.cancel();
                Err(error)
            }
            Ok(()) if tokio::time::Instant::now() >= deadline => {
                hold.cancel();
                Ok(None)
            }
            Ok(()) => Ok(Some(hold)),
        }
    }

    async fn observe_stop(
        &self,
        registration: &Registration,
        reader: ExpiryObserver,
    ) -> Result<StopObservationResult, CycleFailure> {
        ObserveRegistrationStop {
            credentials: self.owner.credentials.as_ref(),
            stops: self.owner.store.as_ref(),
            clock: self.owner.clock.as_ref(),
        }
        .execute(&registration.target.credential, reader)
        .await
        .map_err(|error| match error {
            crate::webhooks::application::StopFailure::Store(error) => store_failure(error),
            crate::webhooks::application::StopFailure::Access(_) => CycleFailure::Retry,
        })
    }
    pub(super) async fn settle_known(&self) -> Result<(), CredentialAdminError> {
        let selected = self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(mut selected) = selected else {
            return Ok(());
        };
        let at = selected
            .at_ms
            .unwrap_or_else(|| self.owner.clock.unix_milliseconds());
        selected.at_ms = Some(at);
        // The timestamp is retained even if publication fails again.
        *self
            .owner
            .known
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(selected.clone());
        match selected.outcome {
            PendingOutcome::Response(observed) => {
                let after = selected
                    .before
                    .observed(observed, at)
                    .map_err(|_| CredentialAdminError::Unavailable)?;
                self.owner
                    .store
                    .settle_response(&selected.before, &after, at)
                    .await
            }
            PendingOutcome::Unposted { sending, expired } => {
                self.owner
                    .store
                    .settle_unposted(
                        selected.registration,
                        &selected.before,
                        &sending,
                        expired,
                        at,
                    )
                    .await
            }
        }
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
