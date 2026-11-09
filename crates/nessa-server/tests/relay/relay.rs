use super::super::wire::read_frame;
use super::*;
use crate::relay::infrastructure::Ed25519Proofs;
use crate::timer::{manual::ManualTimer, Moment};
use nessa_auth::{adapters::pairing::NativeIdentity, application::pairing::PrivateKeyMaterial};
use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
use zeroize::Zeroizing;

/// Fresh-looking bytes: every fill is a new counter value, so challenges
/// and tokens never repeat.
#[derive(Default)]
struct Counting(AtomicU8);
impl RelayEntropy for Counting {
    fn fill(&self, bytes: &mut [u8]) {
        let next = self.0.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        bytes.fill(next);
    }
}

struct Fixture {
    relay: Arc<Relay<DuplexStream>>,
    timer: Arc<ManualTimer>,
}

fn fixture(limits: RelayLimits) -> Fixture {
    let timer = Arc::new(ManualTimer::default());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(Counting::default()),
        Arc::new(Ed25519Proofs),
        limits,
    );
    Fixture { relay, timer }
}

fn small() -> RelayLimits {
    RelayLimits {
        connections: 16,
        registrations: 2,
        waiting_per_gateway: 2,
        joined: 4,
    }
}

fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}

impl Fixture {
    /// A new connection to the relay, as a peer sees it.
    fn open(&self) -> DuplexStream {
        let (peer, relay_end) = duplex(64 * 1024);
        let permit = self.relay.admit().expect("under the connection limit");
        tokio::spawn(self.relay.clone().handle(relay_end, permit));
        peer
    }
    /// A proved serve connection for `gateway`.
    async fn register(&self, gateway: &NativeIdentity) -> DuplexStream {
        let mut serve = self.open();
        send(
            &mut serve,
            RelayFrame::Serve {
                key: gateway.public_key(),
            },
        )
        .await;
        let RelayFrame::Challenge { nonce } = receive(&mut serve).await else {
            panic!("no challenge");
        };
        let signature = gateway.sign_relay_challenge(&nonce).unwrap();
        send(&mut serve, RelayFrame::Proof { signature }).await;
        assert_eq!(receive(&mut serve).await, RelayFrame::Registered);
        serve
    }
    /// A reader asking for `key`.
    async fn connect(&self, key: GatewayKey) -> DuplexStream {
        let mut reader = self.open();
        send(&mut reader, RelayFrame::Connect { key }).await;
        reader
    }
    /// The gateway's side of taking the reader it was told of on `serve`.
    async fn accept(&self, serve: &mut DuplexStream, key: GatewayKey) -> DuplexStream {
        let RelayFrame::Incoming { token } = receive(serve).await else {
            panic!("no incoming reader");
        };
        let mut accepted = self.open();
        send(&mut accepted, RelayFrame::Accept { key, token }).await;
        accepted
    }
}

async fn send(stream: &mut DuplexStream, frame: RelayFrame) {
    stream.write_all(&frame.encode()).await.unwrap();
}

async fn receive(stream: &mut DuplexStream) -> RelayFrame {
    tokio::time::timeout(Duration::from_secs(5), read_frame(stream))
        .await
        .expect("an answer in time")
        .expect("a frame")
}

/// The relay closed this connection: it reads to its end.
async fn closed(stream: &mut DuplexStream) {
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest))
        .await
        .expect("closed in time")
        .expect("read to the end");
    assert!(rest.is_empty(), "nothing more was sent: {rest:?}");
}

#[tokio::test]
async fn rl1_rl6_a_reader_is_joined_to_its_registered_gateway_and_their_bytes_cross_unread() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut reader = fixture.connect(key).await;
    let mut accepted = fixture.accept(&mut serve, key).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);

    // Bytes that are not frames cross untouched, both ways.
    reader.write_all(b"\x16\x03\x01client hello").await.unwrap();
    let mut got = [0; 15];
    accepted.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"\x16\x03\x01client hello");
    accepted.write_all(b"server hello").await.unwrap();
    let mut got = [0; 12];
    reader.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"server hello");

    // Either side going ends the pair: the other is closed.
    drop(reader);
    closed(&mut accepted).await;
    // The registration outlives the pair.
    let _next = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut serve).await,
        RelayFrame::Incoming { .. }
    ));
}

#[tokio::test]
async fn rl6_a_pair_ended_from_the_gateway_side_closes_the_reader_and_gives_back_its_places() {
    let fixture = fixture(RelayLimits {
        connections: 3,
        joined: 1,
        ..small()
    });
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut reader = fixture.connect(key).await;
    let mut accepted = fixture.accept(&mut serve, key).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);
    // Both legs of the pair hold a connection place, the gateway's too.
    assert!(fixture.relay.admit().is_none());

    drop(accepted);
    closed(&mut reader).await;
    // Nothing kept: the joined slot and both connection places come back,
    // so a second pair forms through the same small limits.
    let mut second = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture.relay.connections.available_permits() == 2
                && fixture.relay.joined.available_permits() == 1
            {
                return fixture.connect(key).await;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the pair's places came back");
    let mut accepted = fixture.accept(&mut serve, key).await;
    assert_eq!(receive(&mut second).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);
}

#[tokio::test]
async fn rl4_a_reader_asking_for_a_gateway_with_no_registration_is_told_offline() {
    let fixture = fixture(small());
    let mut reader = fixture.connect(identity(1).public_key()).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Offline);
    closed(&mut reader).await;
}

#[tokio::test]
async fn rl4b_a_registered_gateway_that_does_not_accept_in_time_is_no_answer_never_offline() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut reader = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut serve).await,
        RelayFrame::Incoming { .. }
    ));
    let join = fixture
        .timer
        .started(|_, wait| wait.limit() == JOIN_DEADLINE)
        .await;
    fixture.timer.advance_to(join.deadline);
    assert_eq!(receive(&mut reader).await, RelayFrame::NoAnswer);
    closed(&mut reader).await;
    // Still registered: the next reader reaches it.
    let _next = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut serve).await,
        RelayFrame::Incoming { .. }
    ));
}

#[tokio::test]
async fn rl4b_a_reader_that_gave_up_leaves_nothing_waiting_and_its_token_takes_no_one() {
    let fixture = fixture(RelayLimits {
        waiting_per_gateway: 1,
        ..small()
    });
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut reader = fixture.connect(key).await;
    let RelayFrame::Incoming { token } = receive(&mut serve).await else {
        panic!("no incoming reader");
    };
    let join = fixture
        .timer
        .started(|_, wait| wait.limit() == JOIN_DEADLINE)
        .await;
    fixture.timer.advance_to(join.deadline);
    assert_eq!(receive(&mut reader).await, RelayFrame::NoAnswer);
    closed(&mut reader).await;
    // Its waiting place is free again for the next reader.
    let mut next = fixture.connect(key).await;
    let RelayFrame::Incoming { token: theirs } = receive(&mut serve).await else {
        panic!("no incoming reader");
    };
    // A late accept for the first is closed and takes nobody, the next
    // reader included.
    let mut late = fixture.open();
    send(&mut late, RelayFrame::Accept { key, token }).await;
    closed(&mut late).await;
    let mut accepted = fixture.open();
    send(&mut accepted, RelayFrame::Accept { key, token: theirs }).await;
    assert_eq!(receive(&mut next).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);
}

#[tokio::test]
async fn rl2_a_proof_for_another_key_or_another_challenge_registers_nothing() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();

    // Signed by another key.
    let mut forged = fixture.open();
    send(&mut forged, RelayFrame::Serve { key }).await;
    let RelayFrame::Challenge { nonce } = receive(&mut forged).await else {
        panic!("no challenge");
    };
    let signature = identity(2).sign_relay_challenge(&nonce).unwrap();
    send(&mut forged, RelayFrame::Proof { signature }).await;
    closed(&mut forged).await;
    let mut reader = fixture.connect(key).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Offline);

    // The right key, but last connection's challenge: a replayed proof.
    let mut replayed = fixture.open();
    send(&mut replayed, RelayFrame::Serve { key }).await;
    assert!(matches!(
        receive(&mut replayed).await,
        RelayFrame::Challenge { .. }
    ));
    let signature = gateway.sign_relay_challenge(&nonce).unwrap();
    send(&mut replayed, RelayFrame::Proof { signature }).await;
    closed(&mut replayed).await;

    // A refused proof leaves the key's real registration as it was.
    let mut serve = fixture.register(&gateway).await;
    let mut bad = fixture.open();
    send(&mut bad, RelayFrame::Serve { key }).await;
    assert!(matches!(
        receive(&mut bad).await,
        RelayFrame::Challenge { .. }
    ));
    send(&mut bad, RelayFrame::Proof { signature: [0; 64] }).await;
    closed(&mut bad).await;
    let _reader = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut serve).await,
        RelayFrame::Incoming { .. }
    ));
}

#[tokio::test]
async fn rl3_the_newest_proved_serve_connection_takes_the_key_and_the_first_is_closed() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut first = fixture.register(&gateway).await;
    let mut second = fixture.register(&gateway).await;
    closed(&mut first).await;
    let _reader = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut second).await,
        RelayFrame::Incoming { .. }
    ));
    // The first going did not take the second's registration with it.
    let _another = fixture.connect(key).await;
    assert!(matches!(
        receive(&mut second).await,
        RelayFrame::Incoming { .. }
    ));
}

/// A lookup queued for the registration that replaced another survives the
/// old registration's cleanup. That cleanup can run after the new lookup is
/// already in the table.
#[tokio::test]
async fn a_reconnected_gateway_keeps_a_lookup_queued_for_the_new_registration() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut first = fixture.register(&gateway).await;
    let old = fixture
        .relay
        .lock()
        .registrations
        .get(&key)
        .expect("the first registration")
        .id;
    let mut second = fixture.register(&gateway).await;
    closed(&mut first).await;
    let mut phone = fixture.open();
    send(&mut phone, RelayFrame::Lookup { key }).await;
    assert!(
        matches!(receive(&mut phone).await, RelayFrame::Challenge { .. }),
        "the live registration took the lookup"
    );
    // The first task has exited. Run its cleanup again, as it does when it
    // loses the race and exits after this lookup was queued.
    drop(super::Registered {
        relay: &fixture.relay,
        key,
        id: old,
    });
    send(&mut phone, RelayFrame::Seal { mac: [0; 32] }).await;
    let RelayFrame::Admit { token, .. } = receive(&mut second).await else {
        panic!("the new registration was not asked the lookup");
    };
    send(&mut second, RelayFrame::Allow { token }).await;
    assert!(
        matches!(receive(&mut phone).await, RelayFrame::Located { .. }),
        "the lookup was not dropped with the old registration"
    );
}

#[tokio::test]
async fn rl5_an_accept_naming_no_waiting_reader_is_closed_and_the_reader_still_joins() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut reader = fixture.connect(key).await;
    let RelayFrame::Incoming { token } = receive(&mut serve).await else {
        panic!("no incoming reader");
    };

    // A token the relay never sent.
    let mut guessed = fixture.open();
    send(
        &mut guessed,
        RelayFrame::Accept {
            key,
            token: [0xee; 16],
        },
    )
    .await;
    closed(&mut guessed).await;
    // The right token for another key.
    let mut other = fixture.open();
    let other_key = identity(2).public_key();
    send(
        &mut other,
        RelayFrame::Accept {
            key: other_key,
            token,
        },
    )
    .await;
    closed(&mut other).await;

    let mut accepted = fixture.open();
    send(&mut accepted, RelayFrame::Accept { key, token }).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);

    // Used once: the same token again is closed.
    let mut again = fixture.open();
    send(&mut again, RelayFrame::Accept { key, token }).await;
    closed(&mut again).await;
}

#[tokio::test]
async fn rl7_a_serve_connection_that_stops_answering_pings_is_dropped_and_its_key_goes_offline() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;

    // Answered pings keep it registered, however many.
    for round in 1..=4u32 {
        let due = Moment::from_origin(PING_INTERVAL * round);
        fixture.timer.started(|_, wait| wait.deadline == due).await;
        fixture.timer.advance_to(due);
        assert_eq!(receive(&mut serve).await, RelayFrame::Ping);
        send(&mut serve, RelayFrame::Pong).await;
    }
    // From here on, silence: three pings unanswered, then it is dropped at
    // the fourth interval.
    let start = PING_INTERVAL * 4;
    for round in 1..=MISSED_PINGS {
        let due = Moment::from_origin(start + PING_INTERVAL * round);
        fixture.timer.started(|_, wait| wait.deadline == due).await;
        fixture.timer.advance_to(due);
        assert_eq!(receive(&mut serve).await, RelayFrame::Ping);
    }
    let due = Moment::from_origin(start + PING_INTERVAL * (MISSED_PINGS + 1));
    fixture.timer.started(|_, wait| wait.deadline == due).await;
    fixture.timer.advance_to(due);
    closed(&mut serve).await;
    let mut reader = fixture.connect(key).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Offline);
}

#[tokio::test]
async fn rl7_a_serve_connection_that_sends_anything_but_pong_is_dropped() {
    let fixture = fixture(small());
    let gateway = identity(1);
    let mut serve = fixture.register(&gateway).await;
    send(&mut serve, RelayFrame::Joined).await;
    closed(&mut serve).await;
    let mut reader = fixture.connect(gateway.public_key()).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Offline);
}

#[tokio::test]
async fn rl8_a_malformed_or_unexpected_first_frame_is_closed() {
    let fixture = fixture(small());
    let mut wrong_version = fixture.open();
    wrong_version.write_all(&[9, 1, 32]).await.unwrap();
    closed(&mut wrong_version).await;
    let mut unknown = fixture.open();
    unknown.write_all(&[1, 99, 0]).await.unwrap();
    closed(&mut unknown).await;
    let mut out_of_place = fixture.open();
    send(&mut out_of_place, RelayFrame::Pong).await;
    closed(&mut out_of_place).await;
}

#[tokio::test]
async fn rl8_a_preamble_or_a_proof_slower_than_its_deadline_is_closed() {
    let fixture = fixture(small());
    let mut silent = fixture.open();
    let wait = fixture
        .timer
        .started(|_, wait| wait.limit() == PREAMBLE_DEADLINE)
        .await;
    fixture.timer.advance_to(wait.deadline);
    closed(&mut silent).await;

    // The proof is due by the same deadline the first frame was.
    let gateway = identity(1);
    let mut slow = fixture.open();
    send(
        &mut slow,
        RelayFrame::Serve {
            key: gateway.public_key(),
        },
    )
    .await;
    assert!(matches!(
        receive(&mut slow).await,
        RelayFrame::Challenge { .. }
    ));
    let deadline = Moment::from_origin(wait.deadline.since_origin() + PREAMBLE_DEADLINE);
    fixture.timer.advance_to(deadline);
    closed(&mut slow).await;
    let mut reader = fixture.connect(gateway.public_key()).await;
    assert_eq!(receive(&mut reader).await, RelayFrame::Offline);
}

#[tokio::test]
async fn rl8_past_the_connection_limit_a_connection_is_refused_and_the_others_kept() {
    let fixture = fixture(RelayLimits {
        connections: 2,
        ..small()
    });
    let gateway = identity(1);
    let mut serve = fixture.register(&gateway).await;
    let _one = fixture.open();
    assert!(fixture.relay.admit().is_none());
    drop(_one);
    // Its permit comes back once that connection is gone.
    let admitted = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(permit) = fixture.relay.admit() {
                return permit;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the permit came back");
    drop(admitted);
    let _reader = fixture.connect(gateway.public_key()).await;
    assert!(matches!(
        receive(&mut serve).await,
        RelayFrame::Incoming { .. }
    ));
}

#[tokio::test]
async fn rl8_past_the_registration_limit_a_new_key_is_closed_and_a_known_one_still_replaces() {
    let fixture = fixture(RelayLimits {
        registrations: 1,
        ..small()
    });
    let first = identity(1);
    let mut serve = fixture.register(&first).await;
    let second = identity(2);
    let mut refused = fixture.open();
    send(
        &mut refused,
        RelayFrame::Serve {
            key: second.public_key(),
        },
    )
    .await;
    let RelayFrame::Challenge { nonce } = receive(&mut refused).await else {
        panic!("no challenge");
    };
    let signature = second.sign_relay_challenge(&nonce).unwrap();
    send(&mut refused, RelayFrame::Proof { signature }).await;
    closed(&mut refused).await;
    // The registered key may still take its own place again.
    let mut replacing = fixture.register(&first).await;
    closed(&mut serve).await;
    let _reader = fixture.connect(first.public_key()).await;
    assert!(matches!(
        receive(&mut replacing).await,
        RelayFrame::Incoming { .. }
    ));
}

#[tokio::test]
async fn rl8_past_the_waiting_or_joined_limit_a_reader_is_no_answer_and_nothing_is_left_waiting() {
    let fixture = fixture(RelayLimits {
        waiting_per_gateway: 1,
        ..small()
    });
    let gateway = identity(1);
    let key = gateway.public_key();
    let mut serve = fixture.register(&gateway).await;
    let mut first = fixture.connect(key).await;
    let RelayFrame::Incoming { token } = receive(&mut serve).await else {
        panic!("no incoming reader");
    };
    let mut second = fixture.connect(key).await;
    assert_eq!(receive(&mut second).await, RelayFrame::NoAnswer);
    closed(&mut second).await;
    // The first still joins.
    let mut accepted = fixture.open();
    send(&mut accepted, RelayFrame::Accept { key, token }).await;
    assert_eq!(receive(&mut first).await, RelayFrame::Joined);

    // With every joined slot taken, a reader is no answer too.
    let fixture = self::fixture(RelayLimits {
        joined: 1,
        ..small()
    });
    let mut serve = fixture.register(&gateway).await;
    let mut held = fixture.connect(key).await;
    let mut accepted = fixture.accept(&mut serve, key).await;
    assert_eq!(receive(&mut held).await, RelayFrame::Joined);
    assert_eq!(receive(&mut accepted).await, RelayFrame::Joined);
    let mut over = fixture.connect(key).await;
    assert_eq!(receive(&mut over).await, RelayFrame::NoAnswer);
}
