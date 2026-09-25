//! A conversation's summary rules, and who a deleted one answers, tested
//! without storage, a runtime, or an agent.
use super::{
    Conversation, ConversationDeletion, ConversationPreview, ConversationRefusal,
    ConversationSummary, ConversationTitle, DeletionContradiction, ProviderSessionErasure,
    ProviderSessionLink,
};
use crate::agents::domain::AgentId;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;

fn preview(text: &str) -> Option<String> {
    ConversationPreview::from_text(text).map(|preview| preview.as_str().to_owned())
}

#[test]
fn a_reply_previews_as_one_line_of_plain_text() {
    assert_eq!(
        preview(
            "## Booked\n\n- Confirmation **K7Q2PX**, see [the email](https://x.test)\n- Run `pnpm dev` in snake_case_dir"
        )
        .as_deref(),
        Some("Booked Confirmation K7Q2PX, see the email Run pnpm dev in snake_case_dir")
    );
    for (text, expected) in [
        ("# One\n###### Six\n####### Seven", "One Six ####### Seven"),
        (
            "> quoted\n   > indented\n    > code",
            "quoted indented > code",
        ),
        ("* a\n+ b\n1. c\n12) d\n> - nested", "a b c d nested"),
        ("![a cat](cat.png) and [x]", "a cat and [x]"),
        (
            "__bold__ *it* _it_ ~~gone~~ ``co`de``",
            "bold it it gone co`de",
        ),
        ("[**bold link**](https://x.test)", "bold link"),
        // Not marks: inside a word, around whitespace, or never closed.
        (
            "2*3*4 a * b * c snake_case *open",
            "2*3*4 a * b * c snake_case *open",
        ),
        ("__init__ and ***three***", "init and ***three***"),
        ("#hashtag", "#hashtag"),
        ("tab\there\u{7}bell\u{2028}line", "tab here bell line"),
    ] {
        assert_eq!(preview(text).as_deref(), Some(expected), "{text:?}");
    }
    // Nothing a row could show.
    for text in ["", " \n\t", "- ", "## ", "> "] {
        assert_eq!(preview(text), None, "{text:?}");
    }
}

#[test]
fn a_preview_is_cut_on_a_character_boundary_at_its_byte_bound() {
    let text = "€".repeat(300);
    let preview = ConversationPreview::from_text(&text).unwrap();
    assert_eq!(preview.as_str(), "€".repeat(170));
    assert!(preview.as_str().len() <= ConversationPreview::MAX_BYTES);
    // A cut that lands on a space does not leave it trailing.
    let words = format!("{} tail", "a".repeat(511));
    assert_eq!(
        ConversationPreview::from_text(&words).unwrap().as_str(),
        "a".repeat(511)
    );
}

#[test]
fn a_title_is_the_first_line_of_the_first_message() {
    for (text, file, expected) in [
        ("  Plan the trip\nwith details", None, "Plan the trip"),
        ("\n\n  spaced    out  words ", None, "spaced out words"),
        ("", Some("report.pdf"), "report.pdf"),
        ("   ", Some("report.pdf"), "report.pdf"),
        ("", None, "Image"),
        // Read as plain text, like a preview: it is shown in the same list.
        ("# Heading", None, "Heading"),
        (
            "Plan a **3-day** trip\nin `Lisbon`",
            None,
            "Plan a 3-day trip",
        ),
        ("", Some("snake_case_notes.md"), "snake_case_notes.md"),
        // A line is the first only if something of it is kept: control
        // characters are not, alone or once a marker is dropped.
        ("\u{7}\nHello there", None, "Hello there"),
        ("- \u{7}\nHello there", None, "Hello there"),
        ("- \u{7}", Some("report.pdf"), "report.pdf"),
        ("\u{1b}\u{7}", None, "Image"),
    ] {
        assert_eq!(
            ConversationTitle::for_message(text, file).as_str(),
            expected,
            "{text:?}"
        );
    }
    let long = ConversationTitle::for_message(&"é".repeat(60), None);
    assert_eq!(long.as_str().chars().count(), ConversationTitle::MAX_CHARS);
    // A cut that lands on a space does not leave it trailing.
    let words = format!("{} tail", "a".repeat(47));
    assert_eq!(
        ConversationTitle::for_message(&words, None).as_str(),
        "a".repeat(47)
    );
}

#[test]
fn a_title_and_a_preview_read_the_same_bounded_opening() {
    let title = |text: &str| {
        ConversationTitle::for_message(text, None)
            .as_str()
            .to_owned()
    };
    // Lines with nothing visible once their marks are dropped are passed
    // over however many there are, by both.
    for text in [
        format!("{}Late line", "\n".repeat(9000)),
        format!("{}Late line", "> \n".repeat(3000)),
        format!("{}Late line", "\n \u{7}".repeat(9000)),
        format!("{}Late line", "- \u{7}\n".repeat(3000)),
        format!("{}Late line", "[](x)\n".repeat(100)),
        // More of them than one bound's worth: read as plain text, not
        // taken for visible because they hold characters.
        format!("{}Late line", "[](x)\n".repeat(2000)),
    ] {
        assert_eq!(title(&text), "Late line");
        assert_eq!(preview(&text).as_deref(), Some("Late line"));
    }
    // The markers before a line's first words are passed over too.
    assert_eq!(title(&format!("{}Late", "- ".repeat(5000))), "Late");
    // A first line is read only to the bound: its words past it are not.
    let long = format!("a{}Late", " ".repeat(9000));
    assert_eq!(title(&long), "a");
    assert_eq!(preview(&long).as_deref(), Some("a"));
    // Lines that say nothing only once read as plain text are read for at
    // most the bound in all, then the opening starts where they do.
    // Eight bytes a line, so the opening's bound ends on a line.
    let links = format!("{}Late", "[](xyz)\n".repeat(5000));
    assert_eq!(
        ConversationTitle::for_message(&links, Some("notes.md")).as_str(),
        "notes.md"
    );
    assert_eq!(preview(&links), None);
}

#[test]
fn stored_values_must_be_what_the_rules_produce() {
    assert!(ConversationTitle::new("Plan the trip").is_ok());
    assert!(ConversationTitle::new(&"a".repeat(ConversationTitle::MAX_CHARS)).is_ok());
    for title in ["", " padded", "two  spaces", "line\nbreak", &"a".repeat(49)] {
        assert!(ConversationTitle::new(title).is_err(), "{title:?}");
    }
    assert!(ConversationPreview::new(&"a".repeat(ConversationPreview::MAX_BYTES)).is_ok());
    for preview in ["", "trailing ", "tab\there", &"a".repeat(513)] {
        assert!(ConversationPreview::new(preview).is_err(), "{preview:?}");
    }
}

#[test]
fn a_summary_keeps_its_first_title_and_follows_what_was_said_last() {
    let first = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    assert_eq!(first.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(first.preview().unwrap().as_str(), "Plan the trip");
    assert_eq!(first.updated_at_ms(), 10);

    let second = ConversationSummary::after_message(Some(&first), "", Some("a.pdf"), 20);
    assert_eq!(second.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(second.preview().unwrap().as_str(), "Attachment");
    assert_eq!(second.updated_at_ms(), 20);

    let reply = ConversationSummary::after_reply(Some(&second), "**Done.**", 30).unwrap();
    assert_eq!(reply.title().unwrap().as_str(), "Plan the trip");
    assert_eq!(reply.preview().unwrap().as_str(), "Done.");
    assert_eq!(reply.updated_at_ms(), 30);

    // A reply with nothing to show changes nothing.
    assert_eq!(
        ConversationSummary::after_reply(Some(&reply), " ", 40),
        None
    );
    // A reply never titles a conversation that has no title yet.
    let untitled = ConversationSummary::after_reply(None, "hello", 5).unwrap();
    assert_eq!(untitled.title(), None);
    // Its first message does.
    let titled = ConversationSummary::after_message(Some(&untitled), "Name me", None, 6);
    assert_eq!(titled.title().unwrap().as_str(), "Name me");

    // A clock stepped backwards does not make a conversation older.
    let stepped = ConversationSummary::after_message(Some(&reply), "again", None, 1);
    assert_eq!(stepped.updated_at_ms(), 30);
}

#[test]
fn a_new_message_unarchives_and_a_reply_does_not() {
    let said = ConversationSummary::after_message(None, "Plan the trip", None, 10);
    assert!(!said.archived());
    let archived = said.after_archiving(true);
    assert!(archived.archived());
    // Archiving says nothing and changes nothing else, the time included.
    assert_eq!(archived.title(), said.title());
    assert_eq!(archived.preview(), said.preview());
    assert_eq!(archived.updated_at_ms(), 10);

    // A turn that finishes after the archive is not somebody talking in it.
    let replied = ConversationSummary::after_reply(Some(&archived), "Done.", 20).unwrap();
    assert!(replied.archived());
    // A message is.
    let talked = ConversationSummary::after_message(Some(&replied), "again", None, 30);
    assert!(!talked.archived());
    assert_eq!(talked.title(), said.title());
    let unarchived = archived.after_archiving(false);
    assert!(!unarchived.archived());
    assert_eq!(unarchived.updated_at_ms(), 10);
}

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
    use crate::conversation::domain::LATEST_TIME_MS;
    let created = |at: u64| {
        Conversation::new(
            ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            at,
            AgentId::Claude,
        )
    };
    assert!(created(LATEST_TIME_MS).is_ok());
    assert!(created(LATEST_TIME_MS + 1).is_err());
}
