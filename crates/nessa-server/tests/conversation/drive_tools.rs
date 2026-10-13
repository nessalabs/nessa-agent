use super::*;
use crate::conversation::application::{
    ConversationError, ConversationFuture, HeldGrant, ShareGrant, ShareGrantChange,
    ToolPolicyUnavailable,
};
use crate::conversation::domain::ToolPreset;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::executions::GrantInitiator;

/// Refuses what its list names, with that name as the rule; fails on
/// `broken`; answers Unavailable for any other policy than the one kept.
struct Listed {
    refused: &'static [&'static str],
}

impl ToolPolicyEvaluator for Listed {
    fn evaluate(
        &self,
        initiator: ToolInitiator<'_>,
        call: ToolCall<'_>,
    ) -> Result<ToolDecision, ToolPolicyUnavailable> {
        let ToolInitiator::Grant { policy, .. } = initiator else {
            panic!("the owner is never asked about");
        };
        if *policy != ToolPolicy::Preset(ToolPreset::ReadOnlyTools) || call.name() == "broken" {
            return Err(ToolPolicyUnavailable);
        }
        Ok(if self.refused.contains(&call.name()) {
            ToolDecision::Refuse {
                rules: vec![format!("deny.{}", call.name())],
            }
        } else if call.name() == "unlisted" {
            ToolDecision::Refuse { rules: Vec::new() }
        } else {
            ToolDecision::Allow {
                rules: vec!["allow".into()],
            }
        })
    }
}

/// A journal that is never read: the evaluator here decides from the kept
/// policy alone.
struct NoJournal;

impl ShareGrants for NoJournal {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn change(&self, _: ShareGrantChange) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn grants<'a>(&'a self, _: &'a ConversationId) -> ConversationFuture<'a, Vec<ShareGrant>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> ConversationFuture<'a, Option<ToolPolicy>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
}

fn bound() -> OnceLock<DriveTools> {
    let drive = OnceLock::new();
    let _ = drive.set(DriveTools {
        grants: Arc::new(NoJournal),
        evaluator: Arc::new(Listed {
            refused: &["Write"],
        }),
    });
    drive
}

fn grant(revision: u64) -> Initiation {
    Initiation::checked_grant(GrantInitiator::new("phone", revision).unwrap())
}

fn read_only() -> ToolPolicy {
    ToolPolicy::Preset(ToolPreset::ReadOnlyTools)
}

fn call(name: &str) -> ToolCall<'_> {
    ToolCall::new(ToolSource::Claude, name)
}

/// Row J27: the owner's turns are not checked, whatever is bound or kept.
#[test]
fn the_owners_turn_is_never_refused() {
    let admitted = AdmittedPolicies::default();
    for drive in [OnceLock::new(), bound()] {
        assert_eq!(
            refusal(&drive, &admitted, &Initiation::owner(), call("Write")),
            None
        );
    }
}

/// Row J27: a grant's call is decided by its policy at the turn's
/// revision; a refusal names the grant, the revision, the tool and the
/// deciding rules, or that none allowed it.
#[test]
fn a_grants_call_is_decided_by_its_policy_at_the_turns_revision() {
    let drive = bound();
    let admitted = AdmittedPolicies::default();
    admitted.admit("phone", 3, read_only());
    assert_eq!(refusal(&drive, &admitted, &grant(3), call("Read")), None);
    assert_eq!(
        refusal(&drive, &admitted, &grant(3), call("Write")).as_deref(),
        Some(r#"Drive grant "phone" at revision 3 does not allow "Write" (rules: deny.Write)"#)
    );
    assert_eq!(
        refusal(&drive, &admitted, &grant(3), call("unlisted")).as_deref(),
        Some(
            r#"Drive grant "phone" at revision 3 does not allow "unlisted" (rules: none allows it)"#
        )
    );
    // Not the other revision's policy, and not another grant's.
    let unavailable = |revision| {
        format!(
            r#"the tool policy of Drive grant "phone" at revision {revision} is unavailable, so "Read" is refused"#
        )
    };
    assert_eq!(
        refusal(&drive, &admitted, &grant(4), call("Read")),
        Some(unavailable(4))
    );
    let other = Initiation::checked_grant(GrantInitiator::new("laptop", 3).unwrap());
    assert!(refusal(&drive, &admitted, &other, call("Read")).is_some());
}

/// Row J27: a grant's call is refused when nothing checks it, when its
/// policy was not kept, or when the policy cannot be evaluated.
#[test]
fn a_grants_call_nothing_can_check_is_refused() {
    let admitted = AdmittedPolicies::default();
    admitted.admit("phone", 3, read_only());
    assert!(refusal(&OnceLock::new(), &admitted, &grant(3), call("Read")).is_some());
    let drive = bound();
    assert!(refusal(&drive, &admitted, &grant(3), call("broken")).is_some());
    admitted.admit("phone", 5, ToolPolicy::Preset(ToolPreset::All));
    assert!(refusal(&drive, &admitted, &grant(5), call("Read"))
        .is_some_and(|reason| reason.contains("unavailable")));
}

/// A conversation keeps at most its bound of policies, letting go of the
/// least recently admitted; one admitted again is kept once, as the newest.
#[test]
fn admitted_policies_keep_at_most_their_bound() {
    let drive = bound();
    let admitted = AdmittedPolicies::default();
    let bound = MAX_ADMITTED_POLICIES as u64;
    for revision in 0..bound {
        admitted.admit("phone", revision, read_only());
    }
    // Admitted again, revision 0 is now the newest and 1 the oldest.
    admitted.admit("phone", 0, read_only());
    admitted.admit("phone", bound, read_only());
    assert!(refusal(&drive, &admitted, &grant(1), call("Read")).is_some());
    for revision in [0, 2, bound] {
        assert_eq!(
            refusal(&drive, &admitted, &grant(revision), call("Read")),
            None
        );
    }
}

/// OpenCode runs the tools it allows without asking, so it has no checked
/// source and admits no Drive turn.
#[test]
fn only_agents_that_ask_have_a_checked_source() {
    assert_eq!(checked_source(AgentId::Claude), Some(ToolSource::Claude));
    assert_eq!(checked_source(AgentId::Codex), Some(ToolSource::Codex));
    assert_eq!(checked_source(AgentId::Opencode), None);
}
