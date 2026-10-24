//! How one read of a paired reader's cache walks the cached conversations,
//! and how it ends: a full cache still withdraws what the gateway no longer
//! grants, and a read its budget cut short counts only if it saved something.
use super::*;
use nessa_protocol::relay::RelayFrame;

#[test]
fn connection_reasons_keep_their_retained_failure_and_status_policy() {
    use nessa_protocol::product_contract::generated::SessionCloseReason;
    for (reason, expected, status) in [
        (
            SessionCloseReason::HandshakeTimeout,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::TemporaryUnavailable,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::GatewayRestarting,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::GatewayOverloaded,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::TransportInterrupted,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::ServerShutdown,
            ReadFailure::Unreachable,
            false,
        ),
        (
            SessionCloseReason::ProtocolIncompatible,
            ReadFailure::Protocol,
            false,
        ),
        (
            SessionCloseReason::AuthenticationFailed,
            ReadFailure::Refused,
            true,
        ),
        (
            SessionCloseReason::CredentialRevoked,
            ReadFailure::Refused,
            true,
        ),
        (
            SessionCloseReason::CredentialExpired,
            ReadFailure::Refused,
            true,
        ),
        (
            SessionCloseReason::AuthorizationLost,
            ReadFailure::Refused,
            true,
        ),
    ] {
        for error in [
            GatewayError::Authentication(reason),
            GatewayError::Closed(Some(reason)),
        ] {
            assert_eq!(failure(error), expected, "{error:?}");
            assert_eq!(asks_status(error), status, "{error:?}");
            assert_eq!(unread(error), Err(expected), "records {error:?}");
        }
    }
    assert_eq!(
        failure(GatewayError::Closed(None)),
        ReadFailure::Unreachable
    );
    assert!(!asks_status(GatewayError::Closed(None)));
    assert_eq!(
        unread(GatewayError::Closed(None)),
        Err(ReadFailure::Unreachable)
    );
    assert_eq!(
        unread(GatewayError::Record(RecordReadErrorCode::SourcePreparing)),
        Ok(Conversation::Unread {
            connection_kept: true
        }),
    );
    assert_eq!(
        unread(GatewayError::Protocol),
        Ok(Conversation::Unread {
            connection_kept: false
        })
    );
}

#[test]
fn native_protocol_termination_during_records_ends_the_refresh() {
    use crate::read_only_sync::infrastructure::gateway::tests::session::{
        connect, peer, request, send,
    };
    use nessa_protocol::product::generated::{product_event, product_method, SessionTermination};
    use nessa_protocol::product::passive_read::wire_scope;
    use nessa_protocol::product_contract::generated::SessionCloseReason;
    use nessa_protocol::protocol::{EventFrame, OutgoingMessage, ResponseFrame};
    use serde_json::json;

    struct Wall;
    impl WallClock for Wall {
        fn unix_milliseconds(&self) -> u64 {
            123_000
        }
    }
    struct Time(Instant);
    impl MonotonicClock for Time {
        fn elapsed_ms(&self) -> u64 {
            self.0.elapsed().as_millis() as u64
        }
    }
    for during_page in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("cache");
        nessa_local_storage::create_directory(&private).unwrap();
        let mut cache = RetainedCache::open(
            &private.join("cache.sqlite3"),
            Arc::new(Wall),
            Arc::new(Time(Instant::now())),
        )
        .unwrap();
        let conversation = ConversationId::new("8e024fc9-0c9d-4952-8427-bcb2b3b07f8f").unwrap();
        let scope = Scope::new(
            Id::new("receiver").unwrap(),
            Id::new("gateway").unwrap(),
            Id::new(conversation.to_string()).unwrap(),
            Id::new("incarnation").unwrap(),
            nessa_sdk::infrastructure::session_storage::physical_record_schema(),
            Id::new("epoch-3").unwrap(),
        );
        let (endpoint, peer) = peer(move |socket| loop {
            let reading = request(socket);
            if during_page && reading.method == product_method::CONVERSATION_RECORDS_HEAD {
                send(
                    socket,
                    OutgoingMessage::Response(
                        ResponseFrame::success(
                            &reading.id,
                            &json!({"scope":wire_scope(&scope),"head":"1"}),
                        )
                        .unwrap(),
                    ),
                );
                continue;
            }
            assert_eq!(
                reading.method,
                if during_page {
                    product_method::CONVERSATION_RECORDS_PAGE
                } else {
                    product_method::CONVERSATION_RECORDS_HEAD
                }
            );
            send(
                socket,
                OutgoingMessage::Event(
                    EventFrame::push(
                        product_event::SESSION_TERMINATED,
                        &SessionTermination {
                            code: SessionCloseReason::ProtocolIncompatible,
                            retryable: false,
                            retry_after_ms: None,
                        },
                        0,
                        0,
                    )
                    .unwrap(),
                ),
            );
            break;
        });
        let connection = GatewayConnection::new(connect(&endpoint));
        assert_eq!(
            cache.records(
                &connection,
                &Id::new("receiver").unwrap(),
                3,
                conversation,
                false
            ),
            Err(ReadFailure::Protocol),
            "during page: {during_page}"
        );
        drop(connection);
        drop(cache);
        peer.join().unwrap();
    }
}

/// A walk over conversations whose answers the test sets, keeping what the
/// walk asked of each.
struct Scripted {
    answers: Vec<(&'static str, Result<Conversation, ReadFailure>)>,
    withdraw_fails: Option<ReadFailure>,
    /// Each read asked: the conversation and whether it was withdraw-only.
    asked: Vec<(String, bool)>,
    withdrawn: Vec<String>,
    spent_after: usize,
}
impl Scripted {
    fn new(answers: Vec<(&'static str, Result<Conversation, ReadFailure>)>) -> Self {
        Self {
            answers,
            withdraw_fails: None,
            asked: Vec::new(),
            withdrawn: Vec::new(),
            spent_after: usize::MAX,
        }
    }
    fn ids(&self) -> Vec<Id> {
        self.answers
            .iter()
            .map(|(id, _)| Id::new(*id).unwrap())
            .collect()
    }
}
impl EachConversation for Scripted {
    fn spent(&self) -> bool {
        self.asked.len() >= self.spent_after
    }
    fn read(&mut self, id: &Id, withdraw_only: bool) -> Result<Conversation, ReadFailure> {
        self.asked.push((id.as_str().to_owned(), withdraw_only));
        let answer = self
            .answers
            .iter()
            .find(|(each, _)| *each == id.as_str())
            .map(|(_, answer)| answer.clone())
            .unwrap();
        match answer {
            // A withdraw-only ask never reads records.
            Ok(Conversation::Read { .. }) if withdraw_only => Ok(Conversation::Unread {
                connection_kept: true,
            }),
            answer => answer,
        }
    }
    fn withdraw(&mut self, id: &Id) -> Result<(), ReadFailure> {
        if let Some(failure) = self.withdraw_fails {
            return Err(failure);
        }
        self.withdrawn.push(id.as_str().to_owned());
        Ok(())
    }
}
fn walk(scripted: &mut Scripted, held_back: Option<ReadFailure>) -> Result<bool, ReadFailure> {
    let mut withdrawn = Vec::new();
    let ids = scripted.ids();
    let result = read_each(ids, held_back, usize::MAX, scripted, &mut withdrawn);
    assert_eq!(withdrawn, scripted.withdrawn, "reported as withdrawn");
    result
}

#[test]
fn a_full_cache_still_withdraws_what_the_gateway_no_longer_grants() {
    // A is over quota; B, after it, is no longer granted; C is still granted.
    let mut scripted = Scripted::new(vec![
        ("a", Err(ReadFailure::Quota)),
        ("b", Ok(Conversation::Withdrawn)),
        ("c", Ok(Conversation::Read { complete: true })),
    ]);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Quota));
    assert_eq!(scripted.withdrawn, ["b"], "B is withdrawn past A's quota");
    // Once full, the rest are only asked whether they are still granted.
    assert_eq!(
        scripted.asked,
        [
            ("a".to_owned(), false),
            ("b".to_owned(), true),
            ("c".to_owned(), true)
        ]
    );
}

#[test]
fn a_full_catalogue_still_withdraws_and_quota_outranks_a_storage_failure() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Err(ReadFailure::Cache)),
        ("c", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(
        walk(&mut scripted, Some(ReadFailure::Quota)),
        Err(ReadFailure::Quota)
    );
    assert_eq!(scripted.withdrawn, ["a", "c"]);
    assert!(scripted.asked.iter().all(|(_, only)| *only));

    // A storage failure alone is reported as such, after the withdrawals.
    let mut scripted = Scripted::new(vec![
        ("a", Err(ReadFailure::Cache)),
        ("b", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Cache));
    assert_eq!(scripted.withdrawn, ["b"]);
}

#[test]
fn a_withdrawal_that_cannot_be_written_does_not_stop_the_others() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    scripted.withdraw_fails = Some(ReadFailure::Cache);
    assert_eq!(walk(&mut scripted, None), Err(ReadFailure::Cache));
    assert_eq!(
        scripted.asked,
        [("a".to_owned(), false), ("b".to_owned(), true)]
    );
}

#[test]
fn a_damaged_cache_or_a_lost_connection_ends_the_walk_at_once() {
    for failure in [
        ReadFailure::CacheDamaged,
        ReadFailure::CacheUncertain,
        ReadFailure::ResetRequired,
        ReadFailure::Unreachable,
        ReadFailure::Refused,
        ReadFailure::Stopped,
        ReadFailure::Protocol,
    ] {
        let mut scripted = Scripted::new(vec![
            ("a", Err(failure)),
            ("b", Ok(Conversation::Withdrawn)),
        ]);
        assert_eq!(walk(&mut scripted, None), Err(failure));
        assert_eq!(scripted.asked.len(), 1, "{failure:?}");
    }
}

#[test]
fn a_walk_reports_whether_every_conversation_was_read_to_the_end() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: true })),
        ("b", Ok(Conversation::Withdrawn)),
    ]);
    assert_eq!(walk(&mut scripted, None), Ok(true));
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: false })),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    assert_eq!(walk(&mut scripted, None), Ok(false));
    // No conversation starts once the budget is spent.
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Read { complete: true })),
        ("b", Ok(Conversation::Read { complete: true })),
    ]);
    scripted.spent_after = 1;
    assert_eq!(walk(&mut scripted, None), Ok(false));
    assert_eq!(scripted.asked.len(), 1);
}

/// Row m3: a full cache stays listed full when the walk past it is then
/// stopped or loses its connection; a damaged cache or a refusal still wins.
#[test]
fn a_held_quota_outlasts_a_stop_or_a_lost_connection() {
    for cut in [ReadFailure::Stopped, ReadFailure::Unreachable] {
        let mut scripted = Scripted::new(vec![
            ("a", Err(ReadFailure::Quota)),
            ("b", Ok(Conversation::Withdrawn)),
            ("c", Err(cut)),
        ]);
        assert_eq!(
            walk(&mut scripted, None),
            Err(ReadFailure::Quota),
            "{cut:?}"
        );
        assert_eq!(scripted.withdrawn, ["b"]);
        // A catalogue held back the same way.
        let mut scripted = Scripted::new(vec![("a", Err(cut))]);
        assert_eq!(
            walk(&mut scripted, Some(ReadFailure::Quota)),
            Err(ReadFailure::Quota)
        );
    }
    for failure in [ReadFailure::CacheDamaged, ReadFailure::Refused] {
        let mut scripted = Scripted::new(vec![("a", Err(ReadFailure::Quota)), ("b", Err(failure))]);
        assert_eq!(walk(&mut scripted, None), Err(failure));
    }
}

fn progress(completed: u64, generation: u64) -> CatalogueProgress {
    let id = |text: &str| Id::new(text).unwrap();
    CatalogueProgress {
        scope: Scope::new(
            id("receiver"),
            id("origin"),
            id("stream"),
            id("incarnation"),
            id("schema"),
            id("epoch"),
        ),
        completed,
        generation,
        active: None,
    }
}

/// A cache whose catalogue progress is at the head but which notes no
/// finished walk there is walked: an earlier walk was cut short after its
/// catalogue pass, and the conversations it did not reach may no longer be
/// granted. Only a walk noted at the same head and generation settles it.
#[test]
fn a_cache_without_the_walk_marker_is_walked_at_an_unchanged_head() {
    let saved = progress(7, 3);
    assert!(!settled(Some(&saved), None, 7), "no walk noted");
    assert!(
        !settled(Some(&saved), Some((6, 3)), 7),
        "walked at an older head"
    );
    assert!(
        !settled(Some(&saved), Some((7, 2)), 7),
        "walked before the catalogue was reset"
    );
    assert!(settled(Some(&saved), Some((7, 3)), 7));
    // The gateway's head moved on: walk, whatever was noted.
    assert!(!settled(Some(&saved), Some((7, 3)), 8));
    assert!(!settled(None, Some((7, 3)), 7), "no progress at all");
}

/// Only a walk that finished is noted, and one whose note cannot be written
/// fails as the cache's failure, so it is backed off, not walked again at
/// once: the note is what the next read settles on.
#[test]
fn a_marker_that_cannot_be_written_fails_the_read() {
    assert_eq!(walked(true, || Ok(())), Ok(true));
    assert_eq!(
        walked(true, || Err(CacheError::Unavailable)),
        Err(ReadFailure::Cache)
    );
    assert_eq!(
        walked(true, || Err(CacheError::Quota)),
        Err(ReadFailure::Quota)
    );
    // An incomplete walk notes nothing.
    assert_eq!(
        walked(false, || panic!("an incomplete walk is never noted")),
        Ok(false)
    );
}

/// A read withdraws at most as many conversations as its caller allows, so
/// the evidence one read leaves stays bounded; it then ends incomplete, and
/// the next read withdraws the rest.
#[test]
fn a_read_withdraws_at_most_its_cap_and_ends_incomplete() {
    let mut scripted = Scripted::new(vec![
        ("a", Ok(Conversation::Withdrawn)),
        ("b", Ok(Conversation::Read { complete: true })),
        ("c", Ok(Conversation::Withdrawn)),
        ("d", Ok(Conversation::Withdrawn)),
    ]);
    let mut withdrawn = vec!["earlier".to_owned()];
    let ids = scripted.ids();
    assert_eq!(
        read_each(ids, None, 2, &mut scripted, &mut withdrawn),
        Ok(false)
    );
    assert_eq!(withdrawn, ["earlier", "a", "c"]);
    assert_eq!(scripted.asked.len(), 3, "d is left for the next read");
}

fn report(complete: bool) -> ReadReport {
    ReadReport {
        moved: true,
        complete,
        conversations: 1,
    }
}

#[test]
fn a_read_its_budget_cut_short_counts_only_if_it_saved_something() {
    for cut in [ReadFailure::Stopped, ReadFailure::Unreachable] {
        // Saved something: incomplete, and the next read carries on.
        assert_eq!(read_end(Err(cut), false, true, true), ReadEnd::Incomplete);
        // Saved nothing: the peer did not answer in time.
        assert_eq!(
            read_end(Err(cut), false, true, false),
            ReadEnd::Failed(ReadFailure::Unreachable)
        );
    }
    // A walk the budget stopped before any conversation, having saved nothing.
    assert_eq!(
        read_end(Ok(report(false)), false, true, false),
        ReadEnd::Failed(ReadFailure::Unreachable)
    );
    assert_eq!(
        read_end(Ok(report(false)), false, true, true),
        ReadEnd::Read(report(false))
    );
    // Unreachable before the budget was spent is unreachable, not masked.
    assert_eq!(
        read_end(Err(ReadFailure::Unreachable), false, false, true),
        ReadEnd::Failed(ReadFailure::Unreachable)
    );
    // A stop is a stop, whatever the read saw first.
    assert_eq!(
        read_end(Err(ReadFailure::Unreachable), true, true, true),
        ReadEnd::Failed(ReadFailure::Stopped)
    );
    // Up to date is synced, saved or not.
    assert_eq!(
        read_end(Ok(report(true)), false, true, false),
        ReadEnd::Read(report(true))
    );
    // Other failures pass as they are.
    assert_eq!(
        read_end(Err(ReadFailure::Quota), false, true, true),
        ReadEnd::Failed(ReadFailure::Quota)
    );
}

/// A stop reaches a read blocked on a silent peer within one slice, by the
/// flag alone: as on Windows, where shutting the socket from another thread
/// does not wake a blocked receive. The wait the caller set is far longer.
#[test]
fn a_stop_ends_a_blocked_read_without_shutting_its_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    // Accepted and silent.
    let (_held, _) = listener.accept().unwrap();
    let stop = ReadStop::new();
    let reading = std::thread::spawn({
        let stop = stop.clone();
        move || {
            let mut socket = Socket::new(stream, stop);
            socket.read_timeout(Duration::from_secs(30)).unwrap();
            let started = std::time::Instant::now();
            let read = socket.read(&mut [0; 16]);
            (read.map_err(|error| error.kind()), started.elapsed())
        }
    });
    std::thread::sleep(STOP_SLICE * 4);
    // The flag only: no socket is shut.
    stop.stopped.store(true, Ordering::SeqCst);
    let (read, waited) = reading.join().unwrap();
    assert_eq!(read, Err(std::io::ErrorKind::ConnectionAborted));
    assert!(waited < Duration::from_secs(5), "{waited:?}");
}

/// A wait with no stop still lasts as long as the caller set, slice by
/// slice, and ends as the operating system's timeout would.
#[test]
fn a_sliced_wait_lasts_as_long_as_the_caller_set() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (_held, _) = listener.accept().unwrap();
    let mut socket = Socket::new(stream, ReadStop::new());
    socket.read_timeout(STOP_SLICE * 5).unwrap();
    let started = std::time::Instant::now();
    let kind = socket.read(&mut [0; 16]).unwrap_err().kind();
    assert!(matches!(
        kind,
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert!(
        started.elapsed() >= STOP_SLICE * 4,
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn uncertain_cache_commit_is_distinct_from_a_pre_effect_failure() {
    assert_eq!(
        cache_failure(&CacheError::Uncertain),
        ReadFailure::CacheUncertain
    );
    assert_eq!(cache_failure(&CacheError::Unavailable), ReadFailure::Cache);
    assert!(!holds_back(ReadFailure::CacheUncertain));
}

/// A monotonic clock the test moves by hand.
struct Moved(std::sync::atomic::AtomicU64);
impl MonotonicClock for Moved {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// A connect with a relay ends once the read's budget on the injected clock
/// is spent, though neither dial has answered and their own deadlines, on
/// the system clock, are far off.
#[test]
fn a_relayed_connect_ends_when_the_injected_budget_is_spent() {
    // A relay that takes the connection and never answers it.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let clock = Arc::new(Moved(std::sync::atomic::AtomicU64::new(0)));
    let ending = Arc::new(Stopping::new(ReadStop::new(), clock.clone(), 1_000));
    let connector = StoppableConnector {
        ending,
        relay: Some(RelayRoute {
            address: RelayAddress::try_from(silent.local_addr().unwrap().to_string()).unwrap(),
            key: [3; 32],
        }),
        offline: Arc::default(),
    };
    let started = std::time::Instant::now();
    let ended = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            clock.0.store(1_000, Ordering::SeqCst);
        });
        // Port 0 is never a listener's, so the direct dial is refused and
        // the relay leg starts at once.
        connector.connect(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            Duration::from_secs(30),
        )
    });
    let Err(error) = ended else {
        panic!("a silent relay joined nothing");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    assert!(started.elapsed() < Duration::from_secs(15));
}

/// Issue #266, R3: a read that never reached its gateway is `Away` only when
/// its relay said the gateway is not registered with it; every other end,
/// a busy gateway's `NoAnswer` included, is reported as before.
#[test]
fn a_read_is_away_only_when_its_relay_said_the_gateway_is_offline() {
    let unreachable = ReadEnd::Failed(ReadFailure::Unreachable);
    assert_eq!(
        away(unreachable.clone(), true),
        ReadEnd::Failed(ReadFailure::Away)
    );
    assert_eq!(away(unreachable.clone(), false), unreachable);
    for end in [
        ReadEnd::Read(report(true)),
        ReadEnd::Incomplete,
        ReadEnd::Failed(ReadFailure::Refused),
        ReadEnd::Failed(ReadFailure::Stopped),
    ] {
        assert_eq!(away(end.clone(), true), end);
    }
}

/// A monotonic clock that never moves, so no budget runs out.
struct Still;
impl MonotonicClock for Still {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}

/// A relay that takes one `Connect` and answers `answer`.
fn answering_relay(answer: RelayFrame) -> (RelayAddress, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let serving = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        nessa_protocol::relay::read_frame(&mut stream).unwrap();
        stream.write_all(&answer.encode()).unwrap();
    });
    (address, serving)
}

/// The connector notes the relay's `Offline` whichever way the race ends,
/// does not take a busy gateway's `NoAnswer` for it, and clears it once a
/// connect succeeds.
#[test]
fn the_connector_notes_a_relay_offline_and_a_connect_clears_it() {
    let refused = SocketAddr::from(([127, 0, 0, 1], 0));
    let connector = |address: RelayAddress, offline: &Arc<AtomicBool>| StoppableConnector {
        ending: Arc::new(Stopping::new(ReadStop::new(), Arc::new(Still), u64::MAX)),
        relay: Some(RelayRoute {
            address,
            key: [9; 32],
        }),
        offline: offline.clone(),
    };
    for (answer, noted) in [(RelayFrame::Offline, true), (RelayFrame::NoAnswer, false)] {
        let offline = Arc::new(AtomicBool::new(false));
        let (address, serving) = answering_relay(answer);
        assert!(connector(address, &offline)
            .connect(refused, Duration::from_secs(5))
            .is_err());
        serving.join().unwrap();
        assert_eq!(offline.load(Ordering::SeqCst), noted, "{answer:?}");
    }

    let offline = Arc::new(AtomicBool::new(true));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unused = RelayAddress::try_from("127.0.0.1:9".to_owned()).unwrap();
    connector(unused, &offline)
        .connect(listener.local_addr().unwrap(), Duration::from_secs(5))
        .unwrap();
    assert!(!offline.load(Ordering::SeqCst));
}

/// `dial_gateway`: an address that is not `host:port`, or a pin beside a
/// relay that is not a key, is said as such before any route is tried, rather than hidden behind the relay's answer;
/// a refused direct dial goes on to the relay, whose `Offline` it carries;
/// and without a relay it is the direct dial alone.
#[test]
fn dial_gateway_says_a_bad_address_and_carries_the_relays_answer() {
    let pin = NativeIdentity::generate(&mut nessa_auth::adapters::pairing::OsEntropy)
        .unwrap()
        .public_spki();
    let unused = RelayAddress::try_from("127.0.0.1:9".to_owned()).unwrap();
    for address in ["no-port", ":7443", "gw:port", "::1"] {
        let refused = dial_gateway(address, Some((&unused, &pin)), Duration::from_secs(5))
            .expect_err("not an address");
        assert_eq!(
            refused.kind(),
            std::io::ErrorKind::InvalidInput,
            "{address}"
        );
    }

    let not_a_key = dial_gateway(
        "127.0.0.1:0",
        Some((&unused, &[0; 44])),
        Duration::from_secs(5),
    )
    .expect_err("a pin that is not a key");
    assert_eq!(not_a_key.kind(), std::io::ErrorKind::InvalidInput);

    let (relay, serving) = answering_relay(RelayFrame::Offline);
    let offline = dial_gateway("127.0.0.1:0", Some((&relay, &pin)), Duration::from_secs(5))
        .expect_err("nothing listens at the gateway's address");
    serving.join().unwrap();
    assert_eq!(RelayRefusal::carried(&offline), Some(RelayRefusal::Offline));

    let direct = dial_gateway("127.0.0.1:0", None, Duration::from_secs(5))
        .expect_err("nothing listens at the gateway's address");
    // Refused on Unix, an unusable address on Windows: either way not this
    // side's own setup, so a caller dials again.
    assert_ne!(direct.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(RelayRefusal::carried(&direct), None);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connected = dial_gateway(&address.to_string(), None, Duration::from_secs(5)).unwrap();
    assert_eq!(connected.peer_addr().unwrap(), address);
}
