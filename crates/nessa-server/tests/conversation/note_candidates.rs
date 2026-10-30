//! Row J23: the host's verdict on the notes waiting for a turn, from each
//! author's grant as it stands, read and asked of the access policy once
//! per grant.

use super::*;
use crate::conversation::domain::ConversationDeletion;
use crate::conversation_test_support::{access_policy_ref, MemoryRepository, ObservedPolicy};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::sessions::{NoteDelivery, NoteId, NoteText};
use nessa_sdk::domain::agent_execution::executions::{AgentSource, ExecutionId, GrantInitiator};
use std::sync::Mutex;

/// Grants answered from a script, counting each read.
struct Scripted(Mutex<Vec<String>>);
impl ShareGrants for Scripted {
    fn held_grant<'a>(
        &'a self,
        _: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>> {
        self.0.lock().unwrap().push(receiver_id.to_owned());
        let held = |role| {
            Ok(Some(HeldGrant {
                capabilities: role,
                policy_revision: 9,
            }))
        };
        let answer = match receiver_id {
            "commenter" => held(Capabilities::comment()),
            "driver" => held(Capabilities::drive()),
            "reader" => held(Capabilities::read()),
            "unreadable" => Err(ConversationError::Unavailable),
            _ => Ok(None),
        };
        Box::pin(async move { answer })
    }
    fn change(
        &self,
        _: ShareGrantChange,
    ) -> ConversationFuture<'_, crate::conversation::application::ShareGrantReceipt> {
        unreachable!("a verdict changes no grant")
    }
    fn grants<'a>(&'a self, _: &'a ConversationId) -> ConversationFuture<'a, ShareGrantList> {
        unreachable!("a verdict lists no grants")
    }
    fn policy_at<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> ConversationFuture<'a, Option<crate::conversation::domain::ToolPolicy>> {
        unreachable!("a verdict reads no policy")
    }
    fn hold_revision<'a>(
        &'a self,
        _: &'a ConversationId,
        _: &'a str,
        _: u64,
    ) -> ConversationFuture<'a, RevisionHold> {
        unreachable!("a verdict holds no revision")
    }
}

fn note(id: &str, author: Initiator) -> NoteRecord {
    NoteRecord {
        id: NoteId::new(id).unwrap(),
        text: NoteText::new("x").unwrap(),
        delivery: NoteDelivery::NextTurn,
        actor: ActionContext::new("person", "phone", id).unwrap(),
        author,
        accepted_at_ms: 0,
        after_execution: None,
        retired: None,
    }
}
fn by(receiver: &str, revision: u64) -> Initiator {
    Initiator::Grant(GrantInitiator::new(receiver, revision).unwrap())
}

/// The owner's conversation the notes wait in, deleted or not.
fn conversation(deleted: bool) -> Conversation {
    let (organization, owner) = (
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
    );
    let live = Conversation::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization.clone(),
        owner.clone(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    if !deleted {
        return live;
    }
    live.deleted(
        ConversationDeletion::new(organization, owner, "panel".into(), "delete".into(), 2).unwrap(),
    )
    .unwrap()
}

/// A store holding `conversation`.
fn stored(conversation: Conversation) -> MemoryRepository {
    let repository = MemoryRepository::default();
    repository
        .records
        .lock()
        .unwrap()
        .insert(conversation.id().clone(), conversation);
    repository
}

fn id() -> ConversationId {
    conversation(false).id().clone()
}

fn ids(values: &[&str]) -> Vec<NoteId> {
    values.iter().map(|id| NoteId::new(*id).unwrap()).collect()
}

#[tokio::test]
async fn a_note_is_allowed_while_its_author_holds_comment_and_revoked_once_it_does_not() {
    let grants = Scripted(Mutex::new(Vec::new()));
    let notes = vec![
        note("owner", Initiator::Owner),
        note(
            "agent",
            Initiator::Agent(
                AgentSource::new(
                    "00000000-0000-4000-8000-000000000002",
                    ExecutionId::new("source-turn").unwrap(),
                )
                .unwrap(),
            ),
        ),
        // Left under another revision than the grant holds now: still the
        // same grant, and a policy change keeps its note.
        note("comment", by("commenter", 1)),
        note("drive", by("driver", 1)),
        note("read", by("reader", 1)),
        note("gone", by("revoked", 1)),
        note("unknown", by("unreadable", 1)),
        note("comment-again", by("commenter", 2)),
    ];
    let candidates = note_candidates(
        access_policy_ref(),
        &stored(conversation(false)),
        &grants,
        &id(),
        &notes,
    )
    .await;
    assert_eq!(
        candidates.authorized,
        ids(&["owner", "agent", "comment", "drive", "comment-again"])
    );
    // An unreadable grant is neither: its note waits.
    assert_eq!(candidates.revoked, ids(&["read", "gone"]));
    assert_eq!(
        grants.0.lock().unwrap().as_slice(),
        ["commenter", "driver", "reader", "revoked", "unreadable"]
    );
}

/// A deleted conversation delivers no grant's note: each is revoked, an
/// unreadable grant's still waits, and the owner's is allowed.
#[tokio::test]
async fn a_deleted_conversation_delivers_no_grants_note() {
    let grants = Scripted(Mutex::new(Vec::new()));
    let notes = vec![
        note("owner", Initiator::Owner),
        note("comment", by("commenter", 1)),
        note("drive", by("driver", 1)),
        note("unknown", by("unreadable", 1)),
    ];
    let candidates = note_candidates(
        access_policy_ref(),
        &stored(conversation(true)),
        &grants,
        &id(),
        &notes,
    )
    .await;
    assert_eq!(candidates.authorized, ids(&["owner"]));
    assert_eq!(candidates.revoked, ids(&["comment", "drive"]));
}

/// A conversation that cannot be read, or is no longer stored, judges no
/// grant's note: each waits, none is revoked, and the owner's is allowed.
/// Neither is a refusal by the policy.
#[tokio::test]
async fn a_grants_note_waits_while_its_conversation_cannot_be_read() {
    let notes = vec![
        note("owner", Initiator::Owner),
        note("comment", by("commenter", 1)),
        note("read", by("reader", 1)),
    ];
    // A row that answers for another id fails `load_conversation`'s check.
    let unreadable = stored(conversation(false));
    let other = Conversation::new(
        ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    *unreadable.mode_loaded_reply.lock().unwrap() = Some(other);
    for (repository, case) in [
        (unreadable, "unreadable"),
        (MemoryRepository::default(), "missing"),
    ] {
        let grants = Scripted(Mutex::new(Vec::new()));
        let candidates =
            note_candidates(access_policy_ref(), &repository, &grants, &id(), &notes).await;
        assert_eq!(candidates.authorized, ids(&["owner"]), "{case}");
        assert!(candidates.revoked.is_empty(), "{case}");
        assert!(grants.0.lock().unwrap().is_empty(), "{case}: no grant read");
    }
}

/// A policy that cannot decide `note.deliver` leaves a grant's note
/// waiting, never revoked; the owner's is still allowed.
#[tokio::test]
async fn a_grants_note_waits_while_the_policy_cannot_decide() {
    let policy = ObservedPolicy::default();
    policy.undecide();
    let grants = Scripted(Mutex::new(Vec::new()));
    let notes = vec![
        note("owner", Initiator::Owner),
        note("comment", by("commenter", 1)),
        note("read", by("reader", 1)),
    ];
    let candidates = note_candidates(
        &policy,
        &stored(conversation(false)),
        &grants,
        &id(),
        &notes,
    )
    .await;
    assert_eq!(candidates.authorized, ids(&["owner"]));
    assert!(candidates.revoked.is_empty());
    assert!(policy.asked() > 0);
}

/// The facts a paired party is asked with: a grant store that cannot be
/// read is unavailable, never a grant not held; a missing conversation is
/// not found.
#[tokio::test]
async fn paired_facts_are_unavailable_while_the_grant_store_fails() {
    let grants = Scripted(Mutex::new(Vec::new()));
    let repository = stored(conversation(false));
    assert_eq!(
        paired_facts(&repository, &grants, "unreadable", &id())
            .await
            .map(|(_, held)| held),
        Err(PairedRefusal::Unavailable)
    );
    let (found, held) = paired_facts(&repository, &grants, "commenter", &id())
        .await
        .unwrap();
    assert_eq!(found.id(), &id());
    assert_eq!(
        held.map(|held| held.capabilities),
        Some(Capabilities::comment())
    );
    assert_eq!(
        paired_facts(&MemoryRepository::default(), &grants, "commenter", &id())
            .await
            .map(|(_, held)| held),
        Err(PairedRefusal::NotFound)
    );
}
