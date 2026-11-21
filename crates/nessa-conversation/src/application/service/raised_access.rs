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
use super::LiveConversation;
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

/// Allows once every review `live`'s agent still waits on, for `actor`'s raise
/// to Full under `request_id`. Best effort: a review answered or gone
/// meanwhile is stale and skipped, and nothing here fails the raise, which
/// the agent has already taken.
pub(super) async fn allow_waiting_reviews(
    id: &ConversationId,
    live: &LiveConversation,
    actor: &ActionContext,
    request_id: &str,
) -> usize {
    let Ok(snapshot) = ApprovalModeSnapshot::new(FULL_ACCESS, request_id) else {
        return 0;
    };
    let reviews = live.agent.pending_reviews().unwrap_or_default();
    let mut allowed = 0;
    for review in &reviews {
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
            Err(failure) => tracing::warn!(
                conversation_id = %id,
                permission_id = %review.id.as_str(),
                error = %failure.into_parts().0,
                "a review open when access was raised to full was not allowed",
            ),
        }
    }
    allowed
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
