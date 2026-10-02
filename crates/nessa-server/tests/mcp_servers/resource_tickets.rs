//! The resource ticket store: what a ticket buys, for how long, against what
//! bound, what the store keeps of it, and how each ticket's end is recorded.
use super::*;
use crate::mcp_servers::infrastructure::ticket_test_support::{
    app, app_initiator, conversation, held, issued, Fixture, RecordingAudit, ScriptedRandom,
    CONVERSATION, OTHER_CONVERSATION,
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
    let ticket = issued(&fixture.store, resource.clone()).unwrap();

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
    let early = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    let late = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
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
    let ticket = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
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
        ended[0].audit_record().phase,
        McpAppAuditPhase::TicketEnded {
            ticket_digest: ResourceTicketDigest::of(&ticket).to_hex(),
            cause: TicketEnd::Expired,
        }
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    assert!(fixture.store.held_digests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_periodic_sweep_expires_tickets_and_stops_with_the_store() {
    let fixture = Fixture::new();
    issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
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

    let redeemed = issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).unwrap();
    let also_released = issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).unwrap();
    assert_eq!(fixture.store.held_bytes(&us), MAX_HELD_RESOURCE_BYTES);
    // Full: one byte more is refused, and nothing is reported for it.
    assert_eq!(
        issued(&fixture.store, held(CONVERSATION, mount.clone(), b"x")),
        Err(TicketRefusal::Capacity)
    );
    // Another conversation's bound is its own.
    assert!(issued(
        &fixture.store,
        held(OTHER_CONVERSATION, mount.clone(), b"x")
    )
    .is_ok());
    // One resource larger than the bound never fits.
    let other = Fixture::new();
    assert_eq!(
        issued(
            &other.store,
            held(
                CONVERSATION,
                mount.clone(),
                &vec![0; MAX_HELD_RESOURCE_BYTES + 1]
            )
        ),
        Err(TicketRefusal::Capacity)
    );

    // Redemption frees its bytes.
    assert!(fixture.store.redeem(redeemed.as_bytes()).is_some());
    assert_eq!(fixture.store.held_bytes(&us), half.len());
    let released = issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).unwrap();

    // Release frees its bytes.
    fixture
        .store
        .release_conversation(&us, &McpAppInitiator::System);
    assert_eq!(fixture.store.held_bytes(&us), 0);
    assert_eq!(fixture.store.redeem(released.as_bytes()), None);
    assert_eq!(fixture.store.redeem(also_released.as_bytes()), None);

    // Expiry frees its bytes.
    issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).unwrap();
    issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).unwrap();
    assert_eq!(
        issued(&fixture.store, held(CONVERSATION, mount.clone(), b"x")),
        Err(TicketRefusal::Capacity)
    );
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    assert!(issued(&fixture.store, held(CONVERSATION, mount.clone(), &half)).is_ok());
    assert_eq!(fixture.store.held_bytes(&us), half.len());
}

#[test]
fn releasing_a_conversation_ends_its_tickets_and_no_others() {
    let fixture = Fixture::new();
    let ours = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    let theirs = issued(
        &fixture.store,
        held(OTHER_CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    fixture
        .store
        .release_conversation(&conversation(CONVERSATION), &McpAppInitiator::System);
    assert_eq!(
        ends(&fixture),
        vec![(
            TicketEnd::ConversationEnded,
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
    let released = issued(&fixture.store, held(CONVERSATION, mount.clone(), PAGE)).unwrap();
    let kept = issued(&fixture.store, held(CONVERSATION, remount.clone(), PAGE)).unwrap();
    // The same mount ids in another conversation are another app.
    let elsewhere = issued(
        &fixture.store,
        held(OTHER_CONVERSATION, mount.clone(), PAGE),
    )
    .unwrap();

    fixture.store.release_app(
        &conversation(CONVERSATION),
        &mount,
        &McpAppInitiator::System,
    );
    assert_eq!(
        ends(&fixture),
        vec![(TicketEnd::AppReleased, ResourceTicketDigest::of(&released))]
    );
    assert_eq!(
        fixture.store.held_bytes(&conversation(CONVERSATION)),
        PAGE.len()
    );
    // Again: nothing more to end, and nothing reported.
    fixture.store.release_app(
        &conversation(CONVERSATION),
        &mount,
        &McpAppInitiator::System,
    );
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
    let issue = || issued(&fixture.store, held(CONVERSATION, mount.clone(), PAGE)).unwrap();
    let redeemed = issue();
    let app_released = issue();
    let conversation_released = issued(
        &fixture.store,
        held(CONVERSATION, app("call-2", "mount-2"), PAGE),
    )
    .unwrap();

    assert!(fixture.store.redeem(redeemed.as_bytes()).is_some());
    fixture
        .store
        .release_app(&us, &mount, &McpAppInitiator::System);
    fixture
        .store
        .release_conversation(&us, &McpAppInitiator::System);
    // Each again, in every order, and then their deadline: nothing more ends.
    for ticket in [&redeemed, &app_released, &conversation_released] {
        assert_eq!(fixture.store.redeem(ticket.as_bytes()), None);
    }
    fixture
        .store
        .release_app(&us, &mount, &McpAppInitiator::System);
    fixture
        .store
        .release_conversation(&us, &McpAppInitiator::System);
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
                TicketEnd::ConversationEnded,
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
    fixture
        .store
        .release_conversation(&us, &McpAppInitiator::System);
    assert_eq!(
        ends(&fixture),
        vec![(TicketEnd::Expired, ResourceTicketDigest::of(&expired))]
    );
}

#[test]
fn dropping_the_store_ends_every_ticket_it_still_holds() {
    let fixture = Fixture::new();
    let ticket = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    let Fixture { store, ends, .. } = fixture;
    drop(store);
    let ended = ends.take();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].end, TicketEnd::ConversationEnded);
    assert_eq!(ended[0].ticket_digest, ResourceTicketDigest::of(&ticket));
    assert_eq!(
        ended[0].audit_record(),
        McpAppAuditRecord {
            phase: McpAppAuditPhase::TicketEnded {
                ticket_digest: ResourceTicketDigest::of(&ticket).to_hex(),
                cause: TicketEnd::ConversationEnded,
            },
            // The gateway stopping: the system's.
            initiator: McpAppInitiator::System,
            ..ended[0].record.clone()
        }
    );
}

#[test]
fn the_store_keeps_only_each_tickets_digest() {
    let fixture = Fixture::new();
    let tickets: Vec<String> = (0..3)
        .map(|_| {
            issued(
                &fixture.store,
                held(CONVERSATION, app("call-1", "mount-1"), PAGE),
            )
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
        issued(
            &fixture.store,
            held(CONVERSATION, app("call-1", "mount-1"), PAGE)
        ),
        Err(TicketRefusal::Unavailable)
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    assert!(fixture.store.held_digests().is_empty());
    // The source recovers, and so does issuing.
    fixture.random.fail(false);
    assert!(issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE)
    )
    .is_ok());
}

#[test]
fn a_random_source_that_repeats_itself_issues_no_second_ticket() {
    let fixture = Fixture::with_random(ScriptedRandom::repeating());
    let first = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    assert_eq!(
        issued(
            &fixture.store,
            held(CONVERSATION, app("call-2", "mount-2"), PAGE)
        ),
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
    let ticket = issued(&fixture.store, resource.clone()).unwrap();
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
    let ticket = issued(&fixture.store, resource.clone()).unwrap();
    fixture.store.release_app(
        &conversation(CONVERSATION),
        &mount,
        &McpAppInitiator::System,
    );
    let ended = fixture.ends.take();
    assert_eq!(ended[0].record, resource.record);
    assert_eq!(
        ended[0].audit_record(),
        McpAppAuditRecord {
            phase: McpAppAuditPhase::TicketEnded {
                ticket_digest: ResourceTicketDigest::of(&ticket).to_hex(),
                cause: TicketEnd::AppReleased,
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
    let (_running, stop) = tokio::sync::oneshot::channel();
    let auditing = tokio::spawn(audit_ticket_ends(receiver, audit.clone(), stop));
    let fixture = Fixture::new();
    let store = ResourceTicketStore::new(
        fixture.clock.clone(),
        fixture.random.clone(),
        Arc::new(sender),
    );
    let lost = issued(&store, held(CONVERSATION, app("call-1", "mount-1"), PAGE)).unwrap();
    audit.fail(true);
    store.release_app(
        &conversation(CONVERSATION),
        &app("call-1", "mount-1"),
        &McpAppInitiator::System,
    );
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
    audit.fail(false);
    let expired = issued(&store, held(CONVERSATION, app("call-2", "mount-2"), PAGE)).unwrap();
    let released = issued(
        &store,
        held(OTHER_CONVERSATION, app("call-3", "mount-3"), PAGE),
    )
    .unwrap();
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    store.sweep();
    store.release_conversation(&conversation(OTHER_CONVERSATION), &McpAppInitiator::System);
    // The store dropped is every sender gone: the audit finishes what it was
    // sent and ends.
    drop(store);
    auditing.await.unwrap();

    let records = audit.take();
    let digests: Vec<_> = records
        .iter()
        .map(|record| match &record.phase {
            McpAppAuditPhase::TicketEnded { ticket_digest, .. } => ticket_digest.clone(),
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
    issued(&store, held(CONVERSATION, app("call-1", "mount-1"), PAGE)).unwrap();
    store.release_conversation(&conversation(CONVERSATION), &McpAppInitiator::System);
    assert_eq!(
        receiver.recv().await.unwrap().end,
        TicketEnd::ConversationEnded
    );

    // Nothing receives any more: the end is lost, and said to be, but the
    // bytes are let go of all the same.
    drop(receiver);
    issued(&store, held(CONVERSATION, app("call-1", "mount-1"), PAGE)).unwrap();
    store.release_conversation(&conversation(CONVERSATION), &McpAppInitiator::System);
    assert_eq!(store.held_bytes(&conversation(CONVERSATION)), 0);
}

#[test]
fn a_conversation_holds_at_most_its_count_of_tickets_whatever_their_size() {
    let fixture = Fixture::new();
    let mount = app("call-1", "mount-1");
    let mut tickets: Vec<_> = (0..MAX_HELD_TICKETS)
        .map(|_| issued(&fixture.store, held(CONVERSATION, mount.clone(), b"")).unwrap())
        .collect();
    assert_eq!(
        issued(&fixture.store, held(CONVERSATION, mount.clone(), b"")),
        Err(TicketRefusal::Capacity)
    );
    assert!(issued(&fixture.store, held(OTHER_CONVERSATION, mount.clone(), b"")).is_ok());
    // One redeemed: room for one more.
    assert!(fixture
        .store
        .redeem(tickets.pop().unwrap().as_bytes())
        .is_some());
    assert!(issued(&fixture.store, held(CONVERSATION, mount, b"")).is_ok());
}

#[test]
fn a_discarded_ticket_is_refused_and_its_end_is_not_reported() {
    let fixture = Fixture::new();
    let mount = app("call-1", "mount-1");
    let ticket = issued(&fixture.store, held(CONVERSATION, mount, b"bytes")).unwrap();
    fixture.store.discard(&ticket);
    assert!(fixture.store.redeem(ticket.as_bytes()).is_none());
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    // Nor when its deadline passes.
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    assert!(fixture.ends.take().is_empty());
}

#[test]
fn a_pending_ticket_is_not_redeemable_and_its_end_is_its_issuers_to_learn() {
    let fixture = Fixture::new();
    let mount = app("call-1", "mount-1");
    let ticket = fixture
        .store
        .issue(held(CONVERSATION, mount.clone(), PAGE))
        .unwrap();
    // Its issue is not on record yet: nobody can have it to spend.
    assert!(fixture.store.redeem(ticket.as_bytes()).is_none());
    // Released while pending: nothing reported, the issuer told on activation.
    let releaser = app_initiator();
    fixture
        .store
        .release_app(&conversation(CONVERSATION), &mount, &releaser);
    assert!(fixture.ends.take().is_empty());
    assert_eq!(
        fixture.store.activate(&ticket),
        Err((TicketEnd::AppReleased, releaser))
    );
    assert_eq!(fixture.store.held_bytes(&conversation(CONVERSATION)), 0);
    // Activated, it is redeemable once.
    let ticket = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-2"), PAGE))
        .unwrap();
    assert_eq!(fixture.store.activate(&ticket), Ok(()));
    assert!(fixture.store.redeem(ticket.as_bytes()).is_some());
}

#[test]
fn a_pending_ticket_past_its_deadline_is_an_expiry_its_issuer_learns() {
    let fixture = Fixture::new();
    let ticket = fixture
        .store
        .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
        .unwrap();
    fixture.clock.advance(RESOURCE_TICKET_LIFETIME_MS);
    fixture.store.sweep();
    assert!(fixture.ends.take().is_empty());
    assert_eq!(
        fixture.store.activate(&ticket),
        Err((TicketEnd::Expired, McpAppInitiator::System))
    );
}

#[test]
fn each_end_is_recorded_as_whoever_caused_it() {
    let fixture = Fixture::new();
    let releaser = app_initiator();
    let released = issued(
        &fixture.store,
        held(CONVERSATION, app("call-1", "mount-1"), PAGE),
    )
    .unwrap();
    let ended = issued(
        &fixture.store,
        held(OTHER_CONVERSATION, app("call-2", "mount-2"), PAGE),
    )
    .unwrap();
    fixture.store.release_app(
        &conversation(CONVERSATION),
        &app("call-1", "mount-1"),
        &releaser,
    );
    fixture
        .store
        .release_conversation(&conversation(OTHER_CONVERSATION), &releaser);
    let records: Vec<_> = fixture
        .ends
        .take()
        .iter()
        .map(TicketEvent::audit_record)
        .map(|record| (record.phase, record.initiator))
        .collect();
    assert_eq!(
        records,
        [
            (
                McpAppAuditPhase::TicketEnded {
                    ticket_digest: ResourceTicketDigest::of(&released).to_hex(),
                    cause: TicketEnd::AppReleased,
                },
                releaser.clone()
            ),
            (
                McpAppAuditPhase::TicketEnded {
                    ticket_digest: ResourceTicketDigest::of(&ended).to_hex(),
                    cause: TicketEnd::ConversationEnded,
                },
                releaser
            ),
        ]
    );
}

#[tokio::test]
async fn a_stopping_recorder_records_every_end_already_sent() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let audit = Arc::new(RecordingAudit::default());
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let fixture = Fixture::new();
    let store = ResourceTicketStore::new(
        fixture.clock.clone(),
        fixture.random.clone(),
        Arc::new(sender),
    );
    for mount in ["mount-1", "mount-2", "mount-3"] {
        issued(&store, held(CONVERSATION, app("call-1", mount), PAGE)).unwrap();
    }
    // The conversation ends, then the gateway stops — before the recorder has
    // had a turn. The store, and so the channel's sender, lives on.
    store.release_conversation(&conversation(CONVERSATION), &McpAppInitiator::System);
    let _ = stop.send(());
    audit_ticket_ends(receiver, audit.clone(), stopping).await;
    assert_eq!(audit.take().len(), 3);
    drop(store);
}
