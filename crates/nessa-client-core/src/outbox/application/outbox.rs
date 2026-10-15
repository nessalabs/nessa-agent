//! The phone outbox. A draft never claims it was submitted.
//!
//! The ordering is `docs/design/phone-outbox.md`. Departure commits before the
//! writer lease. The lease is held across `conversation.send` and the save of
//! that answer. A lost save after departure reloads the row.

use super::super::domain::{
    after_lookup, after_send, replaces, stored_evidence, Cause, Evidence, Initiator, IntentEvent,
    LookupAnswer, PhoneCommandId, PhoneIntent, PhoneOutcome, PhoneRecord, PhoneView, Step,
    UnknownCause, FOREGROUND_SENDS,
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
    /// A draft is marked departed, and that commit finishes, before the writer
    /// lease. The lease is then held across `conversation.send` and the save
    /// of the answer (`departure_is_durable_before_the_gateway_is_asked`).
    /// A second send waits and reloads
    /// (`a_lost_departure_does_not_send_again`). It sends the same id only
    /// when the stored row is still unknown. If the wait ends without the
    /// lease, the reloaded row is returned.
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
            Ok(()) => self.with_lease(id, |outbox| {
                outbox.foreground(departed, by, Cause::SendAnswered)
            }),
            Err(OutboxError::Conflict) => {
                let current = self.committed_record(record.intent().command())?;
                if !current.intent().same_bytes(record.intent()) {
                    return Err(OutboxError::Conflict);
                }
                self.with_lease(id, |outbox| outbox.follow(current, by))
            }
            Err(error) => Err(error),
        }
    }

    /// Look up every command that may have left and has no confirmed answer.
    ///
    /// This does not send and does not take the lease
    /// (`recovery_does_not_send_when_the_receipt_is_absent`). Its write waits
    /// if a send holds the lease, and then precedence applies. An attempted
    /// receipt is read again, so a later settled receipt is adopted
    /// (`recovery_adopts_a_settled_receipt_after_attempted`).
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
    /// A lookup that finds nothing takes the writer lease before
    /// `conversation.send`, so a second retry, or a retry that overlaps the
    /// first send, waits and reloads
    /// (`a_concurrent_retry_sends_the_departure_once`).
    ///
    /// The audit initiator is [`Initiator::Person`].
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
            return self
                .apply_lookup(record, answer, by, false)
                .map(|body| body.view);
        }
        let command = record.intent().command().clone();
        self.with_lease(&command, |outbox| outbox.send_after_not_found(record, by))
    }

    /// Every saved command, drafts included.
    pub fn list(&mut self) -> Result<Vec<PhoneView>, OutboxError> {
        Ok(self.store.list()?.iter().map(PhoneRecord::view).collect())
    }

    /// The lease is already held. Send only when the row is still the one
    /// the lookup authorized.
    fn send_after_not_found(
        &mut self,
        original: PhoneRecord,
        by: Initiator,
    ) -> Result<LeaseBody, OutboxError> {
        let record = self
            .store
            .load(original.intent().command())?
            .ok_or(OutboxError::NotHeld)?;
        if record != original {
            return Ok(kept_body(record.view()));
        }
        let looked = record
            .clone()
            .with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
        let marked = self.asked_or_kept(&record, &looked, Cause::Lookup, by, Evidence::Lookup)?;
        if marked.resave.is_none() {
            return Ok(marked);
        }
        let reset = looked.clone().with_sends(0);
        let marked =
            self.asked_or_kept(&looked, &reset, Cause::ExplicitRetry, by, Evidence::Lookup)?;
        if marked.resave.is_none() {
            return Ok(marked);
        }
        self.foreground(reset, by, Cause::ExplicitRetry)
    }

    /// Wait for the writer lease, run `body`, and commit it.
    ///
    /// The wait ending without the lease reloads the row. It does not report
    /// [`OutboxError::Unavailable`] (`a_lost_departure_does_not_send_again`).
    fn with_lease(
        &mut self,
        id: &PhoneCommandId,
        body: impl FnOnce(&mut Self) -> Result<LeaseBody, OutboxError>,
    ) -> Result<PhoneView, OutboxError> {
        if self.store.begin_exclusive().is_err() {
            return self.row_or_unsaved(id);
        }
        match body(self) {
            Ok(done) => match self.store.end_exclusive() {
                Ok(()) => Ok(done.view),
                Err(_) => self.replay(done),
            },
            Err(error) => {
                self.store.abort_exclusive();
                Err(error)
            }
        }
    }

    /// The other sender already departed this draft. The lease is held.
    fn follow(&mut self, fallen: PhoneRecord, by: Initiator) -> Result<LeaseBody, OutboxError> {
        let current = self
            .store
            .load(fallen.intent().command())?
            .ok_or(OutboxError::NotHeld)?;
        if !current.intent().same_bytes(fallen.intent()) {
            return Err(OutboxError::Conflict);
        }
        if matches!(
            current.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted | UnknownCause::LookupFailed)
        ) {
            self.foreground(current, by, Cause::SendAnswered)
        } else if open_outcome(&current) {
            self.reconcile_body(current, by, false)
        } else {
            Ok(kept_body(current.view()))
        }
    }

    fn reconcile(
        &mut self,
        record: PhoneRecord,
        by: Initiator,
        allow_send: bool,
    ) -> Result<PhoneView, OutboxError> {
        Ok(self.reconcile_body(record, by, allow_send)?.view)
    }

    fn reconcile_body(
        &mut self,
        record: PhoneRecord,
        by: Initiator,
        allow_send: bool,
    ) -> Result<LeaseBody, OutboxError> {
        if !open_outcome(&record) {
            return Ok(kept_body(record.view()));
        }
        let answer = self.gateway.lookup(record.intent());
        self.apply_lookup(record, answer, by, allow_send)
    }

    fn foreground(
        &mut self,
        mut record: PhoneRecord,
        by: Initiator,
        mut cause: Cause,
    ) -> Result<LeaseBody, OutboxError> {
        loop {
            if record.sends() >= FOREGROUND_SENDS {
                let next = record
                    .clone()
                    .with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
                tracing::warn!(
                    limit = "client.foreground_sends",
                    "product session hit an operational limit"
                );
                return self.asked_or_kept(&record, &next, Cause::Bound, by, Evidence::Lookup);
            }
            let answer = self.gateway.send(record.intent());
            match after_send(record.clone(), answer) {
                Step::Done(next) => {
                    return self.asked_or_kept(&record, &next, cause, by, Evidence::Direct)
                }
                Step::ReturnToDraft(next) => {
                    return self.return_to_draft(&record, next, by, Evidence::Direct)
                }
                Step::Lookup {
                    record: bumped,
                    resend,
                } => {
                    let lost = if resend {
                        Cause::SendLost
                    } else {
                        Cause::Unreadable
                    };
                    let marked =
                        self.asked_or_kept(&record, &bumped, lost, by, Evidence::Lookup)?;
                    if marked.resave.is_none() {
                        return Ok(marked);
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
                            return self.asked_or_kept(
                                &bumped,
                                &next,
                                Cause::Lookup,
                                by,
                                Evidence::Lookup,
                            );
                        }
                        Step::ReturnToDraft(next) => {
                            return self.return_to_draft(&bumped, next, by, Evidence::Lookup)
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
    ) -> Result<LeaseBody, OutboxError> {
        match after_lookup(record.clone(), answer, allow_send) {
            Step::Done(next) if next == record => Ok(kept_body(record.view())),
            Step::Done(next) => {
                self.asked_or_kept(&record, &next, Cause::Lookup, by, Evidence::Lookup)
            }
            Step::ReturnToDraft(next) => self.return_to_draft(&record, next, by, Evidence::Lookup),
            Step::Send(next) => {
                let marked =
                    self.asked_or_kept(&record, &next, Cause::Lookup, by, Evidence::Lookup)?;
                if marked.resave.is_none() {
                    return Ok(marked);
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
        evidence: Evidence,
    ) -> Result<LeaseBody, OutboxError> {
        if failed.successor().is_some() {
            return self.asked_or_kept(previous, &failed, Cause::ReturnedToDraft, by, evidence);
        }
        let intended = failed.clone();
        let (command, turn) = self.ids.mint()?;
        let draft = PhoneRecord::held(
            failed
                .intent()
                .again(command.clone(), turn)
                .map_err(OutboxError::Id)?,
        );
        let failed = failed.with_successor(command);
        let events = self.draft_events(failed.outcome(), by, evidence);
        let draft_event = self.event(Cause::Held, draft.outcome(), by);
        match self
            .store
            .replace_and_insert(previous, &failed, &draft, &events, draft_event)
        {
            Ok(()) => Ok(LeaseBody {
                view: failed.view(),
                resave: Some(Resave::Draft {
                    failed: intended,
                    evidence,
                    by,
                }),
            }),
            Err(StoreError::Conflict) => self.retry_return(intended, evidence, by),
            Err(error) => Err(asked(OutboxError::from(error))),
        }
    }

    /// The other connection committed while this `turn_running` answer was
    /// being saved. Precedence decides whether this refusal replaces it.
    fn retry_return(
        &mut self,
        failed: PhoneRecord,
        evidence: Evidence,
        by: Initiator,
    ) -> Result<LeaseBody, OutboxError> {
        for _ in 0..SAVE_ATTEMPTS {
            let saved = self.committed_record(failed.intent().command())?;
            if !saved.intent().same_bytes(failed.intent()) {
                return Err(OutboxError::Conflict);
            }
            let stored = self.evidence_of(saved.intent().command())?;
            if !replaces(failed.outcome(), evidence, saved.outcome(), stored) {
                return Ok(kept_body(saved.view()));
            }
            if saved.outcome() == failed.outcome() && saved.successor().is_some() {
                return Ok(kept_body(saved.view()));
            }
            let (command, turn) = self.ids.mint()?;
            let draft = PhoneRecord::held(
                failed
                    .intent()
                    .again(command.clone(), turn)
                    .map_err(OutboxError::Id)?,
            );
            let next = overlay(&saved, &failed).with_successor(command);
            let events = self.draft_events(next.outcome(), by, evidence);
            let draft_event = self.event(Cause::Held, draft.outcome(), by);
            match self
                .store
                .replace_and_insert(&saved, &next, &draft, &events, draft_event)
            {
                Ok(()) => {
                    return Ok(LeaseBody {
                        view: next.view(),
                        resave: Some(Resave::Draft {
                            failed: failed.clone(),
                            evidence,
                            by,
                        }),
                    })
                }
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(asked(OutboxError::from(error))),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn replay(&mut self, done: LeaseBody) -> Result<PhoneView, OutboxError> {
        match done.resave {
            None => {
                let id = done.view.intent().command().clone();
                self.row_or_unsaved(&id)
            }
            Some(Resave::Outcome {
                next,
                cause,
                evidence,
                by,
            }) => self
                .prefer(&next, cause, evidence, by)
                .map(|body| body.view),
            Some(Resave::Draft {
                failed,
                evidence,
                by,
            }) => self.replay_draft(failed, evidence, by),
        }
    }

    fn replay_draft(
        &mut self,
        failed: PhoneRecord,
        evidence: Evidence,
        by: Initiator,
    ) -> Result<PhoneView, OutboxError> {
        let saved = self.committed_record(failed.intent().command())?;
        if saved.outcome() == failed.outcome() && saved.successor().is_some() {
            return Ok(saved.view());
        }
        if !saved.intent().same_bytes(failed.intent()) {
            return Err(OutboxError::Conflict);
        }
        let stored = self.evidence_of(saved.intent().command())?;
        if !replaces(failed.outcome(), evidence, saved.outcome(), stored) {
            return Ok(saved.view());
        }
        self.return_to_draft(&saved, failed.clear_successor(), by, evidence)
            .map(|body| body.view)
    }

    fn draft_events(
        &self,
        outcome: PhoneOutcome,
        by: Initiator,
        evidence: Evidence,
    ) -> Vec<IntentEvent> {
        let returned = self.event(Cause::ReturnedToDraft, outcome, by);
        match evidence {
            Evidence::Lookup => vec![self.event(Cause::Lookup, outcome, by), returned],
            Evidence::Direct => vec![returned],
        }
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

    /// A lost compare-and-swap keeps the row precedence selects
    /// (`a_direct_answer_beats_a_lookup`,
    /// `a_lookup_does_not_replace_a_direct_answer`).
    fn asked_or_kept(
        &mut self,
        previous: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        by: Initiator,
        evidence: Evidence,
    ) -> Result<LeaseBody, OutboxError> {
        match self.persist(previous, next, cause, by) {
            Ok(()) => Ok(saved_body(next.clone(), cause, evidence, by)),
            Err(OutboxError::Conflict) => self.prefer(next, cause, evidence, by),
            Err(error) => Err(asked(error)),
        }
    }

    fn prefer(
        &mut self,
        next: &PhoneRecord,
        cause: Cause,
        evidence: Evidence,
        by: Initiator,
    ) -> Result<LeaseBody, OutboxError> {
        for _ in 0..SAVE_ATTEMPTS {
            let saved = self.committed_record(next.intent().command())?;
            if !saved.intent().same_bytes(next.intent()) {
                return Err(OutboxError::Conflict);
            }
            let stored = self.evidence_of(saved.intent().command())?;
            if !replaces(next.outcome(), evidence, saved.outcome(), stored) {
                return Ok(kept_body(saved.view()));
            }
            let stronger = overlay(&saved, next);
            match self.persist(&saved, &stronger, cause, by) {
                Ok(()) => return Ok(saved_body(stronger, cause, evidence, by)),
                Err(OutboxError::Conflict) => continue,
                Err(error) => return Err(asked(error)),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn evidence_of(&mut self, id: &PhoneCommandId) -> Result<Evidence, OutboxError> {
        Ok(stored_evidence(&self.store.events(id)?))
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

    fn row_or_unsaved(&mut self, id: &PhoneCommandId) -> Result<PhoneView, OutboxError> {
        match self.store.load(id) {
            Ok(Some(saved)) => Ok(saved.view()),
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

/// What the lease's transaction was going to make durable.
struct LeaseBody {
    view: PhoneView,
    resave: Option<Resave>,
}

enum Resave {
    Outcome {
        next: PhoneRecord,
        cause: Cause,
        evidence: Evidence,
        by: Initiator,
    },
    /// `failed` has no successor yet. A replay mints the draft again.
    Draft {
        failed: PhoneRecord,
        evidence: Evidence,
        by: Initiator,
    },
}

fn saved_body(next: PhoneRecord, cause: Cause, evidence: Evidence, by: Initiator) -> LeaseBody {
    LeaseBody {
        view: next.view(),
        resave: Some(Resave::Outcome {
            next,
            cause,
            evidence,
            by,
        }),
    }
}

fn kept_body(view: PhoneView) -> LeaseBody {
    LeaseBody { view, resave: None }
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
