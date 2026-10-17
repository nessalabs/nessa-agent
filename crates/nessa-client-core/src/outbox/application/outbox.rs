//! The phone outbox. A draft never claims it was submitted.
//!
//! The ordering is `docs/design/phone-outbox.md`. Departure commits before the
//! writer lease. The lease is held across `conversation.send` and the save of
//! that answer. A lost save after departure reloads the row.

use super::super::domain::{
    apply_lookup as commit_lookup, apply_retry, apply_send as commit_send, before_replaced_direct,
    budget, counted_return, counted_step, rebase_answer, replaces, retryable, stored_evidence,
    Applied, Budget, Cause, Evidence, Initiator, IntentEvent, LookupAnswer, PhoneCommandId,
    PhoneIntent, PhoneOutcome, PhoneRecord, PhoneView, UnknownCause,
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
    /// This outbox opened the writer lease and has not committed or dropped it.
    lease_open: bool,
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
            lease_open: false,
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
    /// The caller that committed departure reloads inside the lease and sends
    /// only when that row is still unknown and still inside the foreground
    /// budget (`the_departure_winner_reloads_before_it_sends`). A second send
    /// that also read Draft waits and reloads
    /// (`a_lost_departure_does_not_send_again`). It sends the same id only when
    /// the stored row is still unknown. If the wait ends without the lease,
    /// the reloaded row is returned. A caller first reading a departed row
    /// only reconciles its receipt and may return unknown while a send is
    /// pending (`a_sender_first_reading_departure_can_return_before_the_send`).
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
            Ok(()) => self.with_lease(id, |outbox| outbox.follow(departed, by)),
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
        let Some(reset) = apply_retry(record.clone()) else {
            return Ok(kept_body(record.view()));
        };
        let looked =
            self.asked_or_kept(&record, &reset.looked, Cause::Lookup, by, Evidence::Lookup)?;
        if looked.kept {
            return Ok(looked);
        }
        let reset_step = self.asked_or_kept(
            &reset.looked,
            &reset.reset,
            Cause::ExplicitRetry,
            by,
            Evidence::Lookup,
        )?;
        let chain = carry(looked, reset_step);
        if chain.kept {
            return Ok(chain);
        }
        let sent = self.foreground(reset.reset, by, Cause::ExplicitRetry)?;
        Ok(carry(chain, sent))
    }

    /// Wait for the writer lease, run `body`, and commit it.
    ///
    /// The wait ending without the lease reloads the row. It does not report
    /// [`OutboxError::Unavailable`]
    /// (`a_failed_lease_wait_returns_the_durable_departure`).
    fn with_lease(
        &mut self,
        id: &PhoneCommandId,
        body: impl FnOnce(&mut Self) -> Result<LeaseBody, OutboxError>,
    ) -> Result<PhoneView, OutboxError> {
        if self.open_lease().is_err() {
            return self.row_or_unsaved(id);
        }
        match body(self) {
            Ok(done) => match self.finish_lease() {
                Ok(()) => Ok(done.view),
                Err(_) => self.replay(done),
            },
            Err(error) => {
                self.release_lease();
                Err(error)
            }
        }
    }

    fn open_lease(&mut self) -> Result<(), StoreError> {
        self.store.begin_exclusive()?;
        self.lease_open = true;
        Ok(())
    }

    /// Commit the lease this outbox opened.
    ///
    /// [`StoreError::Uncertain`] means the store rolled that lease back. Any
    /// other failure drops what the store still holds.
    fn finish_lease(&mut self) -> Result<(), StoreError> {
        let result = self.store.end_exclusive();
        match &result {
            Ok(()) | Err(StoreError::Uncertain) => self.lease_open = false,
            Err(_) => self.release_lease(),
        }
        result
    }

    fn release_lease(&mut self) {
        if !self.lease_open {
            return;
        }
        self.store.abort_exclusive();
        self.lease_open = false;
    }

    /// The lease is held. Send only from the row committed under it.
    ///
    /// The departure winner and the compare-and-swap loser both enter here
    /// (`the_departure_winner_reloads_before_it_sends`,
    /// `a_lost_departure_does_not_send_again`).
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
        let mut chain = kept_body(record.view());
        loop {
            match budget(record.clone()) {
                Budget::Bound(next) => {
                    tracing::warn!(
                        limit = "client.foreground_sends",
                        "product session hit an operational limit"
                    );
                    let step =
                        self.asked_or_kept(&record, &next, Cause::Bound, by, Evidence::Lookup)?;
                    return Ok(carry(chain, step));
                }
                // Lookup failed at the budget writes nothing
                // (`a_spent_budget_keeps_lookup_failed`).
                Budget::Stop => return Ok(chain),
                Budget::Send => {}
            }
            let answer = self.gateway.send(record.intent());
            match commit_send(record.clone(), answer, cause) {
                Applied::Save {
                    record: next,
                    cause,
                    evidence,
                } => {
                    let step = self.asked_or_kept(&record, &next, cause, by, evidence)?;
                    return Ok(carry(chain, step));
                }
                Applied::Return {
                    record: next,
                    lookup: false,
                } => {
                    let step = self.return_to_draft(&record, next, by, Evidence::Direct, cause)?;
                    return Ok(carry(chain, step));
                }
                Applied::Mark {
                    record: bumped,
                    cause: lost,
                    resend,
                } => {
                    let marked =
                        self.asked_or_kept(&record, &bumped, lost, by, Evidence::Lookup)?;
                    chain = carry(chain, marked);
                    if chain.kept {
                        return Ok(chain);
                    }
                    let looked = self.gateway.lookup(bumped.intent());
                    match commit_lookup(bumped.clone(), looked, resend) {
                        Applied::Save {
                            record: next,
                            cause,
                            evidence,
                        } => {
                            if cause == Cause::Bound {
                                tracing::warn!(
                                    limit = "client.foreground_sends",
                                    "product session hit an operational limit"
                                );
                            }
                            let step = self.asked_or_kept(&bumped, &next, cause, by, evidence)?;
                            return Ok(carry(chain, step));
                        }
                        Applied::Return {
                            record: next,
                            lookup: true,
                        } => {
                            let step = self.return_to_draft(
                                &bumped,
                                next,
                                by,
                                Evidence::Lookup,
                                Cause::Lookup,
                            )?;
                            return Ok(carry(chain, step));
                        }
                        Applied::Resend { record: next } => {
                            // Not found under the budget writes Lookup only
                            // when that changes the outcome. The resend is
                            // recorded with its answer
                            // (`a_lost_answer_retransmits_the_same_bytes_until_the_bound`).
                            if next.outcome() != bumped.outcome() {
                                let marked = self.asked_or_kept(
                                    &bumped,
                                    &next,
                                    Cause::Lookup,
                                    by,
                                    Evidence::Lookup,
                                )?;
                                chain = carry(chain, marked);
                                if chain.kept {
                                    return Ok(chain);
                                }
                            }
                            record = next;
                            cause = Cause::ForegroundSend;
                        }
                        // An impossible receipt, or not-found after an
                        // unreadable answer, leaves the marked row.
                        Applied::Keep => return Ok(chain),
                        Applied::Mark { .. } | Applied::Return { .. } => {
                            return Err(OutboxError::Unsaved)
                        }
                    }
                }
                Applied::Keep | Applied::Resend { .. } | Applied::Return { .. } => {
                    return Err(OutboxError::Unsaved)
                }
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
        match commit_lookup(record.clone(), answer, allow_send) {
            Applied::Keep => Ok(kept_body(record.view())),
            Applied::Save {
                record: next,
                cause,
                evidence,
            } => self.asked_or_kept(&record, &next, cause, by, evidence),
            Applied::Return {
                record: next,
                lookup: true,
            } => self.return_to_draft(&record, next, by, Evidence::Lookup, Cause::Lookup),
            Applied::Resend { record: next } => {
                let mut chain = kept_body(record.view());
                if next.outcome() != record.outcome() {
                    let marked =
                        self.asked_or_kept(&record, &next, Cause::Lookup, by, Evidence::Lookup)?;
                    chain = carry(chain, marked);
                    if chain.kept {
                        return Ok(chain);
                    }
                }
                let sent = self.foreground(next, by, Cause::ForegroundSend)?;
                Ok(carry(chain, sent))
            }
            Applied::Mark { .. } | Applied::Return { lookup: false, .. } => {
                Err(OutboxError::Unsaved)
            }
        }
    }

    fn return_to_draft(
        &mut self,
        previous: &PhoneRecord,
        failed: PhoneRecord,
        by: Initiator,
        evidence: Evidence,
        observed: Cause,
    ) -> Result<LeaseBody, OutboxError> {
        if failed.successor().is_some() {
            return self.asked_or_kept(previous, &failed, Cause::ReturnedToDraft, by, evidence);
        }
        let intended = failed.clone();
        let (command, turn) = match self.ids.mint() {
            Ok(minted) => minted,
            Err(error) => {
                self.refuse_without_draft(
                    previous,
                    &failed,
                    by,
                    observation_cause(evidence, observed),
                    evidence,
                )?;
                return Err(error);
            }
        };
        let draft = PhoneRecord::held(
            failed
                .intent()
                .again(command.clone(), turn)
                .map_err(OutboxError::Id)?,
        );
        let failed = failed.with_successor(command.clone());
        let events = self.draft_events(failed.outcome(), by, evidence, &command);
        let draft_event = self.event(Cause::Held, draft.outcome(), by);
        match self
            .store
            .replace_and_insert(previous, &failed, &draft, &events, draft_event)
        {
            Ok(()) => Ok(draft_body(
                previous.clone(),
                failed.view(),
                intended,
                observed,
                evidence,
                by,
            )),
            Err(StoreError::Conflict) => self.retry_return(intended, evidence, by, observed),
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
        observed: Cause,
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
            let next = self.rebase(
                &saved,
                &failed,
                observation_cause(evidence, observed),
                evidence,
            )?;
            let (command, turn) = match self.ids.mint() {
                Ok(minted) => minted,
                Err(error) => {
                    self.refuse_without_draft(
                        &saved,
                        &next,
                        by,
                        observation_cause(evidence, observed),
                        evidence,
                    )?;
                    return Err(error);
                }
            };
            let draft = PhoneRecord::held(
                failed
                    .intent()
                    .again(command.clone(), turn)
                    .map_err(OutboxError::Id)?,
            );
            let next = next.with_successor(command.clone());
            let events = self.draft_events(next.outcome(), by, evidence, &command);
            let draft_event = self.event(Cause::Held, draft.outcome(), by);
            match self
                .store
                .replace_and_insert(&saved, &next, &draft, &events, draft_event)
            {
                Ok(()) => {
                    return Ok(draft_body(
                        saved,
                        next.view(),
                        failed.clone(),
                        observed,
                        evidence,
                        by,
                    ))
                }
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(asked(OutboxError::from(error))),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn replay(&mut self, done: LeaseBody) -> Result<PhoneView, OutboxError> {
        if done.written.is_empty() {
            let id = done.view.intent().command().clone();
            return self.row_or_unsaved(&id);
        }
        let steps = done.written;
        let mut view = done.view;
        let mut index = 0;
        while index < steps.len() {
            match &steps[index] {
                Written::Draft {
                    previous,
                    failed,
                    cause,
                    evidence,
                    by,
                } => {
                    view = self.replay_draft(
                        previous.clone(),
                        failed.clone(),
                        *cause,
                        *evidence,
                        *by,
                    )?;
                    index += 1;
                }
                Written::Answer {
                    previous,
                    next,
                    cause,
                    evidence,
                    by,
                } => {
                    let saved = self.asked_or_kept(previous, next, *cause, *by, *evidence)?;
                    if !saved.kept {
                        view = saved.view;
                        index += 1;
                        continue;
                    }
                    let id = next.intent().command().clone();
                    let stored = self.committed_record(&id)?;
                    let stored_evidence = self.evidence_of(&id)?;
                    if let Some(end) = winning_answer(&steps, index, &stored, stored_evidence) {
                        view = self.replay_counted(&steps[index..=end])?;
                        index = end + 1;
                    } else {
                        view = saved.view;
                        index += 1;
                    }
                }
            }
        }
        Ok(view)
    }

    /// The kept prefix and the later answer are one commit. The answer may be
    /// a returned-to-draft. Send counts are applied from the row before the
    /// direct suffix that answer replaces
    /// (`a_replay_writes_the_lost_send_before_the_stronger_answer`,
    /// `a_replay_rebases_send_counts_onto_the_committed_history`,
    /// `a_replay_counts_the_lost_send_before_turn_running`).
    fn replay_counted(&mut self, steps: &[Written]) -> Result<PhoneView, OutboxError> {
        let Some(id) = answer_command(steps.first()) else {
            return Err(OutboxError::Unsaved);
        };
        for _ in 0..SAVE_ATTEMPTS {
            if self.open_lease().is_err() {
                return Err(OutboxError::Unsaved);
            }
            let saved = match self.committed_record(&id) {
                Ok(row) => row,
                Err(error) => {
                    self.release_lease();
                    return Err(error);
                }
            };
            let Some(Written::Answer { next, .. }) = steps.first() else {
                self.release_lease();
                return Err(OutboxError::Unsaved);
            };
            if !saved.intent().same_bytes(next.intent()) {
                self.release_lease();
                return Err(OutboxError::Conflict);
            }
            let stored_evidence = match self.evidence_of(&id) {
                Ok(evidence) => evidence,
                Err(error) => {
                    self.release_lease();
                    return Err(error);
                }
            };
            let Some((winning, evidence)) = counted_winner(steps.last()) else {
                self.release_lease();
                return Err(OutboxError::Unsaved);
            };
            if !replaces(winning, evidence, saved.outcome(), stored_evidence) {
                self.release_lease();
                return Ok(saved.view());
            }
            match self.write_counted(&saved, steps) {
                Ok(view) => return Ok(view),
                Err(OutboxError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn write_counted(
        &mut self,
        saved: &PhoneRecord,
        steps: &[Written],
    ) -> Result<PhoneView, OutboxError> {
        let Some((winner, _)) = counted_winner(steps.last()) else {
            self.release_lease();
            return Err(OutboxError::Unsaved);
        };
        let events = match self.store.events(saved.intent().command()) {
            Ok(events) => events,
            Err(error) => {
                self.release_lease();
                return Err(OutboxError::from(error));
            }
        };
        let Some(base) = before_replaced_direct(saved, &events, winner) else {
            self.release_lease();
            return Err(OutboxError::Unsaved);
        };
        let mut previous = saved.clone();
        let mut cursor = base;
        let mut view = saved.view();
        let last = steps.len() - 1;
        for (offset, step) in steps.iter().enumerate() {
            match step {
                Written::Answer {
                    next, cause, by, ..
                } => {
                    let Some(record) = counted_step(&cursor, *cause, next.outcome()) else {
                        self.release_lease();
                        return Err(OutboxError::Unsaved);
                    };
                    if let Err(error) = self.persist(&previous, &record, *cause, *by) {
                        self.release_lease();
                        return Err(asked(error));
                    }
                    previous = record.clone();
                    cursor = record;
                    view = previous.view();
                }
                Written::Draft {
                    failed,
                    cause,
                    evidence,
                    by,
                    ..
                } if offset == last => {
                    let Some(record) = counted_return(&cursor, *evidence) else {
                        self.release_lease();
                        return Err(OutboxError::Unsaved);
                    };
                    if record.outcome() != failed.outcome() {
                        self.release_lease();
                        return Err(OutboxError::Unsaved);
                    }
                    match self.return_to_draft(
                        &previous,
                        record,
                        *by,
                        *evidence,
                        observation_cause(*evidence, *cause),
                    ) {
                        Ok(body) => view = body.view,
                        Err(error) => {
                            self.release_lease();
                            return Err(error);
                        }
                    }
                }
                Written::Draft { .. } => {
                    self.release_lease();
                    return Err(OutboxError::Unsaved);
                }
            }
        }
        self.finish_lease().map_err(|_| OutboxError::Unsaved)?;
        Ok(view)
    }

    fn replay_draft(
        &mut self,
        previous: PhoneRecord,
        failed: PhoneRecord,
        cause: Cause,
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
        let failed = if saved == previous {
            failed
        } else {
            self.rebase(
                &saved,
                &failed,
                observation_cause(evidence, cause),
                evidence,
            )?
        };
        self.return_to_draft(
            &saved,
            failed.clear_successor(),
            by,
            evidence,
            observation_cause(evidence, cause),
        )
        .map(|body| body.view)
    }

    fn draft_events(
        &self,
        outcome: PhoneOutcome,
        by: Initiator,
        evidence: Evidence,
        successor: &PhoneCommandId,
    ) -> Vec<IntentEvent> {
        let mut returned = self.event(Cause::ReturnedToDraft, outcome, by);
        returned.successor = Some(successor.clone());
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

    /// The refusal is durable before the id error leaves, when its commit lands.
    ///
    /// A send already holds the lease, so that lease commits with the refusal.
    /// A failed commit is [`OutboxError::Unsaved`]: the row was not written
    /// (`a_failed_lease_commit_after_turn_running_is_unsaved`). Recovery holds
    /// no lease. Its replace already committed, and the caller hears the id
    /// error (`a_recovery_turn_running_is_saved_when_the_draft_id_fails`).
    /// A competing save uses the same precedence as other answered commands
    /// (`a_recovery_mint_failure_reloads_after_a_competing_lookup`).
    fn refuse_without_draft(
        &mut self,
        previous: &PhoneRecord,
        failed: &PhoneRecord,
        by: Initiator,
        observed: Cause,
        evidence: Evidence,
    ) -> Result<(), OutboxError> {
        let failed = failed.clone().clear_successor();
        self.asked_or_kept(previous, &failed, observed, by, evidence)?;
        self.commit_open_lease()
    }

    fn commit_open_lease(&mut self) -> Result<(), OutboxError> {
        if !self.lease_open {
            return Ok(());
        }
        self.finish_lease()
            .map_err(|error| asked(OutboxError::from(error)))
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
            Ok(()) => Ok(saved_body(
                previous.clone(),
                next.clone(),
                cause,
                evidence,
                by,
            )),
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
            let stronger = self.rebase(&saved, next, cause, evidence)?;
            match self.persist(&saved, &stronger, cause, by) {
                Ok(()) => return Ok(saved_body(saved, stronger, cause, evidence, by)),
                Err(OutboxError::Conflict) => continue,
                Err(error) => return Err(asked(error)),
            }
        }
        Err(OutboxError::Unsaved)
    }

    fn rebase(
        &mut self,
        saved: &PhoneRecord,
        next: &PhoneRecord,
        cause: Cause,
        evidence: Evidence,
    ) -> Result<PhoneRecord, OutboxError> {
        let events = self
            .store
            .events(saved.intent().command())
            .map_err(|error| asked(OutboxError::from(error)))?;
        let record = rebase_answer(saved, &events, cause, next.outcome(), evidence)
            .ok_or(OutboxError::Unsaved)?;
        Ok(match next.successor() {
            Some(successor) => record.with_successor(successor.clone()),
            None => record,
        })
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
            successor: None,
        }
    }
}

/// What the lease's transaction was going to make durable.
///
/// `written` is every transition in that transaction, in order. An uncertain
/// commit rolls the transaction back and [`PhoneOutbox::replay`] writes each
/// one again (`an_uncertain_lease_rewrites_every_transition`).
struct LeaseBody {
    view: PhoneView,
    written: Vec<Written>,
    /// The last save did not append a transition.
    kept: bool,
}

enum Written {
    Answer {
        previous: PhoneRecord,
        next: PhoneRecord,
        cause: Cause,
        evidence: Evidence,
        by: Initiator,
    },
    /// `failed` has no successor yet. A replay mints the draft again.
    ///
    /// `cause` is the send or retry that observed `turn_running`. Replay uses
    /// it when the second mint fails
    /// (`a_replayed_foreground_send_keeps_its_cause_when_the_draft_id_fails`).
    Draft {
        previous: PhoneRecord,
        failed: PhoneRecord,
        cause: Cause,
        evidence: Evidence,
        by: Initiator,
    },
}

fn saved_body(
    previous: PhoneRecord,
    next: PhoneRecord,
    cause: Cause,
    evidence: Evidence,
    by: Initiator,
) -> LeaseBody {
    LeaseBody {
        view: next.view(),
        written: vec![Written::Answer {
            previous,
            next,
            cause,
            evidence,
            by,
        }],
        kept: false,
    }
}

fn draft_body(
    previous: PhoneRecord,
    view: PhoneView,
    failed: PhoneRecord,
    cause: Cause,
    evidence: Evidence,
    by: Initiator,
) -> LeaseBody {
    LeaseBody {
        view,
        written: vec![Written::Draft {
            previous,
            failed,
            cause,
            evidence,
            by,
        }],
        kept: false,
    }
}

fn kept_body(view: PhoneView) -> LeaseBody {
    LeaseBody {
        view,
        written: Vec::new(),
        kept: true,
    }
}

fn carry(mut earlier: LeaseBody, later: LeaseBody) -> LeaseBody {
    earlier.written.extend(later.written);
    LeaseBody {
        view: later.view,
        written: earlier.written,
        kept: later.kept,
    }
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

/// The cause a `turn_running` refusal keeps when its draft id was not minted.
fn observation_cause(evidence: Evidence, observed: Cause) -> Cause {
    match evidence {
        Evidence::Lookup => Cause::Lookup,
        Evidence::Direct => match observed {
            Cause::SendAnswered | Cause::ForegroundSend | Cause::ExplicitRetry => observed,
            Cause::Held
            | Cause::Departed
            | Cause::SendLost
            | Cause::Unreadable
            | Cause::Lookup
            | Cause::Bound
            | Cause::ReturnedToDraft => Cause::SendAnswered,
        },
    }
}

fn asked(error: OutboxError) -> OutboxError {
    match error {
        OutboxError::Unavailable | OutboxError::Uncertain => OutboxError::Unsaved,
        other => other,
    }
}

/// How many times a lost compare-and-swap reloads the row before giving up.
const SAVE_ATTEMPTS: usize = 4;

/// The first later step that outranks `stored`, including a returned-to-draft.
fn winning_answer(
    steps: &[Written],
    kept: usize,
    stored: &PhoneRecord,
    stored_evidence: Evidence,
) -> Option<usize> {
    for (offset, step) in steps.iter().enumerate().skip(kept + 1) {
        let Some((outcome, evidence)) = counted_winner(Some(step)) else {
            continue;
        };
        if replaces(outcome, evidence, stored.outcome(), stored_evidence) {
            return Some(offset);
        }
        if matches!(step, Written::Draft { .. }) {
            return None;
        }
    }
    None
}

fn counted_winner(step: Option<&Written>) -> Option<(PhoneOutcome, Evidence)> {
    match step {
        Some(Written::Answer { next, evidence, .. }) => Some((next.outcome(), *evidence)),
        Some(Written::Draft {
            failed, evidence, ..
        }) => Some((failed.outcome(), *evidence)),
        None => None,
    }
}

fn answer_command(step: Option<&Written>) -> Option<PhoneCommandId> {
    match step {
        Some(Written::Answer { next, .. }) => Some(next.intent().command().clone()),
        _ => None,
    }
}
