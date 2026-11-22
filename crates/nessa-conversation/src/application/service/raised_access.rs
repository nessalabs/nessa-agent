//! Raising a conversation to Full settles what it already asks.
//!
//! Full means the agent runs commands without asking. A review the agent
//! opened before the raise still waits on the person, and the turn waits on
//! it, so the raise would not reach the very call that made the person raise
//! it. The raise is the person's answer to what is waiting: once the agent has
//! taken Full, every review its running turn still waits on is allowed once,
//! and the answer is recorded as the mode's (`ApprovalBasis::Mode`, "full")
//! for the person who raised it. It is allowed once, never remembered: a call
//! the harness still asks for under Full (a write to its own settings, say)
//! asks again next time.
//!
//! ```text
//! setApprovalMode(full) ──▶ agent acknowledges Full ──▶ committed
//!   ──▶ for each review the agent still waits on (oldest first):
//!         its first allow option that is not remembered ──▶ answered
//!   ──▶ the turn carries on; its next calls are decided at Full
//! ```
//!
//! Only the agent's own reviews: an MCP app's review is the gateway's own
//! policy, which Full does not lift. Only a raise to Full: Auto still lets the
//! agent's classifier ask, so a review open under Ask stays the person's.
//!
//! An answer is audited like the person's own. One that fails — its audit
//! above all — fails the raise as the person's answer would fail, after the
//! other reviews are tried: the mode the agent took stays taken, but the
//! raise never reports success over an answer that could not be audited.
//! The reviews it could not answer are kept under its request id
//! ([`UnsettledRaise`]), so a replay of that same request (a lost reply, a
//! retry) tries them again and reports what still fails, rather than
//! answering the saved mode alone.
use super::LiveConversation;
use crate::application::ConversationError;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::permissions::{
    is_remember_option, ActionContext, ApprovalAttribution, ApprovalBasis, ApprovalModeSnapshot,
    PermissionAnswer,
};
use nessa_sdk::domain::agent_execution::permissions::{
    PendingReview, PermissionEffect, PermissionOption, PermissionOptionId,
};

/// The mode name an answer made by raising to Full is recorded under.
const FULL_ACCESS: &str = "full";

/// The option Full answers `review` with: its first allow for this request
/// alone, the card's Allow once — never a Nessa rule it would remember, nor a
/// choice the harness keeps past this request (an `allow_always`), whatever
/// order the harness lists them in. `None` when it offers no such allow, and
/// the review stays the person's.
pub(super) fn allowed_once(review: &PendingReview) -> Option<&PermissionOptionId> {
    once_allow(review.options.choices())
}

fn once_allow(choices: &[PermissionOption]) -> Option<&PermissionOptionId> {
    choices
        .iter()
        .find(|option| {
            option.decision().effect() == PermissionEffect::Allow
                && !option.kept_by_harness()
                && !is_remember_option(option.id().as_str())
        })
        .map(|option| option.id())
}

/// The review keys a raise to Full could not answer, under that raise's
/// request id: held with the live conversation, whose agent holds the
/// reviews, and replaced by each raise.
#[derive(Default)]
pub(super) struct UnsettledRaise(std::sync::Mutex<Option<(String, Vec<String>)>>);

impl UnsettledRaise {
    fn keep(&self, request_id: &str, keys: Vec<String>) {
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *held = (!keys.is_empty()).then(|| (request_id.to_owned(), keys));
    }
    fn take(&self, request_id: &str) -> Option<Vec<String>> {
        let mut held = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.as_ref().is_some_and(|(id, _)| id == request_id) {
            return held.take().map(|(_, keys)| keys);
        }
        None
    }
}

/// Allows once every review `live`'s agent still waits on, for `actor`'s raise
/// to Full under `request_id`, and says how many it allowed. A review
/// answered or gone meanwhile is stale and skipped.
///
/// # Errors
/// The first answer that failed otherwise (an audit failure, say), as
/// [`ConversationError::PermissionAnswer`], once every review was tried.
/// The reviews it failed are kept for a replay of `request_id`.
pub(super) async fn allow_waiting_reviews(
    id: &ConversationId,
    live: &LiveConversation,
    actor: &ActionContext,
    request_id: &str,
) -> Result<usize, ConversationError> {
    allow_reviews(id, live, actor, request_id, None).await
}

/// A replay of the raise under `request_id`: tries again the reviews it could
/// not answer, those still waiting, and reports as the raise did. Nothing
/// kept for it (all answered, or another raise since) is `Ok(0)`.
///
/// # Errors
/// As [`allow_waiting_reviews`].
pub(super) async fn replay_unsettled(
    id: &ConversationId,
    live: &LiveConversation,
    actor: &ActionContext,
    request_id: &str,
) -> Result<usize, ConversationError> {
    let Some(keys) = live.unsettled_raise.take(request_id) else {
        return Ok(0);
    };
    allow_reviews(id, live, actor, request_id, Some(&keys)).await
}

async fn allow_reviews(
    id: &ConversationId,
    live: &LiveConversation,
    actor: &ActionContext,
    request_id: &str,
    only: Option<&[String]>,
) -> Result<usize, ConversationError> {
    let snapshot = ApprovalModeSnapshot::new(FULL_ACCESS, request_id)
        .map_err(|_| ConversationError::InvalidInput)?;
    let reviews = live.agent.pending_reviews().unwrap_or_default();
    let mut allowed = 0;
    let mut failed = None;
    let mut unsettled = Vec::new();
    for review in reviews
        .iter()
        .filter(|review| only.is_none_or(|keys| keys.contains(&review.content.key())))
    {
        let Some(option_id) = allowed_once(review) else {
            continue;
        };
        let answer = live
            .agent
            .answer_permission(PermissionAnswer {
                key: Some(review.content.key()),
                attribution: ApprovalAttribution::new(
                    actor.clone(),
                    ApprovalBasis::Mode(snapshot.clone()),
                ),
                execution_id: review.execution_id.clone(),
                id: review.id.clone(),
                option_id: option_id.clone(),
            })
            .await;
        match answer {
            Ok(_) => allowed += 1,
            // Answered or gone meanwhile: nothing left to allow.
            Err(failure) if matches!(failure.error(), AgentError::StalePermission) => {}
            Err(failure) => {
                let (error, selection) = failure.into_parts();
                tracing::warn!(
                    conversation_id = %id,
                    permission_id = %review.id.as_str(),
                    error = %error,
                    "a review open when access was raised to full was not allowed",
                );
                failed.get_or_insert(ConversationError::PermissionAnswer { error, selection });
                unsettled.push(review.content.key());
            }
        }
    }
    live.unsettled_raise.keep(request_id, unsettled);
    failed.map_or(Ok(allowed), Err)
}

#[cfg(test)]
mod tests {
    use super::once_allow;
    use nessa_sdk::application::agent_execution::permissions::REMEMBER_ALLOW;
    use nessa_sdk::domain::agent_execution::permissions::{
        PermissionDecision, PermissionEffect, PermissionOption, PermissionOptionId, PermissionScope,
    };

    fn option(id: &str, effect: PermissionEffect) -> PermissionOption {
        PermissionOption::new(
            PermissionOptionId::new(id).unwrap(),
            id,
            PermissionDecision::new(effect, PermissionScope::request()),
        )
        .unwrap()
    }

    #[test]
    fn full_answers_with_the_first_allow_nothing_remembers() {
        let choices = [
            option("deny", PermissionEffect::Deny),
            option("always", PermissionEffect::Allow).kept_by_the_harness(),
            option(REMEMBER_ALLOW, PermissionEffect::Allow),
            option("once", PermissionEffect::Allow),
        ];
        assert_eq!(once_allow(&choices).map(|id| id.as_str()), Some("once"));
    }

    #[test]
    fn a_review_offering_only_remembered_allows_stays_the_persons() {
        let choices = [
            option("always", PermissionEffect::Allow).kept_by_the_harness(),
            option(REMEMBER_ALLOW, PermissionEffect::Allow),
            option("deny", PermissionEffect::Deny),
        ];
        assert_eq!(once_allow(&choices), None);
    }
}
