//! The phone outbox. A draft never claims it was submitted.

use super::super::domain::{
    after_lookup, after_send, outranks, Cause, Initiator, IntentEvent, LookupAnswer,
    PhoneCommandId, PhoneIntent, PhoneOutcome, PhoneRecord, PhoneView, Step, UnknownCause,
    FOREGROUND_SENDS,
};
use super::ports::{CommandIds, IntentStore, OutboxError, PhoneGateway, StoreError};
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
                match self
                    .store
                    .insert(&record, self.event(Cause::Held, record.outcome(), by))
                {
                    Ok(()) => Ok(record.view()),
                    // Another holder saved this command between the load and
                    // the insert. The same bytes are the saved draft
                    // (`a_concurrent_hold_of_the_same_bytes_keeps_one_draft`).
                    Err(StoreError::Conflict) => {
                        match self.store.load(record.intent().command())? {
                            Some(existing) if existing.intent().same_bytes(record.intent()) => {
                                Ok(existing.view())
                            }
                            Some(_) => Err(OutboxError::Conflict),
                            None => Err(OutboxError::Unsaved),
                        }
                    }
                    Err(error) => Err(OutboxError::from(error)),
                }
            }
            Some(existing) if existing.intent().same_bytes(&intent) => Ok(existing.view()),
            Some(_) => Err(OutboxError::Conflict),
        }
    }

    /// Send a saved draft, or read the receipt of a command that already left.
    ///
    /// A draft is marked departed before `conversation.send`. A command that
    /// already left is only looked up: this call does not send it again.
    /// An accepted, running, done, failed, or unresolved command is returned
    /// as saved. An attempted command is read again and is not sent.
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
        match self.persist(&record, &departed, Cause::Departed, by) {
            Ok(()) => self.foreground(departed, by, Cause::SendAnswered),
            // The other sender already marked this draft departed. Do not
            // send it again (`a_lost_departure_does_not_send_again`).
            Err(OutboxError::Conflict) => {
                let current = self.committed_record(record.intent().command())?;
                if current.intent().same_bytes(record.intent()) {
                    self.reconcile(current, by, false)
                } else {
                    Err(OutboxError::Conflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Look up every command that may have left and has no confirmed answer.
    ///
    /// This does not send (`recovery_does_not_send_when_the_receipt_is_absent`).
    /// An attempted receipt is read again, so a later settled receipt is
    /// adopted (`recovery_adopts_a_settled_receipt_after_attempted`).
    /// The audit initiator is [`Initiator::Recovery`], which `hold`, `send`,
    /// and `retry` do not record.
    pub fn recover(&mut self) -> Result<Vec<PhoneView>, OutboxError> {
        let pending: Vec<PhoneRecord> = self
            .store
            .list()?
            .into_iter()
            .filter(|record| record.departed() && open_outcome(record))
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
        if let Some(kept) = self.asked_or_kept(&record, &looked, Cause::Lookup, by)? {
            return Ok(kept);
        }
        let reset = looked.clone().with_sends(0);
        if let Some(kept) = self.asked_or_kept(&looked, &reset, Cause::ExplicitRetry, by)? {
            return Ok(kept);
        }
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
        if !open_outcome(&record) {
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
                if let Some(kept) = self.asked_or_kept(&record, &next, Cause::Bound, by)? {
                    return Ok(kept);
                }
                tracing::warn!(
                    limit = "client.foreground_sends",
                    "product session hit an operational limit"
                );
                return Ok(next.view());
            }
            let answer = self.gateway.send(record.intent());
            match after_send(record.clone(), answer) {
                Step::Done(next) => return self.asked_view(&record, &next, cause, by),
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
                    if let Some(kept) = self.asked_or_kept(&record, &bumped, lost, by)? {
                        return Ok(kept);
                    }
                    let looked = self.gateway.lookup(bumped.intent());
                    match after_lookup(bumped.clone(), looked, resend) {
                        Step::Done(next) => {
                            if resend && stopped_at_bound(&bumped, &next) {
                                tracing::warn!(
                                    limit = "client.foreground_sends",
                                    "product session hit an operational limit"
                                );
                            }
                            return self.asked_view(&bumped, &next, Cause::Lookup, by);
                        }
                        Step::ReturnToDraft(next) => {
                            return self.return_to_draft(&bumped, next, by)
                        }
                        Step::Send(next) => {
                            // The resend is recorded with its answer, after
                            // `conversation.send`. A crash before that call
                            // leaves no ForegroundSend event
                            // (`a_lost_answer_retransmits_the_same_bytes_until_the_bound`).
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
            Step::Done(next) if next == record => Ok(record.view()),
            Step::Done(next) => self.asked_view(&record, &next, Cause::Lookup, by),
            Step::ReturnToDraft(next) => self.return_to_draft(&record, next, by),
            Step::Send(next) => {
                if let Some(kept) = self.asked_or_kept(&record, &next, Cause::Lookup, by)? {
                    return Ok(kept);
                }
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
            return self.asked_view(previous, &failed, Cause::ReturnedToDraft, by);
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
        match self
            .store
            .replace_and_insert(previous, &failed, &draft, failed_event, draft_event)
        {
            Ok(()) => Ok(failed.view()),
            Err(StoreError::Conflict) => self.retry_return(failed, draft, by),
            Err(error) => Err(asked(OutboxError::from(error))),
        }
    }

    /// The other connection committed while this `turn_running` answer was
    /// being saved. A weaker row is replaced. An equal or stronger row stands.
    fn retry_return(
        &mut self,
        failed: PhoneRecord,
        draft: PhoneRecord,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        for _ in 0..SAVE_ATTEMPTS {
            let saved = self.committed_record(failed.intent().command())?;
            if !saved.intent().same_bytes(failed.intent())
                || !outranks(failed.outcome(), saved.outcome())
            {
                return Ok(saved.view());
            }
            let next = overlay(&saved, &failed);
            let failed_event = self.event(Cause::ReturnedToDraft, next.outcome(), by);
            let draft_event = self.event(Cause::Held, draft.outcome(), by);
            match self
                .store
                .replace_and_insert(&saved, &next, &draft, failed_event, draft_event)
            {
                Ok(()) => return Ok(next.view()),
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(asked(OutboxError::from(error))),
            }
        }
        Err(OutboxError::Unsaved)
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
    /// as [`OutboxError::Unavailable`] or [`OutboxError::Conflict`]: the first
    /// means nothing was sent, and the second means the command id stores
    /// different bytes. A lost compare-and-swap keeps a committed row that is
    /// at least as strong (`a_reconciled_row_is_kept_when_the_send_save_loses`)
    /// and writes this answer over a weaker one
    /// (`a_queued_receipt_replaces_a_weaker_lookup_failure`).
    fn asked_or_kept(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
    ) -> Result<Option<PhoneView>, OutboxError> {
        match self.persist(previous, next, cause, by) {
            Ok(()) => Ok(None),
            Err(OutboxError::Conflict) => Ok(Some(self.prefer_stronger(next, cause, by)?)),
            Err(error) => Err(asked(error)),
        }
    }

    fn prefer_stronger(
        &mut self,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        for _ in 0..SAVE_ATTEMPTS {
            let saved = self.committed_record(next.intent().command())?;
            if !saved.intent().same_bytes(next.intent())
                || !outranks(next.outcome(), saved.outcome())
            {
                return Ok(saved.view());
            }
            let stronger = overlay(&saved, next);
            match self.persist(&saved, &stronger, cause, by) {
                Ok(()) => return Ok(stronger.view()),
                Err(OutboxError::Conflict) => continue,
                Err(error) => return Err(asked(error)),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn asked_view(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        Ok(self
            .asked_or_kept(previous, next, cause, by)?
            .unwrap_or_else(|| next.view()))
    }

    fn committed_record(&mut self, id: &PhoneCommandId) -> Result<PhoneRecord, OutboxError> {
        match self.store.load(id) {
            Ok(Some(saved)) => Ok(saved),
            Ok(None) | Err(StoreError::Unavailable | StoreError::Uncertain) => {
                Err(OutboxError::Unsaved)
            }
            Err(error) => Err(OutboxError::from(error)),
        }
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

/// A departed command whose receipt may still change. Attempted is included
/// so recovery can adopt a later settled receipt. It is not retried.
fn open_outcome(record: &PhoneRecord) -> bool {
    retryable(record)
        || matches!(
            record.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        )
}

fn asked(error: OutboxError) -> OutboxError {
    match error {
        OutboxError::Unavailable | OutboxError::Uncertain => OutboxError::Unsaved,
        other => other,
    }
}

/// How many times a lost compare-and-swap reloads the row before giving up.
const SAVE_ATTEMPTS: usize = 4;

/// `next`'s outcome and send count on the committed intent.
fn overlay(saved: &PhoneRecord, next: &PhoneRecord) -> PhoneRecord {
    let mut updated = saved
        .clone()
        .with_sends(next.sends().max(saved.sends()))
        .with_outcome(next.outcome());
    if let Some(successor) = next.successor() {
        updated = updated.with_successor(successor.clone());
    }
    updated
}

fn stopped_at_bound(before: &PhoneRecord, next: &PhoneRecord) -> bool {
    before.sends() >= FOREGROUND_SENDS
        && matches!(
            next.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        )
}
