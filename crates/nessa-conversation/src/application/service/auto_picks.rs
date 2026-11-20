//! Auto-pick: an ask the agent allowed it on (`autoPick: allowed`, its
//! recommended choice easy to undo) that nobody answered within its owner's
//! wait is answered for them with that choice.
//!
//! The gateway owns the deadline, not a window: the wait counts from when the
//! agent asked, so it fires with every window closed and, after a restart, at
//! once for one that came due while the gateway was down. The answer is the
//! owner's own message, sent from [`AUTO_PICK_SURFACE`] under the ask's own
//! action id ([`auto_pick_action`]), so it settles that ask and no other, and
//! the decision it settles says it was auto-picked.
use super::{
    ConversationCaller, ConversationError, ConversationService, SubmissionMode, SubmittedMessage,
};
use crate::application::WaitingAutoPick;
use crate::timer::clock;
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

/// The action id auto-pick sends ask `ask_id`'s answer under: what lets the
/// message settle that ask only, and lets a retry be the same message.
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

    /// Whether `waiting` is still to be picked now: still the ask waiting in
    /// its conversation, not paused, and due under its owner's settings as
    /// they are now. What `pick_due` read may be stale by the time a slow
    /// send before it is done, so this is asked again under the summary lock
    /// an answer, a newer ask and a pause are each written under. On yes the
    /// ask is marked sending until [`Sending`] drops, and a pause in that
    /// time is refused, since the answer goes anyway.
    async fn claim(
        &self,
        waiting: &WaitingAutoPick,
    ) -> Result<Option<Sending<'_>>, ConversationError> {
        let ask_id = &waiting.ask.ask_id;
        let _writes = self
            .inner
            .records
            .summary_writes()
            .lock(&waiting.conversation)
            .await;
        let still = self
            .inner
            .summaries
            .decisions(&waiting.conversation)
            .await?
            .into_iter()
            .any(|decision| {
                &decision.ask.ask_id == ask_id
                    && decision.outcome.is_none()
                    && !decision.auto_pick_paused
            });
        if !still {
            return Ok(None);
        }
        let settings = self
            .inner
            .summaries
            .decision_settings(&waiting.organization, &waiting.owner)
            .await?;
        let due = waiting
            .ask
            .asked_at_ms
            .saturating_add(u64::from(settings.auto_pick_after_seconds).saturating_mul(1000));
        if !settings.auto_pick || due > self.inner.clock.unix_milliseconds() {
            return Ok(None);
        }
        let Ok(mut sending) = self.inner.auto_picks_sending.lock() else {
            return Ok(None);
        };
        sending.insert(ask_id.clone());
        Ok(Some(Sending {
            service: self,
            ask_id: ask_id.clone(),
        }))
    }

    /// Whether auto-pick is sending the answer to `ask_id` now.
    pub(super) fn auto_pick_sending(&self, ask_id: &str) -> bool {
        self.inner
            .auto_picks_sending
            .lock()
            .map_or(true, |sending| sending.contains(ask_id))
    }

    /// Send `waiting`'s recommended choice as its owner's answer, if it is
    /// still to be picked ([`Self::claim`]); nothing to send is done.
    pub(crate) async fn auto_pick(
        &self,
        waiting: &WaitingAutoPick,
    ) -> Result<(), ConversationError> {
        let Some(_sending) = self.claim(waiting).await? else {
            return Ok(());
        };
        let text = waiting
            .ask
            .auto_pick_text(waiting.settings.auto_pick_after_seconds)
            .ok_or(ConversationError::InvalidInput)?;
        let action = auto_pick_action(&waiting.ask.ask_id);
        let caller = ConversationCaller {
            organization_id: waiting.organization.clone(),
            principal_id: waiting.owner.clone(),
            surface_id: AUTO_PICK_SURFACE.to_owned(),
            action_id: action.clone(),
        };
        self.submit(
            waiting.conversation.clone(),
            caller,
            action,
            SubmittedMessage {
                text,
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .map(|_| ())
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

/// An ask auto-pick is sending the answer to, until dropped.
struct Sending<'a> {
    service: &'a ConversationService,
    ask_id: String,
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        if let Ok(mut sending) = self.service.inner.auto_picks_sending.lock() {
            sending.remove(&self.ask_id);
        }
    }
}
