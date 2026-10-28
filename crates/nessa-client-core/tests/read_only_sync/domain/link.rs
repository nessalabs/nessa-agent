//! One test for each row the schedule claims, without a socket or a cache.
use super::*;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use std::num::NonZeroU64;

fn conversation(byte: u8) -> ConversationId {
    ConversationId::new(&format!("00000000-0000-4000-8000-0000000000{byte:02x}")).unwrap()
}

fn schedule() -> LinkSchedule {
    LinkSchedule::new(LinkSpec {
        route: Route::Direct,
        presence: Presence::Foreground,
        billing: Billing::Unmetered,
        pace_bytes_per_second: None,
        open: Some(conversation(1)),
        subscription: Subscription::Up,
    })
}

fn transcript() -> Candidate {
    Candidate {
        interest: Interest::OpenTranscript(conversation(1)),
        quote: Quote::Known(100),
    }
}

fn catalogue() -> Candidate {
    Candidate {
        interest: Interest::Catalogue,
        quote: Quote::Known(40),
    }
}

fn history(byte: u8) -> Candidate {
    Candidate {
        interest: Interest::OlderHistory(conversation(byte)),
        quote: Quote::Known(80),
    }
}

fn artifact() -> Candidate {
    Candidate {
        interest: Interest::Artifact {
            conversation: conversation(1),
            digest: Sha256Digest::from_bytes([9; 32]),
        },
        quote: Quote::Known(32),
    }
}

fn interest(admit: Admit) -> Interest {
    match admit {
        Admit::Go { interest, .. } => interest,
        other => panic!("expected an admission, got {other:?}"),
    }
}

#[test]
fn unreachable_outranks_sleep_reconnect_and_backpressure() {
    let paced = LinkSchedule::new(LinkSpec {
        pace_bytes_per_second: NonZeroU64::new(8_000),
        ..schedule_spec()
    });
    let (paced, _) = paced.admit(&[transcript()]);
    let (paced, _) = paced.finish(1);
    let (paced, admit) = paced
        .unreachable()
        .sleep()
        .connection_lost()
        .admit(&[transcript()]);
    assert_eq!(admit, Admit::Defer(Deferral::Unreachable));
    assert!(matches!(
        paced.reachable().admit(&[transcript()]).1,
        Admit::Defer(Deferral::Reconnect)
    ));
}

fn metered(remaining: u64) -> LinkSchedule {
    LinkSchedule::new(LinkSpec {
        billing: Billing::Metered { remaining },
        ..schedule_spec()
    })
}

fn closed_pace(kept: u64) -> LinkSchedule {
    let paced = LinkSchedule::new(LinkSpec {
        pace_bytes_per_second: NonZeroU64::new(8_000),
        ..schedule_spec()
    });
    let (paced, _) = paced.admit(&[transcript()]);
    let (paced, _) = paced.finish(kept);
    paced
}

fn closed_metered_pace() -> LinkSchedule {
    let paced = LinkSchedule::new(LinkSpec {
        billing: Billing::Metered { remaining: 100 },
        pace_bytes_per_second: NonZeroU64::new(8_000),
        ..schedule_spec()
    });
    let (paced, _) = paced.admit(&[transcript()]);
    let (paced, _) = paced.finish(1);
    paced
}

fn lost_during_copy() -> LinkSchedule {
    let (schedule, _) = schedule().admit(&[transcript()]);
    schedule.connection_lost()
}

fn asleep_and_unreachable() -> LinkSchedule {
    let (schedule, _) = schedule().admit(&[transcript()]);
    schedule.sleep().unreachable()
}

fn schedule_spec() -> LinkSpec {
    LinkSpec {
        route: Route::Direct,
        presence: Presence::Foreground,
        billing: Billing::Unmetered,
        pace_bytes_per_second: None,
        open: Some(conversation(1)),
        subscription: Subscription::Up,
    }
}

#[test]
fn deferral_order_is_unreachable_then_reconnect_then_sleep_then_pace() {
    let candidates = [transcript()];
    assert!(matches!(
        schedule().unreachable().admit(&candidates).1,
        Admit::Defer(Deferral::Unreachable)
    ));
    assert!(matches!(
        schedule().connection_lost().admit(&candidates).1,
        Admit::Defer(Deferral::Reconnect)
    ));
    assert!(matches!(
        schedule().sleep().admit(&candidates).1,
        Admit::Defer(Deferral::Sleep)
    ));
    assert!(matches!(
        schedule()
            .sleep()
            .wake(Presence::Foreground)
            .admit(&candidates)
            .1,
        Admit::Go { .. }
    ));
    assert!(matches!(
        schedule()
            .sleep()
            .wake(Presence::Sleep)
            .admit(&candidates)
            .1,
        Admit::Defer(Deferral::Sleep)
    ));
    let (paced, settled) = {
        let paced = LinkSchedule::new(LinkSpec {
            pace_bytes_per_second: NonZeroU64::new(8_000),
            ..schedule_spec()
        });
        let (paced, _) = paced.admit(&candidates);
        paced.finish(1)
    };
    assert!(matches!(settled, Settle::Settled { wait_ms: 1_000, .. }));
    let _ = paced;
    assert!(matches!(
        closed_pace(1).admit(&candidates).1,
        Admit::Defer(Deferral::Backpressure)
    ));
    assert!(matches!(
        closed_pace(1).pace_elapsed().admit(&candidates).1,
        Admit::Go { .. }
    ));

    let (metered, settled) = {
        let metered = LinkSchedule::new(LinkSpec {
            billing: Billing::Metered { remaining: 100 },
            pace_bytes_per_second: NonZeroU64::new(8_000),
            ..schedule_spec()
        });
        let (metered, admit) = metered.admit(&candidates);
        assert!(matches!(admit, Admit::Go { .. }));
        metered.finish(1)
    };
    assert!(matches!(settled, Settle::Settled { wait_ms: 1_000, .. }));
    let _ = metered;
    assert_eq!(
        closed_metered_pace().sleep().admit(&candidates).1,
        Admit::Defer(Deferral::Sleep)
    );
    let unknown = Candidate {
        quote: Quote::Unknown,
        ..transcript()
    };
    assert_eq!(
        closed_metered_pace().admit(&[unknown]).1,
        Admit::Defer(Deferral::Backpressure)
    );
}

#[test]
fn metered_unknown_quote_blocks_a_cheaper_candidate() {
    let metered = LinkSchedule::new(LinkSpec {
        billing: Billing::Metered { remaining: 50 },
        ..schedule_spec()
    });
    let unknown = Candidate {
        quote: Quote::Unknown,
        ..transcript()
    };
    let (metered, admit) = metered.admit(&[unknown, artifact()]);
    assert_eq!(admit, Admit::Defer(Deferral::UnknownQuote));
    assert_eq!(metered.remaining(), Some(50));
    let over = Candidate {
        quote: Quote::Known(51),
        ..transcript()
    };
    let (metered, admit) = metered.admit(&[over, artifact()]);
    assert_eq!(admit, Admit::Defer(Deferral::Allowance));
    assert_eq!(metered.remaining(), Some(50));
}

#[test]
fn finish_and_cancel_leave_the_allowance_minus_what_was_kept() {
    let quoted = Candidate {
        quote: Quote::Known(400),
        ..transcript()
    };
    let (admitted, _) = metered(2_000).admit(std::slice::from_ref(&quoted));
    assert_eq!(admitted.remaining(), Some(1_600));
    let (finished, _) = admitted.finish(100);
    assert_eq!(finished.remaining(), Some(1_900));

    let (admitted, _) = metered(2_000).admit(std::slice::from_ref(&quoted));
    let (cancelled, _) = admitted.cancel(100);
    assert_eq!(cancelled.remaining(), Some(1_900));

    let (admitted, _) = metered(2_000).admit(std::slice::from_ref(&quoted));
    let (cancelled, _) = admitted.cancel(0);
    assert_eq!(cancelled.remaining(), Some(2_000));

    let (admitted, _) = metered(2_000).admit(std::slice::from_ref(&quoted));
    let (over, _) = admitted.finish(900);
    assert_eq!(over.remaining(), Some(1_100));
    let (admitted, _) = metered(2_000).admit(std::slice::from_ref(&quoted));
    let (over, _) = admitted.finish(u64::MAX);
    assert_eq!(over.remaining(), Some(0));
    let raised = over.with_allowance(0).expect("idle");
    let huge = Candidate {
        quote: Quote::Known(1),
        ..transcript()
    };
    assert!(matches!(
        raised.admit(&[huge]).1,
        Admit::Defer(Deferral::Allowance)
    ));
}

#[test]
fn a_second_interest_is_in_flight_until_settle() {
    let (schedule, first) = schedule().admit(&[transcript(), artifact()]);
    assert!(matches!(first, Admit::Go { .. }));
    let (schedule, second) = schedule.admit(&[artifact()]);
    assert_eq!(second, Admit::InFlight);
    assert!(schedule.in_flight());
    let (schedule, _) = schedule.cancel(0);
    assert!(!schedule.in_flight());
    assert!(matches!(schedule.admit(&[artifact()]).1, Admit::Go { .. }));
}

#[test]
fn open_transcript_outranks_catalogue_history_and_artifact() {
    let candidates = [artifact(), history(2), catalogue(), transcript()];
    assert!(matches!(
        interest(schedule().admit(&candidates).1),
        Interest::OpenTranscript(_)
    ));
    let without_transcript = [artifact(), history(2), catalogue()];
    assert_eq!(
        interest(schedule().admit(&without_transcript).1),
        Interest::Catalogue
    );
    let history_only = [artifact(), history(2), history(1)];
    assert_eq!(
        interest(schedule().admit(&history_only).1),
        Interest::OlderHistory(conversation(2))
    );
    assert!(matches!(
        interest(schedule().admit(&[artifact()]).1),
        Interest::Artifact { .. }
    ));

    let down = schedule().subscription(Subscription::Down);
    let fallback = [transcript(), catalogue(), history(1), artifact()];
    assert_eq!(
        interest(down.admit(&fallback).1),
        Interest::OlderHistory(conversation(1))
    );
}

#[test]
fn loss_during_the_quantum_reconnects_after_cancel() {
    assert!(matches!(
        lost_during_copy().admit(&[transcript()]).1,
        Admit::InFlight
    ));
    let (schedule, settled) = lost_during_copy().cancel(25);
    assert!(matches!(
        settled,
        Settle::Settled {
            kept: 25,
            reconnecting: true,
            ..
        }
    ));
    assert!(matches!(
        schedule.admit(&[transcript()]).1,
        Admit::Defer(Deferral::Reconnect)
    ));
    let (schedule, admit) = lost_during_copy()
        .cancel(25)
        .0
        .backoff_elapsed()
        .admit(&[transcript()]);
    assert!(matches!(admit, Admit::Go { .. }));
    let _ = schedule;
}

#[test]
fn finish_from_idle_does_not_charge() {
    let (settled, outcome) = metered(10).finish(4);
    assert_eq!(outcome, Settle::Idle);
    assert_eq!(settled.remaining(), Some(10));
    let (settled, outcome) = metered(10).cancel(4);
    assert_eq!(outcome, Settle::Idle);
    assert_eq!(settled.remaining(), Some(10));
}

#[test]
fn relay_does_not_change_who_is_admitted() {
    let candidates = [artifact(), transcript()];
    let direct = interest(schedule().admit(&candidates).1);
    let relay = interest(schedule().relay().admit(&candidates).1);
    assert_eq!(direct, relay);
    assert_eq!(schedule().relay().route(), Route::Relay);
}

#[test]
fn pace_stays_closed_until_the_caller_says_so() {
    let (paced, settled) = {
        let paced = LinkSchedule::new(LinkSpec {
            pace_bytes_per_second: NonZeroU64::new(8_000),
            ..schedule_spec()
        });
        let (paced, _) = paced.admit(&[transcript()]);
        paced.finish(8_001)
    };
    assert!(matches!(settled, Settle::Settled { wait_ms: 2_000, .. }));
    let _ = paced;
    assert!(matches!(
        closed_pace(8_001).admit(&[transcript()]).1,
        Admit::Defer(Deferral::Backpressure)
    ));
    let (open, _) = closed_pace(8_001).pace_elapsed().admit(&[transcript()]);
    let (open, settled) = open.finish(0);
    assert!(matches!(settled, Settle::Settled { wait_ms: 0, .. }));
    assert!(matches!(open.admit(&[transcript()]).1, Admit::Go { .. }));
}

#[test]
fn allowance_is_refused_while_a_quantum_is_in_flight() {
    let metered = LinkSchedule::new(LinkSpec {
        billing: Billing::Metered { remaining: 100 },
        ..schedule_spec()
    });
    let (admitted, _) = metered.admit(&[transcript()]);
    let refused = admitted.with_allowance(1).expect_err("in flight");
    assert!(refused.in_flight());
    assert_eq!(refused.remaining(), Some(0));
    assert!(schedule().with_allowance(1).is_err());
}

#[test]
fn foreground_and_background_share_one_order() {
    let candidates = [artifact(), catalogue(), transcript()];
    let foreground = interest(schedule().admit(&candidates).1);
    let background = interest(schedule().wake(Presence::Background).admit(&candidates).1);
    assert_eq!(foreground, background);
}

#[test]
fn an_unmetered_link_admits_an_unknown_quote() {
    let unknown = Candidate {
        quote: Quote::Unknown,
        ..transcript()
    };
    assert!(matches!(
        schedule().admit(&[unknown]).1,
        Admit::Go { reserved: 0, .. }
    ));
}

#[test]
fn no_candidate_copies_nothing() {
    assert_eq!(schedule().admit(&[]).1, Admit::None);
    let down = schedule().subscription(Subscription::Down);
    assert_eq!(down.admit(&[transcript()]).1, Admit::None);
}

#[test]
fn sleep_or_unreachable_during_a_quantum_applies_at_the_next_admit() {
    assert!(matches!(
        asleep_and_unreachable().admit(&[transcript()]).1,
        Admit::InFlight
    ));
    let (schedule, settled) = asleep_and_unreachable().finish(5);
    assert!(matches!(
        settled,
        Settle::Settled {
            reconnecting: false,
            ..
        }
    ));
    assert!(matches!(
        schedule.admit(&[transcript()]).1,
        Admit::Defer(Deferral::Unreachable)
    ));
    let (schedule, _) = asleep_and_unreachable().finish(5);
    assert!(matches!(
        schedule.reachable().admit(&[transcript()]).1,
        Admit::Defer(Deferral::Sleep)
    ));
}
