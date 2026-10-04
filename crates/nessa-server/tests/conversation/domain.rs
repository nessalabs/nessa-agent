//! Who a deleted conversation answers, tested without storage, a runtime, or
//! an agent. The summary rules are tested where they live, in
//! `nessa-protocol/tests/conversation/summary.rs`.
use super::{
    Conversation, ConversationDeletion, ConversationRefusal, DeletionContradiction,
    ProviderSessionErasure, ProviderSessionLink,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;

fn deletion(request: &str) -> ConversationDeletion {
    ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        request.into(),
        100,
    )
    .unwrap()
}
fn owned() -> Conversation {
    Conversation::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("test-model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}

#[test]
fn a_tombstone_keeps_the_first_decision_and_reads_its_history_once() {
    let first = deletion("delete-1");
    assert_eq!(first.provider_session(), &ProviderSessionLink::Unread);
    assert_eq!(first.provider_erasure(), None);
    let session = ExecutionSessionId::new("provider-session").unwrap();

    // The same decision completes it with what its history named.
    let read = first.followed_by(&first.after_reading(Some(session.clone())));
    assert_eq!(read.request(), "delete-1");
    assert_eq!(
        read.provider_session(),
        &ProviderSessionLink::Recorded(session.clone())
    );
    // Read once: an erased history read later names nothing, and that is not
    // allowed to overwrite what was read before it was erased.
    assert_eq!(read.after_reading(None), read);
    assert_eq!(read.followed_by(&first.after_reading(None)), read);
    // A later delete by another request changes nothing about who decided,
    // and cannot carry the first's progress anywhere.
    assert_eq!(first.followed_by(&deletion("delete-2")), first);
    assert_eq!(
        first.followed_by(&deletion("delete-2").after_reading(Some(session.clone()))),
        first
    );
    // A history that named no provider session is read too, and settles the
    // provider session at once: there is nothing to ask the agent.
    let absent = first.followed_by(&first.after_reading(None));
    assert_eq!(absent.provider_session(), &ProviderSessionLink::Absent);
    assert_eq!(
        absent.provider_erasure(),
        Some(ProviderSessionErasure::NoProviderSession)
    );

    // What became of the agent's record is settled once, only for a session
    // that was read, and never as "no provider session" for one that was.
    assert_eq!(
        first.after_provider_erasure(ProviderSessionErasure::Deleted),
        first
    );
    assert_eq!(
        read.after_provider_erasure(ProviderSessionErasure::NoProviderSession),
        read
    );
    let settled = read.followed_by(&read.after_provider_erasure(ProviderSessionErasure::Archived));
    assert_eq!(
        settled.provider_erasure(),
        Some(ProviderSessionErasure::Archived)
    );
    assert_eq!(
        settled.after_provider_erasure(ProviderSessionErasure::Deleted),
        settled
    );
    // Erased only once that is settled, and for good.
    assert_eq!(read.after_erasure(), None);
    let finished = settled.followed_by(&settled.after_erasure().unwrap());
    assert!(finished.erased());
    assert_eq!(finished.followed_by(&settled), finished);
    assert!(finished.followed_by(&deletion("delete-2")).erased());

    // What gets written down is held to the creator's rule.
    for (surface, request) in [("", "delete"), ("panel", "line\nbreak")] {
        assert!(ConversationDeletion::new(
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            surface.into(),
            request.into(),
            1
        )
        .is_err());
    }
}

#[test]
fn the_first_decision_stands_whatever_the_repository_does() {
    // A repository that only persists what `deleted` returns, handed every
    // later delete as it comes.
    let decided = owned().deleted(deletion("delete-1")).unwrap();
    let from_phone = ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "phone".into(),
        "delete-9".into(),
        900,
    )
    .unwrap();
    let still = decided.clone().deleted(from_phone).unwrap();
    assert_eq!(still.deletion(), decided.deletion());
    assert_eq!(still.deletion().unwrap().surface(), "panel");
    // The same decision carries it further, and only further.
    let read = deletion("delete-1").after_reading(None);
    let further = still.deleted(read.clone()).unwrap();
    assert_eq!(further.deletion(), Some(&read));
    let unread_again = further.clone().deleted(deletion("delete-1")).unwrap();
    assert_eq!(unread_again.deletion(), Some(&read));
}

#[test]
fn the_deciding_request_is_the_same_caller_on_the_same_surface_asking_again() {
    let decided = deletion("delete-1");
    // A repeat of the request, later, is the same decision.
    let repeat = ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        "delete-1".into(),
        500,
    )
    .unwrap();
    assert!(decided.is_same_decision(&repeat));
    // The same request identifier from another surface, principal or
    // organization, or another request, is not.
    for other in [
        ConversationDeletion::new(
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "phone".into(),
            "delete-1".into(),
            100,
        ),
        ConversationDeletion::new(
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("someone").unwrap(),
            "panel".into(),
            "delete-1".into(),
            100,
        ),
        ConversationDeletion::new(
            OrganizationId::new("elsewhere").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "delete-1".into(),
            100,
        ),
        Ok(deletion("delete-2")),
    ] {
        let other = other.unwrap();
        assert!(!decided.is_same_decision(&other));
        // Nor can it carry the decision's progress.
        assert_eq!(
            decided.followed_by(&other.after_reading(None)),
            decided,
            "{other:?}"
        );
    }
}

#[test]
fn a_restored_tombstone_that_contradicts_itself_or_its_conversation_is_refused() {
    let session = || ProviderSessionLink::Recorded(ExecutionSessionId::new("provider").unwrap());
    // Progress no deletion reaches.
    for (link, erasure, erased) in [
        (
            ProviderSessionLink::Unread,
            Some(ProviderSessionErasure::NoProviderSession),
            false,
        ),
        (
            ProviderSessionLink::Absent,
            Some(ProviderSessionErasure::Deleted),
            false,
        ),
        (
            session(),
            Some(ProviderSessionErasure::NoProviderSession),
            false,
        ),
        (ProviderSessionLink::Unread, None, true),
        (session(), None, true),
        // Read as naming none, or as unreadable, and left unsettled: reading
        // settles both at once, so no deletion stands there.
        (ProviderSessionLink::Absent, None, false),
        (ProviderSessionLink::Unknown, None, false),
        (
            ProviderSessionLink::Unknown,
            Some(ProviderSessionErasure::NoProviderSession),
            true,
        ),
        (
            session(),
            Some(ProviderSessionErasure::SessionUnknown),
            false,
        ),
    ] {
        assert_eq!(
            ConversationDeletion::restore(deletion("delete-1"), link, erasure, erased),
            Err(DeletionContradiction::ImpossibleProgress)
        );
    }
    // And every state a deletion does reach is accepted.
    for (link, erasure, erased) in [
        (ProviderSessionLink::Unread, None, false),
        (session(), None, false),
        (session(), Some(ProviderSessionErasure::NoHandler), true),
        (
            ProviderSessionLink::Absent,
            Some(ProviderSessionErasure::NoProviderSession),
            true,
        ),
        (
            ProviderSessionLink::Unknown,
            Some(ProviderSessionErasure::SessionUnknown),
            true,
        ),
    ] {
        assert!(ConversationDeletion::restore(deletion("delete-1"), link, erasure, erased).is_ok());
    }

    // A tombstone naming anybody but the owner, in its organization, or dated
    // before the conversation, cannot stand beside it.
    let stranger = ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("stranger").unwrap(),
        "panel".into(),
        "delete-1".into(),
        100,
    )
    .unwrap();
    let elsewhere = ConversationDeletion::new(
        OrganizationId::new("elsewhere").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        "delete-1".into(),
        100,
    )
    .unwrap();
    let early = ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "panel".into(),
        "delete-1".into(),
        0,
    )
    .unwrap();
    assert_eq!(
        owned().deleted(stranger),
        Err(DeletionContradiction::InitiatorNotOwner)
    );
    assert_eq!(
        owned().deleted(elsewhere),
        Err(DeletionContradiction::InitiatorNotOwner)
    );
    assert_eq!(
        owned().deleted(early),
        Err(DeletionContradiction::BeforeCreation)
    );
    assert!(owned().deleted(deletion("delete-1")).is_ok());
}

#[test]
fn only_the_owner_is_told_a_conversation_was_deleted() {
    let conversation = owned();
    let owner = (
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
    );
    let others = [
        (
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("stranger").unwrap(),
        ),
        (
            OrganizationId::new("elsewhere").unwrap(),
            PrincipalId::new("person").unwrap(),
        ),
    ];
    assert_eq!(conversation.check_access(&owner.0, &owner.1), Ok(()));
    let deleted = conversation.deleted(deletion("delete-1")).unwrap();
    assert_eq!(
        deleted.check_access(&owner.0, &owner.1),
        Err(ConversationRefusal::Deleted)
    );
    // Still the owner's, to finish deleting.
    assert!(deleted.allows(&owner.0, &owner.1));
    for (organization, principal) in &others {
        assert_eq!(
            deleted.check_access(organization, principal),
            Err(ConversationRefusal::NotFound)
        );
    }
}

#[test]
fn every_restored_tombstone_state_is_accepted_or_refused_by_the_rule() {
    use ProviderSessionErasure as E;
    let links = [
        ProviderSessionLink::Unread,
        ProviderSessionLink::Absent,
        ProviderSessionLink::Unknown,
        ProviderSessionLink::Recorded(ExecutionSessionId::new("provider").unwrap()),
    ];
    let erasures = [
        None,
        Some(E::NoProviderSession),
        Some(E::SessionUnknown),
        Some(E::Deleted),
        Some(E::Archived),
        Some(E::Acknowledged),
        Some(E::NotListed),
        Some(E::NotSupported),
        Some(E::NoHandler),
    ];
    // The rule, stated from what a deletion does: reading a history that
    // named nothing, or finding none to read, settles it in the same step;
    // a session that was read is settled only by asking about it; and only a
    // settled deletion can be finished.
    let reachable = |link: &ProviderSessionLink, erasure: Option<E>, erased: bool| {
        let settled = match link {
            ProviderSessionLink::Unread => erasure.is_none(),
            ProviderSessionLink::Absent => erasure == Some(E::NoProviderSession),
            ProviderSessionLink::Unknown => erasure == Some(E::SessionUnknown),
            ProviderSessionLink::Recorded(_) => {
                !matches!(erasure, Some(E::NoProviderSession | E::SessionUnknown))
            }
        };
        settled && (!erased || erasure.is_some())
    };
    let mut seen = 0;
    for link in &links {
        for erasure in erasures {
            for erased in [false, true] {
                seen += 1;
                let restored = ConversationDeletion::restore(
                    deletion("delete-1"),
                    link.clone(),
                    erasure,
                    erased,
                );
                if reachable(link, erasure, erased) {
                    assert!(restored.is_ok(), "{link:?} {erasure:?} {erased}");
                } else {
                    assert_eq!(
                        restored,
                        Err(DeletionContradiction::ImpossibleProgress),
                        "{link:?} {erasure:?} {erased}"
                    );
                }
            }
        }
    }
    assert_eq!(seen, 72);
}

#[test]
fn a_record_created_past_the_latest_time_is_refused() {
    use nessa_protocol::conversation::domain::LATEST_TIME_MS;
    let created = |at: u64| {
        Conversation::new(
            ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            at,
            AgentId::Claude,
            ConversationModelId::new("test-model").unwrap(),
            ConversationApprovalMode::Ask,
        )
    };
    assert!(created(LATEST_TIME_MS).is_ok());
    assert!(created(LATEST_TIME_MS + 1).is_err());
}
