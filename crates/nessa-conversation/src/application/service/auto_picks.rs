//! Auto-pick: an ask the agent allowed it on (`autoPick: allowed`, its
//! recommended choice easy to undo) that nobody answered within its owner's
//! wait is answered for them with that choice.
//!
//! The gateway owns the deadline, not a window: the wait counts from when the
//! agent asked, so it fires with every window closed and, after a restart, at
//! once for one that came due while the gateway was down. The answer is
//! recorded first, under the summary lock every answer is recorded under, so
//! exactly one of the owner and auto-pick answers an ask; then it is sent as
//! the owner's own message, from [`AUTO_PICK_SURFACE`] under the ask's own
//! action id ([`auto_pick_action`]), naming the question it answers, since a
//! newer ask may be waiting by the time the agent reads it. One that does not
//! reach the agent is taken back.
use super::{
    ConversationCaller, ConversationError, ConversationService, Initiation, Reach, SubmissionMode,
    SubmittedMessage, Writer,
};
use crate::application::WaitingAutoPick;
use crate::timer::clock;
use nessa_protocol::conversation::domain::ConversationSummary;
use nessa_protocol::conversation::domain::{ConversationId, SettledAsk};
use nessa_protocol::conversation::view::AUTO_PICK_SURFACE;
use std::time::Duration;

/// The longest auto-pick sleeps before it looks again, even with nothing
/// due: a backstop, since every change that could bring a pick nearer wakes
/// it.
const MOST_BETWEEN_LOOKS: Duration = Duration::from_secs(15 * 60);
/// How long it waits after the waiting decisions could not be read.
const READ_RETRY: Duration = Duration::from_secs(5);
/// The first wait after a pick could not be sent, doubled each time it
/// fails again, up to [`LATEST_RETRY_MS`].
const FIRST_RETRY_MS: u64 = 30_000;
const LATEST_RETRY_MS: u64 = 60 * 60 * 1000;

/// The action id auto-pick sends ask `ask_id`'s answer under: what makes a
/// retry the same message.
pub(crate) fn auto_pick_action(ask_id: &str) -> String {
    format!("auto-pick-{ask_id}")
}

impl ConversationService {
    /// Answer asks nobody answered in time, until the gateway retires.
    /// Started once by composition.
    pub async fn run_auto_picks(&self) {
        loop {
            if self.inner.retirement.get().is_some() {
                return;
            }
            // Registered before the look, so a change during it is not missed.
            let wakeup = self.inner.auto_pick_wakeup.notified();
            let wait = self.pick_due().await.min(MOST_BETWEEN_LOOKS);
            tokio::select! {
                () = wakeup => {}
                () = clock::sleep(wait) => {}
                () = self.retired() => return,
            }
        }
    }

    /// One look: send each pick that is due, and say how long until the next
    /// one is.
    pub(crate) async fn pick_due(&self) -> Duration {
        self.take_back_pending().await;
        let waiting = match self.inner.summaries.auto_picks_waiting().await {
            Ok(waiting) => waiting,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "the decisions waiting for auto-pick could not be read"
                );
                return READ_RETRY;
            }
        };
        let now = self.inner.clock.unix_milliseconds();
        // A failure kept for an ask that stopped waiting otherwise is let go.
        if let Ok(mut failures) = self.inner.auto_pick_failures.lock() {
            failures.retain(|ask, _| waiting.iter().any(|each| &each.ask.ask_id == ask));
        }
        let mut next = MOST_BETWEEN_LOOKS;
        for waiting in waiting {
            if !waiting.settings.auto_pick || !waiting.ask.auto_pick_allowed() {
                continue;
            }
            let due = waiting.ask.asked_at_ms.saturating_add(
                u64::from(waiting.settings.auto_pick_after_seconds).saturating_mul(1000),
            );
            let retry = self.auto_pick_retry_at(&waiting.ask.ask_id);
            let at = due.max(retry.unwrap_or(0));
            if at > now {
                next = next.min(Duration::from_millis(at - now));
                continue;
            }
            match self.auto_pick(&waiting).await {
                Ok(()) => self.auto_pick_settled(&waiting.ask.ask_id),
                Err(error) => {
                    let wait = self.auto_pick_failed(&waiting.ask.ask_id, now);
                    tracing::warn!(
                        conversation = %waiting.conversation,
                        ask = %waiting.ask.ask_id,
                        ?error,
                        retry_ms = wait,
                        "auto-pick could not send the recommended choice"
                    );
                    next = next.min(Duration::from_millis(wait));
                }
            }
        }
        next
    }

    /// Record `waiting`'s recommended choice as its answer, if it is still
    /// to be picked: still the ask waiting in its conversation, not paused,
    /// and due under its owner's settings as they are now. Asked and written
    /// under the summary lock the owner's answer, a newer ask and a pause
    /// are each written under, so from here an owner's message finds nothing
    /// waiting and is a message after the pick, a pause says it did not
    /// apply, and a newer ask does not replace this one. What was recorded
    /// comes back, to be sent and, should it not reach the agent, taken back.
    pub(super) async fn record_pick(
        &self,
        waiting: &WaitingAutoPick,
    ) -> Result<Option<RecordedPick>, ConversationError> {
        let ask_id = &waiting.ask.ask_id;
        let _writes = self
            .inner
            .records
            .summary_writes()
            .lock(&waiting.conversation)
            .await;
        let before = self.inner.summaries.load(&waiting.conversation).await?;
        if before
            .as_ref()
            .and_then(|summary| summary.ask())
            .map(|ask| &ask.ask_id)
            != Some(ask_id)
        {
            return Ok(None);
        }
        let paused = self
            .inner
            .summaries
            .decisions(&waiting.conversation)
            .await?
            .into_iter()
            .any(|decision| &decision.ask.ask_id == ask_id && decision.auto_pick_paused);
        if paused {
            return Ok(None);
        }
        let settings = self
            .inner
            .summaries
            .decision_settings(&waiting.organization, &waiting.owner)
            .await?;
        let now = self.inner.clock.unix_milliseconds();
        let due = waiting
            .ask
            .asked_at_ms
            .saturating_add(u64::from(settings.auto_pick_after_seconds).saturating_mul(1000));
        if !settings.auto_pick || due > now {
            return Ok(None);
        }
        let text = waiting
            .ask
            .auto_pick_text(settings.auto_pick_after_seconds)
            .ok_or(ConversationError::InvalidInput)?;
        let written = ConversationSummary::after_message(before.as_ref(), &text, None, now)
            .answering(Some(ask_id), &text, true);
        let settled = written.settled().cloned();
        self.inner
            .summaries
            .record(&waiting.conversation, written.clone())
            .await?;
        // From here a person's message in this conversation says so, for a
        // take-back not to undo what they said after it.
        let writing = self
            .inner
            .persons_writing
            .lock()
            .map_or(true, |writing| writing.contains_key(&waiting.conversation));
        if let Ok(mut flight) = self.inner.picks_in_flight.lock() {
            flight.insert(waiting.conversation.clone(), writing);
        }
        self.inner.live_changes.publish(&waiting.conversation);
        Ok(Some(RecordedPick {
            waiting: waiting.clone(),
            text,
            before,
            written,
            settled,
        }))
    }

    /// A person's message is on its way into `conversation`, until the
    /// guard drops: a pick recorded there meanwhile, and not yet sent, is no
    /// longer the last word, so taking it back leaves its ask waiting for
    /// them, not for another pick.
    pub(super) fn person_writing(&self, conversation: &ConversationId) -> PersonWriting<'_> {
        if let Ok(mut writing) = self.inner.persons_writing.lock() {
            *writing.entry(conversation.clone()).or_default() += 1;
        }
        if let Ok(mut flight) = self.inner.picks_in_flight.lock() {
            if let Some(after) = flight.get_mut(conversation) {
                *after = true;
            }
        }
        PersonWriting {
            service: self,
            conversation: conversation.clone(),
        }
    }

    /// `pick` never reached the agent: its ask waits again, unless
    /// something was asked since. The list goes back to how it was before
    /// the pick if nothing else changed it; if a person wrote since, the ask
    /// waits for them, its auto-pick paused, so the pick does not come after
    /// what they said. False when it could not be written, to be tried again.
    pub(super) async fn take_back_pick(&self, pick: &RecordedPick) -> bool {
        let conversation = &pick.waiting.conversation;
        let _writes = self.inner.records.summary_writes().lock(conversation).await;
        let current = match self.inner.summaries.load(conversation).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(%conversation, ?error, "an unsent auto-pick could not be taken back");
                return false;
            }
        };
        let person_wrote = self.inner.picks_in_flight.lock().map_or(true, |flight| {
            flight.get(conversation).copied().unwrap_or(false)
        });
        // Untouched since the pick: back to the list as it was.
        let untouched = current.as_ref().is_some_and(|current| {
            current.preview() == pick.written.preview()
                && current.updated_at_ms() == pick.written.updated_at_ms()
                && current.archived() == pick.written.archived()
                && current.title() == pick.written.title()
        });
        let base = if untouched {
            pick.before.as_ref()
        } else {
            current.as_ref()
        };
        let Some(next) = ConversationSummary::reopening(base, pick.waiting.ask.clone()) else {
            self.pick_done(conversation);
            return true;
        };
        if let Err(error) = self.inner.summaries.record(conversation, next).await {
            tracing::warn!(%conversation, ?error, "an unsent auto-pick could not be taken back");
            return false;
        }
        if person_wrote {
            let at = self.inner.clock.unix_milliseconds();
            if let Err(error) = self
                .inner
                .summaries
                .pause_auto_pick(conversation, &pick.waiting.ask.ask_id, Some(at))
                .await
            {
                // Tried again: reopening an ask already reopened changes nothing.
                tracing::warn!(%conversation, ?error, "a taken-back auto-pick could not be paused");
                return false;
            }
        }
        self.pick_done(conversation);
        self.inner.live_changes.publish(conversation);
        true
    }

    fn pick_done(&self, conversation: &ConversationId) {
        if let Ok(mut flight) = self.inner.picks_in_flight.lock() {
            flight.remove(conversation);
        }
    }

    /// Take back the picks an earlier take-back could not write.
    async fn take_back_pending(&self) {
        let pending: Vec<RecordedPick> = self
            .inner
            .auto_pick_takebacks
            .lock()
            .map(|pending| pending.values().cloned().collect())
            .unwrap_or_default();
        for pick in pending {
            if self.take_back_pick(&pick).await {
                if let Ok(mut pending) = self.inner.auto_pick_takebacks.lock() {
                    pending.remove(&pick.waiting.ask.ask_id);
                }
            }
        }
    }

    /// Answer `waiting` with its recommended choice, if it is still to be
    /// picked ([`Self::record_pick`]), and send that answer to the agent as
    /// its owner's message. One the agent does not have is taken back, and
    /// its ask waits again, to be tried later; one it may have stands.
    pub(crate) async fn auto_pick(
        &self,
        waiting: &WaitingAutoPick,
    ) -> Result<(), ConversationError> {
        let Some(pick) = self.record_pick(waiting).await? else {
            return Ok(());
        };
        let action = auto_pick_action(&waiting.ask.ask_id);
        let caller = ConversationCaller {
            organization_id: waiting.organization.clone(),
            principal_id: waiting.owner.clone(),
            surface_id: AUTO_PICK_SURFACE.to_owned(),
            action_id: action.clone(),
        };
        let sent = self
            .submit_as(
                waiting.conversation.clone(),
                caller,
                action,
                SubmittedMessage {
                    text: pick.text.clone(),
                    ..Default::default()
                },
                SubmissionMode::Queue,
                Writer::Person(Initiation::owner(), true),
            )
            .await;
        let failure = match sent {
            Ok(_) => None,
            Err(failure) if failure.reach() != Reach::NotTaken => {
                tracing::warn!(
                    ?failure,
                    "auto-pick may have reached the agent; its answer stands"
                );
                None
            }
            Err(failure) => Some(failure),
        };
        let Some(failure) = failure else {
            self.pick_done(&waiting.conversation);
            if let Some(settled) = &pick.settled {
                self.inner.decision_traces.settled(
                    &self.telemetry(),
                    &waiting.conversation,
                    settled,
                );
            }
            return Ok(());
        };
        if !self.take_back_pick(&pick).await {
            if let Ok(mut pending) = self.inner.auto_pick_takebacks.lock() {
                pending.insert(waiting.ask.ask_id.clone(), pick);
            }
        }
        Err(failure.into_error())
    }

    fn auto_pick_retry_at(&self, ask_id: &str) -> Option<u64> {
        self.inner
            .auto_pick_failures
            .lock()
            .ok()?
            .get(ask_id)
            .map(|(at, _)| *at)
    }

    /// Note a failed pick and return how long until it is tried again.
    fn auto_pick_failed(&self, ask_id: &str, now: u64) -> u64 {
        let Ok(mut failures) = self.inner.auto_pick_failures.lock() else {
            return FIRST_RETRY_MS;
        };
        let tries = failures.get(ask_id).map_or(0, |(_, tries)| *tries);
        let wait = FIRST_RETRY_MS
            .saturating_mul(1 << tries.min(16))
            .min(LATEST_RETRY_MS);
        failures.insert(ask_id.to_owned(), (now.saturating_add(wait), tries + 1));
        wait
    }

    fn auto_pick_settled(&self, ask_id: &str) {
        if let Ok(mut failures) = self.inner.auto_pick_failures.lock() {
            failures.remove(ask_id);
        }
    }
}

/// An answer auto-pick recorded, until it is sent or taken back.
#[derive(Clone, Debug)]
pub(crate) struct RecordedPick {
    waiting: WaitingAutoPick,
    text: String,
    /// The summary before the pick, to go back to.
    before: Option<ConversationSummary>,
    /// The summary the pick wrote.
    written: ConversationSummary,
    /// The decision it settled, traced once the answer stands.
    settled: Option<SettledAsk>,
}

/// A person's message on its way in ([`ConversationService::person_writing`]).
pub(super) struct PersonWriting<'a> {
    service: &'a ConversationService,
    conversation: ConversationId,
}

impl Drop for PersonWriting<'_> {
    fn drop(&mut self) {
        if let Ok(mut writing) = self.service.inner.persons_writing.lock() {
            if let Some(count) = writing.get_mut(&self.conversation) {
                *count -= 1;
                if *count == 0 {
                    writing.remove(&self.conversation);
                }
            }
        }
    }
}
