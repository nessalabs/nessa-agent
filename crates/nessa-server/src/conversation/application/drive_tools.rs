//! A Drive grant's turn calls only the tools its grant's policy allows
//! (slice J4 of #707). `docs/design/read-grants.md` holds the rules and the
//! state table its tests are written from.
//!
//! A turn names the grant it was started under and the revision admitted
//! (`Initiation`); the grant journal says what that revision's policy was
//! ([`ShareGrants::policy_at`]). Admission reads it once, keeps it for the
//! conversation in [`AdmittedPolicies`], and saves the turn as checked. Every
//! tool call of that turn is then asked of [`ToolPolicyEvaluator`]:
//!
//! - a call the agent asks permission for, by [`DriveToolHook`] before a
//!   person is asked;
//! - the gateway's own `run`, `artifacts_publish` and `artifacts_fetch`, by
//!   the gateway before it runs them ([`decision`]). Each of these decisions,
//!   allowed or refused, is committed to [`DriveToolAudit`] first; a decision
//!   that could not be recorded refuses the call.
//!
//! ```text
//! send ─▶ admit: Claude or Codex, Ask mode, no native hooks, policy_at(rev)
//!           │                                     └─▶ AdmittedPolicies
//!           ▼
//! turn ─▶ review hook ─────────▶ refusal(initiation, call) ─▶ Cedar
//! turn ─▶ run / publish / fetch ─▶ decision(initiation, call) ─▶ Cedar
//!                                        └─▶ DriveToolAudit, then answer
//! ```
//!
//! Only a call the agent asks about is a call this checks. In Ask mode
//! Claude reads files and Codex runs commands inside its read-only, offline
//! sandbox without asking: no policy takes those away, and a deny pattern
//! naming a read tool does not stop them.
//!
//! The owner's turns are never checked here, and a missing or unreadable
//! policy refuses the call: nothing stands in for an answer the grant journal
//! could not give.

use super::share_grants::ShareGrants;
use super::tool_policy::{ToolDecision, ToolInitiator, ToolPolicyEvaluator};
use super::ConversationFuture;
use crate::conversation::domain::{ToolCall, ToolPolicy, ToolSource};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::hooks::{
    HookError, InvocationHook, InvocationObservationWindow, ProposedToolCall, ToolVerdict,
};
use nessa_sdk::domain::agent_execution::executions::{Initiation, Initiator};
use nessa_sdk::domain::agent_execution::permissions::RefusalOutcome;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

/// What checks a Drive grant's tools: the grant journal, the evaluator, and
/// where the gateway's own tool decisions are recorded. Composition binds it
/// once; a gateway that has not admits no Drive turn.
pub struct DriveTools {
    pub grants: Arc<dyn ShareGrants>,
    pub evaluator: Arc<dyn ToolPolicyEvaluator>,
    pub audit: Arc<dyn DriveToolAudit>,
}

/// Immutable evidence of one decision on a gateway tool call of a Drive
/// grant's turn, committed before the call is answered or run.
///
/// `allowed` with its `rules` is what the policy said; `reason` is why a
/// refused call was refused, which names an unavailable policy when there
/// were no rules to name. The sink assigns the observation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveToolAuditRecord {
    pub conversation_id: ConversationId,
    /// The turn the call was made in.
    pub turn: String,
    /// The tool call's identity.
    pub call: String,
    /// The person who started the turn under the grant, and from where.
    pub initiator_principal_id: String,
    pub initiator_surface_id: String,
    pub grant: String,
    pub revision: u64,
    pub tool: String,
    pub allowed: bool,
    pub rules: Vec<String>,
    pub reason: Option<String>,
}

/// Commits a gateway tool decision of a Drive turn. Records are idempotent by
/// conversation, call, tool and outcome: the same decision repeated is
/// acknowledged again, a contradicting one refused. An unavailable sink
/// refuses the call.
pub trait DriveToolAudit: Send + Sync {
    fn record(&self, record: DriveToolAuditRecord) -> ConversationFuture<'_, ()>;
}

/// What the policy of the Drive grant that started a turn said about one
/// call: whether it is allowed, the deciding policy ids, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DriveDecision {
    pub(crate) grant: String,
    pub(crate) revision: u64,
    pub(crate) rules: Vec<String>,
    /// `None` when the call is allowed.
    pub(crate) refusal: Option<String>,
}

/// The tool source of a conversation's agent, or `None` for an agent whose
/// calls are not all asked about: OpenCode runs the tools it allows without
/// asking, so a policy could judge only some of its calls.
pub(crate) fn checked_source(agent: AgentId) -> Option<ToolSource> {
    match agent {
        AgentId::Claude => Some(ToolSource::Claude),
        AgentId::Codex => Some(ToolSource::Codex),
        AgentId::Opencode => None,
    }
}

/// The most policies one conversation keeps for its admitted Drive turns.
/// A conversation holds at most eight Drive grants, and a policy is let go
/// of only once this many others were admitted since it last was; a turn
/// still needing it has its calls refused, never allowed.
pub(crate) const MAX_ADMITTED_POLICIES: usize = 64;

/// The tool policies of the Drive turns one conversation admitted, by grant
/// and revision. A revision's policy never changes, so a kept copy cannot
/// drift from the journal it was read from.
#[derive(Default)]
pub(crate) struct AdmittedPolicies {
    held: Mutex<VecDeque<(String, u64, ToolPolicy)>>,
}

impl AdmittedPolicies {
    /// Keep `policy` as `grant`'s at `revision`, as the one most recently
    /// admitted, letting go of the least recently admitted past
    /// [`MAX_ADMITTED_POLICIES`].
    pub(crate) fn admit(&self, grant: &str, revision: u64, policy: ToolPolicy) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.retain(|(kept, kept_revision, _)| !(kept == grant && *kept_revision == revision));
        if held.len() == MAX_ADMITTED_POLICIES {
            held.pop_front();
        }
        held.push_back((grant.to_owned(), revision, policy));
    }

    fn policy(&self, grant: &str, revision: u64) -> Option<ToolPolicy> {
        let held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.iter()
            .find(|(kept, kept_revision, _)| kept == grant && *kept_revision == revision)
            .map(|(_, _, policy)| policy.clone())
    }
}

/// What the policy of the Drive grant that started `initiation` says about
/// `call`, or `None` for the owner's turns, which are not checked. A grant's
/// call is refused when no Drive tools are bound, its policy was not kept,
/// the policy could not be evaluated, or the policy refuses it; the reason
/// names the grant, its revision, the tool and the deciding policy ids.
pub(crate) fn decision(
    drive: &OnceLock<DriveTools>,
    admitted: &AdmittedPolicies,
    initiation: &Initiation,
    call: ToolCall<'_>,
) -> Option<DriveDecision> {
    let Initiator::Grant(grant) = initiation.initiator() else {
        return None;
    };
    let (name, revision) = (grant.grant(), grant.policy_revision());
    let decided = |rules: Vec<String>, refusal: Option<String>| {
        Some(DriveDecision {
            grant: name.to_owned(),
            revision,
            rules,
            refusal,
        })
    };
    let unavailable = || {
        decided(
            Vec::new(),
            Some(format!(
                "the tool policy of Drive grant {name:?} at revision {revision} is unavailable, so {:?} is refused",
                call.name()
            )),
        )
    };
    let Some(drive) = drive.get() else {
        return unavailable();
    };
    let Some(policy) = admitted.policy(name, revision) else {
        return unavailable();
    };
    let initiator = ToolInitiator::Grant {
        grant: name,
        policy: &policy,
    };
    match drive.evaluator.evaluate(initiator, call) {
        Ok(ToolDecision::Allow { rules }) => decided(rules, None),
        Ok(ToolDecision::Refuse { rules }) => {
            let reason = format!(
                "Drive grant {name:?} at revision {revision} does not allow {:?} (rules: {})",
                call.name(),
                if rules.is_empty() {
                    "none allows it".to_owned()
                } else {
                    rules.join(", ")
                }
            );
            decided(rules, Some(reason))
        }
        Err(_) => unavailable(),
    }
}

/// Why `call` may not run for a turn started as `initiation`, or `None` when
/// it may: [`decision`]'s refusal.
pub(crate) fn refusal(
    drive: &OnceLock<DriveTools>,
    admitted: &AdmittedPolicies,
    initiation: &Initiation,
    call: ToolCall<'_>,
) -> Option<String> {
    decision(drive, admitted, initiation, call).and_then(|decided| decided.refusal)
}

/// Checks every call a conversation's agent asks permission for against the
/// tool policy of the Drive grant that started its turn, before a person is
/// asked. A refused call is answered as the hook and the turn goes on; the
/// refusal and its reason are saved with the review.
pub(crate) struct DriveToolHook {
    pub(crate) drive: Arc<OnceLock<DriveTools>>,
    pub(crate) admitted: Arc<AdmittedPolicies>,
    pub(crate) source: ToolSource,
}

impl InvocationHook for DriveToolHook {
    fn before_permission_review(
        &self,
        call: &ProposedToolCall<'_>,
        _: &InvocationObservationWindow<'_>,
    ) -> Result<ToolVerdict, HookError> {
        Ok(
            match refusal(
                &self.drive,
                &self.admitted,
                call.initiation(),
                ToolCall::new(self.source, call.name()),
            ) {
                None => ToolVerdict::Allow,
                Some(reason) => ToolVerdict::refuse(reason, RefusalOutcome::Continue),
            },
        )
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/drive_tools.rs"]
mod tests;
