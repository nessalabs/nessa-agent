//! One enrolled binary Head behind the existing direct and joined relay.
use super::{completion, Device, Gateway, Time, Wall, REFUSED};
use crate::composition::read_only_online_tests::fixtures::{Setup, WireClient};
use nessa_auth::{
    adapters::pairing::{FilePairingState, GatewayTrust, NativeTransport},
    domain::{pairing::TerminalCause, CredentialId},
};
use nessa_client_core::retained::{
    dial_gateway, JoinedRecordCommitFixture, ReadFailure, ReadStop, ReaderAccess, ReaderRole,
    RetainedEnrollmentClient, RetainedTranscript,
};
use nessa_local_database::rusqlite::{params, Connection, OpenFlags};
use nessa_protocol::{
    clock::Clock,
    conversation::domain::ConversationId,
    pairing::{
        wire::{
            decode_reply, encode_request, NativePairingReply, NativePairingRequest,
            NativePairingStatus, RecordCarrierMode, RecordCarrierOpen,
        },
        EnrollmentChannel,
    },
    product::generated::{
        product_method, ConversationRecordsHeadParams, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
    },
    relay::RelayAddress,
};
use nessa_sync::replication::{
    application::{RecordHeadSource, RecordSource, SourceError},
    domain::{CapabilitySupport, CommitPlan, Page, PageRequest, Scope, SourceHead},
    infrastructure::session::{
        CapabilityRecordSession, InterruptibleSessionIo, RecordSessionError, SessionInterrupt,
        SessionLimits, TcpSessionIo,
    },
};
use std::{
    io::{self, ErrorKind, Read, Write},
    net::TcpStream,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(5);

struct CommitClock {
    start: Instant,
    expired: AtomicBool,
}

impl Clock for CommitClock {
    fn elapsed_ms(&self) -> u64 {
        if self.expired.load(Ordering::SeqCst) {
            u64::MAX
        } else {
            u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum AtCommit {
    None,
    Stop,
    Expire,
}

// The same socket owner provides the physical interrupt, underneath the one
// verified Nessa TLS stream. No secondary reader or TLS engine is created.
struct EnrolledIo {
    tls: NativeTransport<TcpSessionIo>,
    interrupt: Arc<dyn SessionInterrupt>,
}

impl Read for EnrolledIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.tls.read(bytes)
    }
}
impl Write for EnrolledIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.tls.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
    }
}
impl InterruptibleSessionIo for EnrolledIo {
    fn interrupt_handle(&self) -> Arc<dyn SessionInterrupt> {
        self.interrupt.clone()
    }
}

#[derive(Debug)]
enum ProbeRefusal {
    Dial,
    Tls,
    Opening,
    Selection,
    Refused,
    Binding(RecordSessionError),
    Head(SourceError),
    Close(RecordSessionError),
}

impl std::fmt::Display for ProbeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Binding(error) => write!(f, "binding: {error:?}"),
            Self::Head(error) => write!(f, "head: {error:?}"),
            Self::Close(error) => write!(f, "close: {error:?}"),
            phase => write!(f, "{phase:?}"),
        }
    }
}

fn dial(device: &Device, relay: Option<&RelayAddress>) -> Result<TcpStream, ProbeRefusal> {
    let socket = match relay {
        None => TcpStream::connect_timeout(&device.address, WAIT),
        Some(relay) => dial_gateway(&REFUSED.to_string(), Some((relay, &device.pin)), None, WAIT),
    }
    .map_err(|_| ProbeRefusal::Dial)?;
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket.set_write_timeout(Some(WAIT)).unwrap();
    Ok(socket)
}

fn expect_revoked_status(device: &Device, relay: Option<&RelayAddress>) {
    // A separate pinned status exchange proves the published registry decision
    // without letting its local cleanup erase the fixture's saved evidence.
    let tls = NativeTransport::connect(
        dial(device, relay).unwrap(),
        &device.identity,
        GatewayTrust::Pinned(device.pin),
    )
    .unwrap();
    let mut channel = EnrollmentChannel::new(tls);
    let request = encode_request(&NativePairingRequest::Status(device.public)).unwrap();
    channel.send_envelope(&request).unwrap();
    let reply = decode_reply(&channel.receive_envelope().unwrap()).unwrap();
    let NativePairingReply::Status(status) = reply else {
        panic!("expected pinned status")
    };
    status.correlate(device.public, None).unwrap();
    assert!(matches!(
        status,
        NativePairingStatus::Terminal {
            cause: TerminalCause::CredentialRevoked,
            ..
        }
    ));
}

fn open_probe(
    device: &Device,
    relay: Option<&RelayAddress>,
    scope: &Scope,
    page: bool,
) -> Result<EnrolledIo, ProbeRefusal> {
    let tcp = TcpSessionIo::new(dial(device, relay)?).map_err(|_| ProbeRefusal::Dial)?;
    let interrupt = tcp.interrupt_handle();
    let transport =
        NativeTransport::connect(tcp, &device.identity, GatewayTrust::Pinned(device.pin))
            .map_err(|_| ProbeRefusal::Tls)?;
    let mut channel = EnrollmentChannel::new(transport);
    let request = NativePairingRequest::OpenRecordCarrierV1(RecordCarrierOpen {
        credential: CredentialId::new(device.credential.clone()).unwrap(),
        access_epoch: device.access_epoch,
        scope: scope.clone(),
        mode: if page {
            RecordCarrierMode::Page
        } else {
            RecordCarrierMode::Head
        },
    });
    channel
        .send_envelope(&encode_request(&request).map_err(|_| ProbeRefusal::Opening)?)
        .map_err(|_| ProbeRefusal::Opening)?;
    let reply = channel
        .receive_envelope()
        .map_err(|_| ProbeRefusal::Selection)?;
    match decode_reply(&reply).map_err(|_| ProbeRefusal::Selection)? {
        NativePairingReply::RecordCarrierV1Accepted => {}
        NativePairingReply::Refused => {
            let mut transport = channel.into_transport();
            return match transport.read(&mut [0_u8; 1]) {
                Ok(0) => Err(ProbeRefusal::Refused),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                    ) =>
                {
                    Err(ProbeRefusal::Refused)
                }
                _ => Err(ProbeRefusal::Selection),
            };
        }
        _ => return Err(ProbeRefusal::Selection),
    }
    Ok(EnrolledIo {
        tls: channel.into_transport(),
        interrupt,
    })
}

fn probe(
    device: &Device,
    relay: Option<&RelayAddress>,
    scope: &Scope,
) -> Result<SourceHead, ProbeRefusal> {
    let native = open_probe(device, relay, scope, false)?;
    let session = CapabilityRecordSession::connect(
        native,
        scope.clone(),
        SessionLimits {
            control_bytes: 2048,
            bindings: 1,
        },
        CapabilitySupport::current(),
    )
    .map_err(ProbeRefusal::Binding)?;
    let (_, mut source) = session.parts();
    let head = source.source_head(scope).map_err(ProbeRefusal::Head)?;
    session.close().map_err(ProbeRefusal::Close)?;
    Ok(head)
}

#[test]
fn enrolled_head_matches_the_existing_product_read_on_direct_and_relay() {
    let directory = tempfile::tempdir().unwrap();
    let (gateway, device) = Device::start(directory.path(), "record-head-probe");
    let relay = device.relay();
    device.wait_registered();
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(device.root.join("setup.json")).unwrap()).unwrap();
    let conversation = ConversationId::new(&setup.conversation).unwrap();
    let mut cache = device.cache("record-head-prior.sqlite3");
    assert!(
        completion::finish(|budget| device.read_for(&mut cache, device.address, None, budget, 8))
            .unwrap()
            .complete
    );
    let catalogue = cache
        .saved_catalogue_scope(&device.receiver)
        .unwrap()
        .unwrap();
    let mut saved = || {
        let RetainedTranscript::Retained(snapshot) = cache
            .transcript_view(
                &device.receiver,
                catalogue.origin().as_str(),
                &conversation,
                Uuid::nil(),
            )
            .unwrap()
        else {
            panic!("baseline must retain the seeded transcript")
        };
        snapshot.progress().clone()
    };
    let before = saved();
    assert!(before.downloaded() > 0);
    for route in [None, Some(&relay)] {
        let head = probe(&device, route, before.scope()).unwrap();
        assert_eq!(head.scope(), before.scope());
        assert_eq!(head.position(), before.downloaded());
    }
    assert_eq!(saved(), before);
    drop(gateway);

    let default = Gateway::start_relayed(&device.root, "");
    let relay = device.relay();
    device.wait_registered();
    assert!(matches!(
        probe(&device, None, before.scope()),
        Err(ProbeRefusal::Refused)
    ));
    assert!(matches!(
        probe(&device, Some(&relay), before.scope()),
        Err(ProbeRefusal::Refused)
    ));
    let mut ordinary = device.cache("record-head-ordinary-default.sqlite3");
    assert!(
        completion::finish(|budget| device.read_for(
            &mut ordinary,
            device.address,
            None,
            budget,
            8
        ))
        .unwrap()
        .complete
    );
    drop(default);

    let _revoked = Gateway::start_relayed(&device.root, "record-head-probe-revoked");
    let relay = device.relay();
    device.wait_registered();
    for route in [None, Some(&relay)] {
        expect_revoked_status(&device, route);
        // The current registry refusal is terminal for the selected carrier.
        assert!(matches!(
            open_probe(&device, route, before.scope(), false),
            Err(ProbeRefusal::Refused)
        ));
    }
    assert_eq!(saved(), before);
}

struct RecordRequests<S> {
    source: S,
    after: Vec<u64>,
}

impl<S: RecordSource> RecordSource for RecordRequests<S> {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        self.source.head(scope)
    }

    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.after.push(request.after);
        self.source.page(request)
    }
}

fn scoped_raw_rows(device: &Device, name: &str, scope: &Scope) -> Vec<(u64, String, Vec<u8>)> {
    let db =
        Connection::open_with_flags(device.caches.join(name), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut query = db
        .prepare(
            "SELECT position, record_id, payload FROM transcript_records
         WHERE receiver = ?1 AND origin = ?2 AND stream = ?3 ORDER BY position LIMIT 3",
        )
        .unwrap();
    query
        .query_map(
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str()
            ],
            |row| {
                Ok((
                    u64::from_be_bytes(row.get::<_, [u8; 8]>(0)?),
                    row.get(1)?,
                    row.get(2)?,
                ))
            },
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn enrolled_pair_commits_two_pages_on_one_connection_and_refuses_a_third() {
    for use_relay in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (_gateway, device) = Device::start(directory.path(), "record-page-pair");
        device.wait_registered();
        let setup: Setup =
            serde_json::from_slice(&std::fs::read(device.root.join("setup.json")).unwrap())
                .unwrap();
        let conversation = ConversationId::new(&setup.conversation).unwrap();
        let mut ordinary = device.cache("pair-baseline.sqlite3");
        assert!(
            completion::finish(|budget| device.read_for(
                &mut ordinary,
                device.address,
                None,
                budget,
                8,
            ))
            .unwrap()
            .complete
        );
        let catalogue = ordinary
            .saved_catalogue_scope(&device.receiver)
            .unwrap()
            .unwrap();
        let RetainedTranscript::Retained(baseline) = ordinary
            .transcript_view(
                &device.receiver,
                catalogue.origin().as_str(),
                &conversation,
                Uuid::nil(),
            )
            .unwrap()
        else {
            panic!("ordinary baseline must discover and retain the authorized scope");
        };
        let scope = baseline.progress().scope();
        drop(ordinary);
        let expected = scoped_raw_rows(&device, "pair-baseline.sqlite3", scope);
        assert_eq!(expected.len(), 3);
        let relay = device.relay();
        let io = open_probe(&device, use_relay.then_some(&relay), scope, true).unwrap();
        let session = CapabilityRecordSession::connect(
            io,
            scope.clone(),
            SessionLimits {
                control_bytes: 2048,
                bindings: 1,
            },
            CapabilitySupport::current(),
        )
        .unwrap();
        let (mut auth, source) = session.parts();
        let mut source = RecordRequests {
            source,
            after: Vec::new(),
        };
        let name = "native-record-page-pair.sqlite3";
        let mut retained = device.cache(name);
        for durable in [1, 2] {
            assert_eq!(
                retained
                    .one_record_page_for_test(scope, &mut auth, &mut source)
                    .unwrap(),
                (durable, 1)
            );
            assert_eq!(
                scoped_raw_rows(&device, name, scope),
                expected[..durable as usize]
            );
        }
        assert_eq!(source.after, [0, 1]);
        let source_entries = || {
            std::fs::read_dir(&device.root)
                .unwrap()
                .filter(|entry| {
                    entry
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("pair-source-entries.")
                })
                .count()
        };
        let entries = source_entries();
        assert!(entries > 0, "fixture must see physical source calls");
        assert!(matches!(
            source.page(&PageRequest {
                scope: scope.clone(),
                after: 2,
                target: baseline.progress().downloaded(),
                max_records: 1,
                max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
            }),
            Err(SourceError::Denied)
        ));
        assert_eq!(source.after, [0, 1, 2]);
        assert_eq!(
            source_entries(),
            entries,
            "third Page must not enter physical Head or Page"
        );
        session.close().unwrap();
        drop(retained);
        let mut reopened = device.cache(name);
        let RetainedTranscript::Retained(saved) = reopened
            .transcript_view(
                &device.receiver,
                scope.origin().as_str(),
                &conversation,
                Uuid::nil(),
            )
            .unwrap()
        else {
            panic!("the reopened owner must retain the two committed Pages");
        };
        assert_eq!(saved.progress().scope(), scope);
        assert_eq!(saved.progress().downloaded(), 2);
        drop(reopened);
        assert_eq!(scoped_raw_rows(&device, name, scope), expected[..2]);
    }
}

#[test]
fn enrolled_pages_resume_from_the_existing_retained_database_on_both_routes() {
    for use_relay in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (mut gateway, device) = Device::start(directory.path(), "record-head-probe");
        device.wait_registered();
        let setup: Setup =
            serde_json::from_slice(&std::fs::read(device.root.join("setup.json")).unwrap())
                .unwrap();
        let conversation = ConversationId::new(&setup.conversation).unwrap();
        let mut ordinary = device.cache("page-baseline.sqlite3");
        assert!(
            completion::finish(|budget| device.read_for(
                &mut ordinary,
                device.address,
                None,
                budget,
                8
            ))
            .unwrap()
            .complete
        );
        let catalogue = ordinary
            .saved_catalogue_scope(&device.receiver)
            .unwrap()
            .unwrap();
        let RetainedTranscript::Retained(baseline) = ordinary
            .transcript_view(
                &device.receiver,
                catalogue.origin().as_str(),
                &conversation,
                Uuid::nil(),
            )
            .unwrap()
        else {
            panic!("ordinary baseline did not save the authorized target");
        };
        let scope = baseline.progress().scope();
        drop(ordinary);
        let expected = scoped_raw_rows(&device, "page-baseline.sqlite3", scope);
        assert!(expected.len() >= 2);
        assert_eq!((expected[0].0, expected[1].0), (1, 2));

        if use_relay {
            // Actual enrolled state and the exact scope just observed through
            // Product. Prove Bind/Page on that joined transport without a cache.
            let profile: serde_json::Value =
                serde_json::from_slice(&std::fs::read(device.root.join("profile.json")).unwrap())
                    .unwrap();
            let state = FilePairingState::open(
                Path::new(profile["stateRoot"].as_str().unwrap()),
                Path::new(profile["stateDirectory"].as_str().unwrap()),
            )
            .unwrap();
            let unused_cache = device.caches.join("joined-selector-unused.sqlite3");
            let client = RetainedEnrollmentClient::new(
                Arc::new(state),
                unused_cache.clone(),
                Arc::new(Wall),
                Arc::new(Time(Instant::now())),
            )
            .unwrap();
            let relay = device.relay().as_str().parse().unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let page = runtime
                .block_on(client.joined_record_page_for_test(
                    relay,
                    device.pin,
                    scope.clone(),
                    ReadStop::new(),
                    WAIT.as_millis().try_into().unwrap(),
                ))
                .unwrap()
                .unwrap();
            runtime.block_on(client.shutdown());
            assert_eq!(&page.request.scope, scope);
            assert_eq!(page.request.after, 0);
            assert_eq!(page.records.len(), 1);
            let record = &page.records[0];
            assert_eq!(record.position, expected[0].0);
            assert_eq!(record.id.as_str(), expected[0].1);
            assert_eq!(record.payload.as_slice(), expected[0].2.as_slice());
            assert!(!unused_cache.exists());

            for (name, refusal) in [
                ("joined-stopped.sqlite3", AtCommit::Stop),
                ("joined-expired.sqlite3", AtCommit::Expire),
            ] {
                let run = |expected_after, at_commit, payload: (u64, String, Vec<u8>)| {
                    let state = FilePairingState::open(
                        Path::new(profile["stateRoot"].as_str().unwrap()),
                        Path::new(profile["stateDirectory"].as_str().unwrap()),
                    ).unwrap();
                    let clock = Arc::new(CommitClock {
                        start: Instant::now(),
                        expired: AtomicBool::new(false),
                    });
                    let client = RetainedEnrollmentClient::new(
                        Arc::new(state), device.caches.join(name), Arc::new(Wall), clock.clone(),
                    ).unwrap();
                    let stop = ReadStop::new();
                    let hit = Arc::new(AtomicBool::new(false));
                    let hit_before = hit.clone();
                    let gate_stop = stop.clone();
                    let result = runtime.block_on(client.retain_joined_record_page_for_test(
                        JoinedRecordCommitFixture {
                            relay, pin: device.pin, scope: scope.clone(), stop,
                            expires_ms: WAIT.as_millis().try_into().unwrap(),
                        },
                        device.cache(name),
                        move |plan: &CommitPlan| {
                            assert_eq!(plan.expected().position(), expected_after);
                            assert_eq!(plan.next().position(), payload.0);
                            assert_eq!(plan.records().len(), 1);
                            let record = &plan.records()[0];
                            assert_eq!(record.id.as_str(), payload.1);
                            assert_eq!(record.payload.as_slice(), payload.2.as_slice());
                            assert!(!hit_before.swap(true, Ordering::SeqCst));
                            match at_commit {
                                AtCommit::None => {}
                                AtCommit::Stop => gate_stop.stop(),
                                AtCommit::Expire => clock.expired.store(true, Ordering::SeqCst),
                            }
                        },
                    )).unwrap();
                    runtime.block_on(client.shutdown());
                    assert!(hit.load(Ordering::SeqCst), "after stage, before COMMIT");
                    result
                };

                assert_eq!(run(0, AtCommit::None, expected[0].clone()), Ok((1, 1)));
                assert_eq!(scoped_raw_rows(&device, name, scope), expected[..1]);
                let expected_refusal = match refusal {
                    AtCommit::Stop => ReadFailure::Stopped,
                    AtCommit::Expire => ReadFailure::Unreachable,
                    AtCommit::None => unreachable!(),
                };
                assert_eq!(run(1, refusal, expected[1].clone()), Err(expected_refusal));
                let mut reopened = device.cache(name);
                let RetainedTranscript::Retained(saved) = reopened.transcript_view(
                    &device.receiver, scope.origin().as_str(), &conversation, Uuid::nil(),
                ).unwrap() else {
                    panic!("the refused COMMIT must leave the original checkpoint");
                };
                assert_eq!(saved.progress().scope(), scope);
                assert_eq!(saved.progress().downloaded(), 1);
                drop(reopened);
                assert_eq!(scoped_raw_rows(&device, name, scope), expected[..1]);

                assert_eq!(run(1, AtCommit::None, expected[1].clone()), Ok((2, 1)));
                assert_eq!(scoped_raw_rows(&device, name, scope), expected[..2]);
            }
        }

        let name = "binary-staged.sqlite3";
        for after in [0_u64, 1] {
            if after > 0 {
                drop(gateway);
                gateway = Gateway::start_relayed(&device.root, "record-head-probe");
                device.wait_registered();
            }
            let relay = device.relay();
            let io = open_probe(&device, use_relay.then_some(&relay), scope, true).unwrap();
            let session = CapabilityRecordSession::connect(
                io,
                scope.clone(),
                SessionLimits {
                    control_bytes: 2048,
                    bindings: 1,
                },
                CapabilitySupport::current(),
            )
            .unwrap();
            let (mut auth, source) = session.parts();
            let mut source = RecordRequests {
                source,
                after: Vec::new(),
            };
            let mut retained = device.cache(name);
            if after > 0 {
                let RetainedTranscript::Retained(saved) = retained
                    .transcript_view(
                        &device.receiver,
                        scope.origin().as_str(),
                        &conversation,
                        Uuid::nil(),
                    )
                    .unwrap()
                else {
                    panic!("the reopened cache must own the first committed cursor");
                };
                assert_eq!(saved.progress().scope(), scope);
                assert_eq!(saved.progress().downloaded(), after);
            }
            let (position, pages) = retained
                .one_record_page_for_test(scope, &mut auth, &mut source)
                .unwrap();
            assert_eq!((position, pages), (after + 1, 1));
            assert_eq!(source.after, [after]);
            let counters = session.counters().unwrap();
            assert_eq!(
                counters.payload_bytes as usize,
                expected[after as usize].2.len()
            );
            assert_eq!(counters.duplicate_bytes, 0);
            eprintln!(
                "enrolled_page route={} after={after} durable={position} records=1 payload_bytes={} protocol_bytes={} exchanges={}",
                if use_relay { "opaque-local-relay" } else { "direct" },
                counters.payload_bytes, counters.protocol_bytes, counters.exchanges,
            );
            session.close().unwrap();
            drop(retained);
            assert_eq!(
                scoped_raw_rows(&device, name, scope),
                expected[..position as usize]
            );
        }
        // The original mount spends its one allowance even if the gateway's
        // host limit rejects an upstream-valid request.
        let relay = device.relay();
        let io = open_probe(&device, use_relay.then_some(&relay), scope, true).unwrap();
        let session = CapabilityRecordSession::connect(
            io,
            scope.clone(),
            SessionLimits {
                control_bytes: 2048,
                bindings: 1,
            },
            CapabilitySupport::current(),
        )
        .unwrap();
        let (_, mut source) = session.parts();
        let mut request = PageRequest {
            scope: scope.clone(),
            after: 0,
            target: baseline.progress().downloaded(),
            max_records: MAX_RECORD_PAGE_RECORDS + 1,
            max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        };
        assert!(matches!(
            source.page(&request),
            Err(SourceError::InvalidRequest)
        ));
        request.max_records = 1;
        assert!(matches!(source.page(&request), Err(SourceError::Denied)));
        session.close().unwrap();

        // Repeat the prefix through the real retained client opener, not
        // the fixture's binary session. The same Product-discovered scope
        // and existing SQLite driver remain the authorities on both routes.
        let name = "native-adapter-staged.sqlite3";
        let relay = device.relay();
        let access = || ReaderAccess {
            role: ReaderRole::DeviceOwner,
            address: if use_relay { REFUSED } else { device.address },
            relay: use_relay.then_some(&relay),
            identity: &device.identity,
            pin: device.pin,
            credential: &device.credential,
            receiver: &device.receiver,
            access_epoch: device.access_epoch,
            client_id: "retained-binary-process-fixture",
        };
        let stopped = ReadStop::new();
        stopped.stop();
        let mut retained = device.cache(name);
        assert_eq!(
            retained.enrolled_record_page_for_test(access(), scope, stopped, WAIT),
            Err(ReadFailure::Stopped)
        );
        drop(retained);
        assert!(scoped_raw_rows(&device, name, scope).is_empty());
        let mut retained = device.cache(name);
        assert_eq!(
            retained.enrolled_record_page_for_test(access(), scope, ReadStop::new(), WAIT),
            Ok((1, 1))
        );
        drop(retained);
        assert_eq!(scoped_raw_rows(&device, name, scope), expected[..1]);

        // Keep one separately authenticated Product connection open across
        // this attempt, rather than dialing again after interruption.
        let mut product = WireClient::connect(&device.root);
        let params = ConversationRecordsHeadParams {
            conversation_id: setup.conversation.clone(),
            receiver_id: device.receiver.clone(),
            access_epoch: device.access_epoch.to_string(),
        };
        let product_head = product.call(product_method::CONVERSATION_RECORDS_HEAD, &params);
        assert_eq!(product_head["ok"], true, "{product_head}");

        let stop = ReadStop::new();
        let mut retained = device.cache(name);
        let (interrupted_after, read) =
            retained.interrupt_enrolled_record_page_for_test(access(), scope, stop.clone(), WAIT);
        assert_eq!(interrupted_after, Some(1));
        assert!(read.is_err(), "{read:?}");
        assert!(
            !stop.is_stopped(),
            "only the selected record socket stopped"
        );
        drop(retained);

        let surviving = product.call(product_method::CONVERSATION_RECORDS_HEAD, &params);
        assert_eq!(surviving["ok"], true, "{surviving}");
        assert_eq!(surviving["payload"], product_head["payload"]);

        assert_eq!(scoped_raw_rows(&device, name, scope), expected[..1]);
        let mut reopened = device.cache(name);
        let RetainedTranscript::Retained(saved) = reopened
            .transcript_view(
                &device.receiver,
                scope.origin().as_str(),
                &conversation,
                Uuid::nil(),
            )
            .unwrap()
        else {
            panic!("interruption must preserve the exact first durable checkpoint");
        };
        assert_eq!(saved.progress().scope(), scope);
        assert_eq!(saved.progress().downloaded(), 1);
        assert_eq!(
            reopened.enrolled_record_page_for_test(access(), scope, ReadStop::new(), WAIT),
            Ok((2, 1))
        );
        drop(reopened);
        assert_eq!(scoped_raw_rows(&device, name, scope), expected[..2]);
        drop(gateway);
    }
}
