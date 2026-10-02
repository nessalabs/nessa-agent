//! The resource ticket store: what a ticket buys, for how long, against what
//! bound, what the store keeps of it, and how each ticket's end is recorded.
use super::*;
use crate::mcp_servers::infrastructure::ticket_test_support::{
    app, app_initiator, conversation, held, Fixture, RecordingAudit, ScriptedRandom, CONVERSATION,
    OTHER_CONVERSATION,
};
use std::collections::HashSet;

const PAGE: &[u8] = b"<!doctype html><title>chart</title>";

fn ends(fixture: &Fixture) -> Vec<(TicketEnd, ResourceTicketDigest)> {
    fixture
        .ends
        .take()
        .into_iter()
        .map(|event| (event.end, event.ticket_digest))
        .collect()
}

#[test]
fn issue_then_redeem_hands_over_exactly_the_held_bytes_once() {
    let fixture = Fixture::new();
    let resource = held(CONVERSATION, app("call-1", "mount-1"), PAGE);
    let ticket = fixture.store.issue(resource.clone()).unwrap();

    assert_eq!(
        fixture.store.redeem(ticket.as_bytes()),
        Some(Redemption {
            resource,
            ticket_digest: ResourceTicketDigest::of(&ticket),
        })
    );
    // A redemption is the route's to record, never an event too.
    assert!(fixture.ends.take().is_empty());
    // Spent: the second redemption finds nothing, and reports nothing.
    assert_eq!(fixture.store.redeem(ticket.as_bytes()), None);
    assert!(fixture.ends.take().is_empty());
}

#[test]
fn a_ticket_is_refused_from_its_deadline_and_its_expiry_is_reported() {
    let fixture = Fixture::new();
    let early = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    let late = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();

    // One millisecond before its deadline, a ticket still works.
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS - 1);
    assert!(fixture.store.redeem(early.as_bytes()).is_some());
    assert!(ends(&fixture).is_empty());

    // At its deadline it is refused, its bytes are let go of, and it ended
    // unredeemed.
    fixture.clock.advance(1);
    assert_eq!(fixture.store.redeem(late.as_bytes()), None);
    assert_eq!(
        ends(&fixture),
        vec![(TicketEnd::Expired, ResourceTicketDigest::of(&late))]
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
}

#[test]
fn an_unredeemed_ticket_expires_on_a_sweep_with_nothing_else_asked() {
    let fixture = Fixture::new();
    let ticket = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    fixture.store.sweep();
    assert!(fixture.ends.take().is_empty());
    assert_eq!(
        fixture.store.held_bytes(&conversation(CONVERSATION)),
        PAGE.len()
    );

    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    let ended = fixture.ends.take();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end, TicketEnd::Expired);
    assert_eq!(ended[0].ticket_digest, ResourceTicketDigest::of(&ticket));
    assert_eq!(
        ended[0].phase(),
        McpAppAuditPhase::TicketExpired {
            ticket_digest: ResourceTicketDigest::of(&ticket).to_hex()
        }
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    assert!(fixture.store.held_digests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_periodic_sweep_expires_tickets_and_stops_with_the_store() {
    let fixture = Fixture::new();
    fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    let sweeping = tokio::spawn(ResourceTicketStore::sweep_periodically(
        Arc::downgrade(&fixture.store),
        Duration::from_secs(5),
    ));
    tokio::task::yield_now().await;
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(ends(&fixture)[0].0, TicketEnd::Expired);

    let Fixture { store, .. } = fixture;
    drop(store);
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::timeout(Duration::from_secs(1), sweeping)
        .await
        .expect("the sweep ends once nothing holds the store")
        .unwrap();
}

#[test]
fn a_conversation_holds_at_most_its_bound_and_each_end_frees_what_it_held() {
    let fixture = Fixture::new();
    let us = conversation(CONVERSATION);
    let half = vec![b'x'; MAX_HELD_RESOURCE_BYTES / 2];
    let mount = app("call-1", "mount-1");

    let redeemed = fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .unwrap();
    let also_released = fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .unwrap();
    assert_eq!(fixture.store.held_bytes(&us), MAX_HELD_RESOURCE_BYTES);
    // Full: one byte more is refused, and nothing is reported for it.
    assert_eq!(
        fixture.store.issue(held(CONVERSATION, mount.clone(), b"x")),
        Err(TicketRefusal::Capacity)
    );
    // Another conversation's bound is its own.
    assert!(fixture
        .store
        .issue(held(OTHER_CONVERSATION, mount.clone(), b"x"))
        .is_ok());
    // One resource larger than the bound never fits.
    let other = Fixture::new();
    assert_eq!(
        other.store.issue(held(
            CONVERSATION,
            mount.clone(),
            &vec![0; MAX_HELD_RESOURCE_BYTES + 1]
        )),
        Err(TicketRefusal::Capacity)
    );

    // Redemption frees its bytes.
    assert!(fixture.store.redeem(redeemed.as_bytes()).is_some());
    assert_eq!(fixture.store.held_bytes(&us), half.len());
    let released = fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .unwrap();

    // Release frees its bytes.
    fixture.store.release_conversation(&us);
    assert_eq!(fixture.store.held_bytes(&us), 0);
    assert_eq!(fixture.store.redeem(released.as_bytes()), None);
    assert_eq!(fixture.store.redeem(also_released.as_bytes()), None);

    // Expiry frees its bytes.
    fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .unwrap();
    fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .unwrap();
    assert_eq!(
        fixture.store.issue(held(CONVERSATION, mount.clone(), b"x")),
        Err(TicketRefusal::Capacity)
    );
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    assert!(fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), &half))
        .is_ok());
    assert_eq!(fixture.store.held_bytes(&us), half.len());
}

#[test]
fn releasing_a_conversation_ends_its_tickets_and_no_others() {
    let fixture = Fixture::new();
    let ours = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    let theirs = fixture
        .store
        .issue(held(OTHER_CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    fixture
        .store
        .release_conversation(&conversation(CONVERSATION));
    assert_eq!(
        ends(&fixture),
        vec![(
            TicketEnd::ConversationReleased,
            ResourceTicketDigest::of(&ours)
        )]
    );
    assert_eq!(fixture.store.redeem(ours.as_bytes()), None);
    assert!(fixture.store.redeem(theirs.as_bytes()).is_some());
}

#[test]
fn releasing_an_app_ends_only_that_mounts_tickets_and_twice_is_once() {
    let fixture = Fixture::new();
    let mount = app("call-1", "mount-1");
    // The same tool call mounted again, inline and in a pane.
    let remount = app("call-1", "mount-2");
    let released = fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), PAGE))
        .unwrap();
    let kept = fixture
        .store
        .issue(held(CONVERSATION, remount.clone(), PAGE))
        .unwrap();
    // The same mount ids in another conversation are another app.
    let elsewhere = fixture
        .store
        .issue(held(OTHER_CONVERSATION, mount.clone(), PAGE))
        .unwrap();

    fixture
        .store
        .release_app(&conversation(CONVERSATION), &mount);
    assert_eq!(
        ends(&fixture),
        vec![(TicketEnd::AppReleased, ResourceTicketDigest::of(&released))]
    );
    assert_eq!(
        fixture.store.held_bytes(&conversation(CONVERSATION)),
        PAGE.len()
    );
    // Again: nothing more to end, and nothing reported.
    fixture
        .store
        .release_app(&conversation(CONVERSATION), &mount);
    assert!(fixture.ends.take().is_empty());

    assert_eq!(fixture.store.redeem(released.as_bytes()), None);
    assert!(fixture.store.redeem(kept.as_bytes()).is_some());
    assert!(fixture.store.redeem(elsewhere.as_bytes()).is_some());
}

#[test]
fn every_ticket_ends_once_whatever_ends_it() {
    let fixture = Fixture::new();
    let us = conversation(CONVERSATION);
    let mount = app("call-1", "mount-1");
    let issue = || {
        fixture
            .store
            .issue(held(CONVERSATION, mount.clone(), PAGE))
            .unwrap()
    };
    let redeemed = issue();
    let app_released = issue();
    let conversation_released = fixture
        .store
        .issue(held(CONVERSATION, app("call-2", "mount-2"), PAGE))
        .unwrap();

    assert!(fixture.store.redeem(redeemed.as_bytes()).is_some());
    fixture.store.release_app(&us, &mount);
    fixture.store.release_conversation(&us);
    // Each again, in every order, and then their deadline: nothing more ends.
    for ticket in [&redeemed, &app_released, &conversation_released] {
        assert_eq!(fixture.store.redeem(ticket.as_bytes()), None);
    }
    fixture.store.release_app(&us, &mount);
    fixture.store.release_conversation(&us);
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    assert_eq!(
        ends(&fixture),
        vec![
            (
                TicketEnd::AppReleased,
                ResourceTicketDigest::of(&app_released)
            ),
            (
                TicketEnd::ConversationReleased,
                ResourceTicketDigest::of(&conversation_released)
            ),
        ]
    );

    // Expiry after a release ends nothing again; a ticket left to expire
    // ends once, by expiry, however often it is swept and redeemed.
    let expired = issue();
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    fixture.store.sweep();
    assert_eq!(fixture.store.redeem(expired.as_bytes()), None);
    fixture.store.release_conversation(&us);
    assert_eq!(
        ends(&fixture),
        vec![(TicketEnd::Expired, ResourceTicketDigest::of(&expired))]
    );
}

#[test]
fn dropping_the_store_ends_every_ticket_it_still_holds() {
    let fixture = Fixture::new();
    let ticket = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    let Fixture { store, ends, .. } = fixture;
    drop(store);
    let ended = ends.take();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end, TicketEnd::StoreDropped);
    assert_eq!(ended[0].ticket_digest, ResourceTicketDigest::of(&ticket));
    assert_eq!(
        ended[0].phase(),
        McpAppAuditPhase::TicketExpired {
            ticket_digest: ResourceTicketDigest::of(&ticket).to_hex()
        }
    );
}

#[test]
fn the_store_keeps_only_each_tickets_digest() {
    let fixture = Fixture::new();
    let tickets: Vec<String> = (0..3)
        .map(|_| {
            fixture
                .store
                .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
                .unwrap()
        })
        .collect();
    let digests: HashSet<_> = fixture.store.held_digests().into_iter().collect();
    let expected: HashSet<_> = tickets.iter().map(ResourceTicketDigest::of).collect();
    assert_eq!(digests, expected);
    // Nothing the store can show of itself has a ticket in it: its digests'
    // own rendering is of the digest, and no ticket is a digest's hex.
    for (ticket, digest) in tickets
        .iter()
        .zip(tickets.iter().map(ResourceTicketDigest::of))
    {
        let shown = format!("{digest:?} {}", digest.to_hex());
        assert!(!shown.contains(ticket.as_str()), "{shown}");
    }
}

#[test]
fn without_random_bytes_no_ticket_is_issued_and_nothing_is_held() {
    let fixture = Fixture::new();
    fixture.random.fail(true);
    assert_eq!(
        fixture
            .store
            .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE)),
        Err(TicketRefusal::Unavailable)
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    assert!(fixture.store.held_digests().is_empty());
    // The source recovers, and so does issuing.
    fixture.random.fail(false);
    assert!(fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .is_ok());
}

#[test]
fn a_random_source_that_repeats_itself_issues_no_second_ticket() {
    let fixture = Fixture::with_random(ScriptedRandom::repeating());
    let first = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    assert_eq!(
        fixture
            .store
            .issue(held(CONVERSATION, app("call-2", "mount-2"), PAGE)),
        Err(TicketRefusal::Unavailable)
    );
    // The first ticket still holds the first app's bytes, and nothing else.
    let redeemed = fixture.store.redeem(first.as_bytes()).unwrap();
    assert_eq!(redeemed.resource.app(), &app("call-1", "mount-1"));
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
}

#[test]
fn a_redemption_is_recorded_against_the_reading_call_as_the_app() {
    let fixture = Fixture::new();
    let resource = held(CONVERSATION, app("call-1", "mount-1"), PAGE);
    let ticket = fixture.store.issue(resource.clone()).unwrap();
    let redemption = fixture.store.redeem(ticket.as_bytes()).unwrap();
    assert_eq!(
        redemption.audit_record(),
        McpAppAuditRecord {
            phase: McpAppAuditPhase::TicketRedeemed {
                ticket_digest: ResourceTicketDigest::of(&ticket).to_hex()
            },
            initiator: app_initiator(),
            ..resource.record
        }
    );
}

#[test]
fn an_unredeemed_end_is_recorded_against_the_reading_call_as_the_system() {
    let fixture = Fixture::new();
    let mount = app("call-1", "mount-1");
    let resource = held(CONVERSATION, mount.clone(), PAGE);
    let ticket = fixture.store.issue(resource.clone()).unwrap();
    fixture
        .store
        .release_app(&conversation(CONVERSATION), &mount);
    let ended = fixture.ends.take();
    assert_eq!(ended[0].record, resource.record);
    assert_eq!(
        ended[0].audit_record(),
        McpAppAuditRecord {
            phase: McpAppAuditPhase::TicketExpired {
                ticket_digest: ResourceTicketDigest::of(&ticket).to_hex()
            },
            initiator: McpAppInitiator::System,
            ..resource.record
        }
    );
}

#[tokio::test]
async fn each_unredeemed_end_is_audited_in_order_and_a_failed_record_does_not_stop_the_next() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let audit = Arc::new(RecordingAudit::default());
    let auditing = tokio::spawn(audit_ticket_ends(receiver, audit.clone()));
    let fixture = Fixture::new();
    let store = ResourceTicketStore::new(
        fixture.clock.clone(),
        fixture.random.clone(),
        Arc::new(sender),
    );
    let lost = store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    audit.fail(true);
    store.release_app(&conversation(CONVERSATION), &app("call-1", "mount-1"));
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    audit.fail(false);
    let expired = store
        .issue(held(CONVERSATION, app("call-2", "mount-2"), PAGE))
        .unwrap();
    let released = store
        .issue(held(OTHER_CONVERSATION, app("call-3", "mount-3"), PAGE))
        .unwrap();
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    store.sweep();
    store.release_conversation(&conversation(OTHER_CONVERSATION));
    // The store dropped is every sender gone: the audit finishes what it was
    // sent and ends.
    drop(store);
    auditing.await.unwrap();

    let records = audit.take();
    let digests: Vec<_> = records
        .iter()
        .map(|record| match &record.phase {
            McpAppAuditPhase::TicketExpired { ticket_digest } => ticket_digest.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    // `released` was past its deadline by its release: it ended once, by
    // the sweep, after `expired`.
    assert_eq!(
        digests,
        [&expired, &released].map(|ticket| ResourceTicketDigest::of(ticket).to_hex())
    );
    assert!(!digests.contains(&ResourceTicketDigest::of(&lost).to_hex()));
    assert!(records
        .iter()
        .all(|record| record.initiator == McpAppInitiator::System));
    assert_eq!(records[0].call_id, "call-call-2");
}

#[tokio::test]
async fn ends_sent_to_a_channel_arrive_and_a_closed_channel_does_not_stop_cleanup() {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let fixture = Fixture::new();
    let store = ResourceTicketStore::new(
        fixture.clock.clone(),
        fixture.random.clone(),
        Arc::new(sender),
    );
    store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    store.release_conversation(&conversation(CONVERSATION));
    assert_eq!(
        receiver.recv().await.unwrap().end,
        TicketEnd::ConversationReleased
    );

    // Nothing receives any more: the end is lost, and said to be, but the
    // bytes are let go of all the same.
    drop(receiver);
    store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    store.release_conversation(&conversation(CONVERSATION));
    assert_eq!(store.held_bytes(&conversation(CONVERSATION)), 0);
}
