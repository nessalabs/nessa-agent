//! The runner calls the quantum only when the schedule admits it.
use super::*;
use crate::read_only_sync::domain::{
    Billing, Deferral, Interest, LinkSpec, Presence, Quote, Route, Subscription,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::catalogue::MAX_CATALOGUE_PAYLOAD_BYTES;
use std::cell::Cell;
use std::num::NonZeroU64;

fn conversation() -> ConversationId {
    ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap()
}

fn spec() -> LinkSpec {
    LinkSpec {
        route: Route::Direct,
        presence: Presence::Foreground,
        billing: Billing::Unmetered,
        pace_bytes_per_second: None,
        open: Some(conversation()),
        subscription: Subscription::Up,
    }
}

fn transcript(quote: Quote) -> Candidate {
    Candidate {
        interest: Interest::OpenTranscript(conversation()),
        quote,
    }
}

#[test]
fn unreachable_sleep_reconnect_and_backpressure_copy_nothing() {
    let calls = Cell::new(0);
    let mut quantum = |_interest: &Interest| {
        calls.set(calls.get() + 1);
        1
    };
    let metered = || {
        LinkSchedule::new(LinkSpec {
            billing: Billing::Metered { remaining: 10 },
            ..spec()
        })
    };
    for schedule in [
        metered().unreachable(),
        metered().connection_lost(),
        metered().sleep(),
    ] {
        let (_, admit, settled) =
            run_quantum(schedule, &[transcript(Quote::Known(4))], &mut quantum);
        assert!(matches!(admit, Admit::Defer(_)), "{admit:?}");
        assert!(settled.is_none());
    }
    assert_eq!(calls.get(), 0);

    let (_, admit, settled) = run_quantum(
        metered(),
        &[
            transcript(Quote::Unknown),
            Candidate {
                interest: Interest::Catalogue,
                quote: Quote::Known(4),
            },
        ],
        &mut quantum,
    );
    assert_eq!(admit, Admit::Defer(Deferral::UnknownQuote));
    assert!(settled.is_none());
    assert_eq!(calls.get(), 0);

    let paced = LinkSchedule::new(LinkSpec {
        pace_bytes_per_second: NonZeroU64::new(8_000),
        ..spec()
    });
    let (paced, admit, _) = run_quantum(paced, &[transcript(Quote::Known(4))], &mut quantum);
    assert!(matches!(admit, Admit::Go { .. }));
    assert_eq!(calls.get(), 1);
    let (_, admit, settled) = run_quantum(paced, &[transcript(Quote::Known(4))], &mut quantum);
    assert_eq!(admit, Admit::Defer(Deferral::Backpressure));
    assert!(settled.is_none());
    assert_eq!(calls.get(), 1);
}

#[test]
fn an_admission_runs_the_quantum_once_and_charges_what_it_kept() {
    let calls = Cell::new(0);
    let schedule = LinkSchedule::new(LinkSpec {
        billing: Billing::Metered { remaining: 100 },
        ..spec()
    });
    let (schedule, admit, settled) = run_quantum(
        schedule,
        &[transcript(Quote::Known(40))],
        &mut |_interest| {
            calls.set(calls.get() + 1);
            15
        },
    );
    assert!(matches!(admit, Admit::Go { reserved: 40, .. }));
    assert!(matches!(settled, Some(Settle::Settled { kept: 15, .. })));
    assert_eq!(calls.get(), 1);
    assert_eq!(schedule.remaining(), Some(85));
}

#[test]
fn the_conservative_quote_is_the_published_ceiling() {
    let page = 64 * 1024;
    assert_eq!(
        conservative_quote(&Interest::OpenTranscript(conversation()), page),
        Quote::Known(page)
    );
    assert_eq!(
        conservative_quote(&Interest::OlderHistory(conversation()), page),
        Quote::Known(page)
    );
    assert_eq!(
        conservative_quote(&Interest::Catalogue, page),
        Quote::Known(u64::try_from(MAX_CATALOGUE_PAYLOAD_BYTES).unwrap())
    );
    assert_eq!(
        conservative_quote(
            &Interest::Artifact {
                conversation: conversation(),
                digest: nessa_sdk::domain::common::value_objects::Sha256Digest::from_bytes([1; 32]),
            },
            page
        ),
        Quote::Known(HELD_FILE_RANGE_BYTES)
    );
}
