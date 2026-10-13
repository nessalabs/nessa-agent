//! Row J23: the host's verdict on the notes waiting for a turn, from each
//! author's grant as it stands, read once per grant.

use super::*;
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::sessions::{NoteDelivery, NoteId, NoteText};
use nessa_sdk::domain::agent_execution::executions::GrantInitiator;
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
                role,
                policy_revision: 9,
            }))
        };
        let answer = match receiver_id {
            "commenter" => held(ShareRole::Comment),
            "driver" => held(ShareRole::Drive),
            "reader" => held(ShareRole::Read),
            "unreadable" => Err(ConversationError::Unavailable),
            _ => Ok(None),
        };
        Box::pin(async move { answer })
    }
    fn change(&self, _: ShareGrantChange) -> ConversationFuture<'_, bool> {
        unreachable!("a verdict changes no grant")
    }
    fn grants<'a>(&'a self, _: &'a ConversationId) -> ConversationFuture<'a, Vec<ShareGrant>> {
        unreachable!("a verdict lists no grants")
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

#[tokio::test]
async fn a_note_is_allowed_while_its_author_holds_comment_and_revoked_once_it_does_not() {
    let grants = Scripted(Mutex::new(Vec::new()));
    let id = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let notes = vec![
        note("owner", Initiator::Owner),
        // Left under another revision than the grant holds now: still the
        // same grant, and a policy change keeps its note.
        note("comment", by("commenter", 1)),
        note("drive", by("driver", 1)),
        note("read", by("reader", 1)),
        note("gone", by("revoked", 1)),
        note("unknown", by("unreadable", 1)),
        note("comment-again", by("commenter", 2)),
    ];
    let candidates = note_candidates(&grants, &id, &notes).await;
    let ids = |values: &[&str]| -> Vec<NoteId> {
        values.iter().map(|id| NoteId::new(*id).unwrap()).collect()
    };
    assert_eq!(
        candidates.authorized,
        ids(&["owner", "comment", "drive", "comment-again"])
    );
    // An unreadable grant is neither: its note waits.
    assert_eq!(candidates.revoked, ids(&["read", "gone"]));
    assert_eq!(
        grants.0.lock().unwrap().as_slice(),
        ["commenter", "driver", "reader", "revoked", "unreadable"]
    );
}
