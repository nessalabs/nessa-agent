//! The phone outbox. A draft never claims it was submitted.

use super::super::domain::{
    after_lookup, after_send, Cause, Initiator, IntentEvent, LookupAnswer, PhoneCommandId,
    PhoneIntent, PhoneOutcome, PhoneRecord, PhoneView, Step, UnknownCause, FOREGROUND_SENDS,
};
use super::ports::{CommandIds, IntentStore, OutboxError, PhoneGateway};
use nessa_auth::application::ports::Clock;
use std::sync::Arc;

/// One device's saved sends.
pub struct PhoneOutbox<S, G, I> {
    store: S,
    gateway: G,
    ids: I,
    clock: Arc<dyn Clock>,
}
impl<S, G, I> PhoneOutbox<S, G, I>
where
    S: IntentStore,
    G: PhoneGateway,
    I: CommandIds,
{
    /// `store` keeps the commands, `gateway` asks the two product calls, and
    /// `ids` names a draft created after `turn_running`. `clock` stamps the
    /// audit row. None of them is looked up later.
    pub fn new(store: S, gateway: G, ids: I, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            gateway,
            ids,
            clock,
        }
    }

    /// Save `intent` before any send. The same bytes return the saved command.
    /// Different bytes under that command id are [`OutboxError::Conflict`] and
    /// the stored bytes stay (`a_changed_body_under_the_same_command_is_refused`).
    ///
    /// The audit initiator is [`Initiator::Person`].
    pub fn hold(&mut self, intent: PhoneIntent) -> Result<PhoneView, OutboxError> {
        self.hold_as(intent, Initiator::Person)
    }

    fn hold_as(&mut self, intent: PhoneIntent, by: Initiator) -> Result<PhoneView, OutboxError> {
        match self.store.load(intent.command())? {
            None => {
                let record = PhoneRecord::held(intent);
                self.store
                    .insert(&record, self.event(Cause::Held, record.outcome(), by))?;
                Ok(record.view())
            }
            Some(existing) if existing.intent().same_bytes(&intent) => Ok(existing.view()),
            Some(_) => Err(OutboxError::Conflict),
        }
    }

    /// Send a saved draft, or read the receipt of a command that already left.
    ///
    /// A draft is marked departed before `conversation.send`. A command that
    /// already left is only looked up: this call does not send it again.
    /// An accepted, running, done, failed, attempted, or unresolved command
    /// is returned as saved.
    ///
    /// The audit initiator is [`Initiator::Person`].
    pub fn send(&mut self, id: &PhoneCommandId) -> Result<PhoneView, OutboxError> {
        self.send_as(id, Initiator::Person)
    }

    fn send_as(&mut self, id: &PhoneCommandId, by: Initiator) -> Result<PhoneView, OutboxError> {
        let record = self.store.load(id)?.ok_or(OutboxError::NotHeld)?;
        if !matches!(record.outcome(), PhoneOutcome::Draft) {
            return self.reconcile(record, by, false);
        }
        let departed = record.clone().depart().ok_or(OutboxError::Conflict)?;
        self.persist(&record, &departed, Cause::Departed, by)?;
        self.foreground(departed, by, Cause::SendAnswered)
    }

    /// Look up every command that may have left and has no confirmed answer.
    ///
    /// This does not send (`recovery_does_not_send_when_the_receipt_is_absent`).
    /// The audit initiator is [`Initiator::Recovery`], which `hold`, `send`,
    /// and `retry` do not record.
    pub fn recover(&mut self) -> Result<Vec<PhoneView>, OutboxError> {
        let pending: Vec<PhoneRecord> = self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.departed() && retryable(record))
            .collect();
        let mut views = Vec::with_capacity(pending.len());
        for record in pending {
            views.push(self.reconcile(record, Initiator::Recovery, false)?);
        }
        Ok(views)
    }

    /// Look the command up, then send the same bytes only when the gateway
    /// has no receipt and the saved outcome is still unknown for that reason.
    ///
    /// Unresolved, attempted, failed, and finished commands are not sent
    /// (`an_explicit_retry_does_not_send_an_uncertain_or_finished_command`).
    ///
    /// The audit initiator is [`Initiator::Person`]. A lookup that finds
    /// nothing takes the store's writer lease before `conversation.send`, so
    /// a second retry of the same row waits and does not send
    /// (`a_concurrent_retry_sends_the_departure_once`).
    pub fn retry(&mut self, id: &PhoneCommandId) -> Result<PhoneView, OutboxError> {
        self.retry_as(id, Initiator::Person)
    }

    fn retry_as(&mut self, id: &PhoneCommandId, by: Initiator) -> Result<PhoneView, OutboxError> {
        let record = self.store.load(id)?.ok_or(OutboxError::NotHeld)?;
        if matches!(record.outcome(), PhoneOutcome::Draft) {
            return self.send_as(id, by);
        }
        if !retryable(&record) {
            return Ok(record.view());
        }
        let answer = self.gateway.lookup(record.intent());
        if !matches!(answer, LookupAnswer::NotFound) {
            return self.apply_lookup(record, answer, by, false);
        }
        self.exclusive(|outbox| outbox.send_after_not_found(record, by))
    }

    /// The lease is already held. Send only when the row is still the one
    /// the lookup authorized.
    fn send_after_not_found(
        &mut self,
        original: PhoneRecord,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        let record = self
            .store
            .load(original.intent().command())?
            .ok_or(OutboxError::NotHeld)?;
        if record != original {
            return Ok(record.view());
        }
        let looked = record
            .clone()
            .with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
        self.persist_asked(&record, &looked, Cause::Lookup, by)?;
        let reset = looked.clone().with_sends(0);
        self.persist_asked(&looked, &reset, Cause::ExplicitRetry, by)?;
        self.foreground(reset, by, Cause::ExplicitRetry)
    }

    fn exclusive<T>(
        &mut self,
        body: impl FnOnce(&mut Self) -> Result<T, OutboxError>,
    ) -> Result<T, OutboxError> {
        self.store
            .begin_exclusive()
            .map_err(|error| asked(OutboxError::from(error)))?;
        let result = body(self);
        if result.is_ok() {
            match self.store.end_exclusive() {
                Ok(()) => result,
                Err(error) => Err(asked(OutboxError::from(error))),
            }
        } else {
            self.store.abort_exclusive();
            result
        }
    }

    /// Every saved command, drafts included.
    pub fn list(&mut self) -> Result<Vec<PhoneView>, OutboxError> {
        Ok(self.store.list()?.iter().map(PhoneRecord::view).collect())
    }

    fn reconcile(
        &mut self,
        record: PhoneRecord,
        by: Initiator,
        allow_send: bool,
    ) -> Result<PhoneView, OutboxError> {
        if !matches!(
            record.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted | UnknownCause::LookupFailed)
        ) {
            return Ok(record.view());
        }
        let answer = self.gateway.lookup(record.intent());
        self.apply_lookup(record, answer, by, allow_send)
    }

    fn foreground(
        &mut self,
        mut record: PhoneRecord,
        by: Initiator,
        mut cause: Cause,
    ) -> Result<PhoneView, OutboxError> {
        loop {
            if record.sends() >= FOREGROUND_SENDS {
                let next = record
                    .clone()
                    .with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
                self.persist_asked(&record, &next, Cause::Bound, by)?;
                tracing::warn!(
                    limit = "client.foreground_sends",
                    "product session hit an operational limit"
                );
                return Ok(next.view());
            }
            let answer = self.gateway.send(record.intent());
            match after_send(record.clone(), answer) {
                Step::Done(next) => {
                    self.persist_asked(&record, &next, cause, by)?;
                    return Ok(next.view());
                }
                Step::ReturnToDraft(next) => return self.return_to_draft(&record, next, by),
                Step::Lookup {
                    record: bumped,
                    resend,
                } => {
                    let lost = if resend {
                        Cause::SendLost
                    } else {
                        Cause::Unreadable
                    };
                    self.persist_asked(&record, &bumped, lost, by)?;
                    let looked = self.gateway.lookup(bumped.intent());
                    match after_lookup(bumped.clone(), looked, resend) {
                        Step::Done(next) => {
                            if resend && stopped_at_bound(&bumped, &next) {
                                tracing::warn!(
                                    limit = "client.foreground_sends",
                                    "product session hit an operational limit"
                                );
                            }
                            self.persist_asked(&bumped, &next, Cause::Lookup, by)?;
                            return Ok(next.view());
                        }
                        Step::ReturnToDraft(next) => {
                            return self.return_to_draft(&bumped, next, by)
                        }
                        Step::Send(next) => {
                            self.persist_asked(&bumped, &next, Cause::ForegroundSend, by)?;
                            record = next;
                            cause = Cause::ForegroundSend;
                        }
                        Step::Lookup { .. } => return Err(OutboxError::Unsaved),
                    }
                }
                Step::Send(_) => return Err(OutboxError::Unsaved),
            }
        }
    }

    fn apply_lookup(
        &mut self,
        record: PhoneRecord,
        answer: LookupAnswer,
        by: Initiator,
        allow_send: bool,
    ) -> Result<PhoneView, OutboxError> {
        match after_lookup(record.clone(), answer, allow_send) {
            Step::Done(next) => {
                self.persist_asked(&record, &next, Cause::Lookup, by)?;
                Ok(next.view())
            }
            Step::ReturnToDraft(next) => self.return_to_draft(&record, next, by),
            Step::Send(next) => {
                self.persist_asked(&record, &next, Cause::Lookup, by)?;
                self.foreground(next, by, Cause::ForegroundSend)
            }
            Step::Lookup { .. } => Err(OutboxError::Unsaved),
        }
    }

    fn return_to_draft(
        &mut self,
        previous: &PhoneRecord,
        failed: PhoneRecord,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        if failed.successor().is_some() {
            self.persist_asked(previous, &failed, Cause::ReturnedToDraft, by)?;
            return Ok(failed.view());
        }
        let (command, turn) = self.ids.mint()?;
        let draft = PhoneRecord::held(
            failed
                .intent()
                .again(command.clone(), turn)
                .map_err(OutboxError::Id)?,
        );
        let failed = failed.with_successor(command);
        let failed_event = self.event(Cause::ReturnedToDraft, failed.outcome(), by);
        let draft_event = self.event(Cause::Held, draft.outcome(), by);
        self.store
            .replace_and_insert(previous, &failed, &draft, failed_event, draft_event)
            .map_err(|error| asked(OutboxError::from(error)))?;
        Ok(failed.view())
    }

    fn persist(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
    ) -> Result<(), OutboxError> {
        self.store
            .replace(previous, next, self.event(cause, next.outcome(), by))?;
        Ok(())
    }

    /// The gateway has already been asked. A failed save must not be reported
    /// as [`OutboxError::Unavailable`], which means nothing was sent.
    fn persist_asked(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
    ) -> Result<(), OutboxError> {
        self.persist(previous, next, cause, by).map_err(asked)
    }

    fn event(&self, cause: Cause, outcome: PhoneOutcome, initiator: Initiator) -> IntentEvent {
        IntentEvent {
            cause,
            outcome,
            initiator,
            observed_at_ms: self.clock.unix_milliseconds(),
        }
    }
}

fn retryable(record: &PhoneRecord) -> bool {
    matches!(
        record.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted | UnknownCause::LookupFailed)
    )
}

fn asked(error: OutboxError) -> OutboxError {
    match error {
        OutboxError::Unavailable | OutboxError::Uncertain => OutboxError::Unsaved,
        other => other,
    }
}

fn stopped_at_bound(before: &PhoneRecord, next: &PhoneRecord) -> bool {
    before.sends() >= FOREGROUND_SENDS
        && matches!(
            next.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        )
}
