//! One enrolled binary Head behind the existing direct and joined relay.
use super::{completion, Device, Gateway, REFUSED};
use crate::composition::read_only_online_tests::fixtures::Setup;
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeTransport},
    domain::pairing::TerminalCause,
};
use nessa_client_core::retained::{dial_gateway, RetainedTranscript};
use nessa_device_pairing::infrastructure::RECORD_HEAD_PROBE_PREFIX;
use nessa_protocol::{
    conversation::domain::ConversationId,
    pairing::{
        wire::{
            decode_reply, encode_request, NativePairingReply, NativePairingRequest,
            NativePairingStatus,
        },
        EnrollmentChannel,
    },
    product::passive_read::wire_scope,
    relay::RelayAddress,
};
use nessa_sync::replication::{
    application::{RecordHeadSource, SourceError},
    domain::{CapabilitySupport, Scope, SourceHead},
    infrastructure::session::{
        CapabilityRecordSession, InterruptibleSessionIo, RecordSessionError, SessionInterrupt,
        SessionLimits, TcpSessionIo,
    },
};
use serde_json::json;
use std::{
    io::{self, ErrorKind, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(5);

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
) -> Result<EnrolledIo, ProbeRefusal> {
    let tcp = TcpSessionIo::new(dial(device, relay)?).map_err(|_| ProbeRefusal::Dial)?;
    let interrupt = tcp.interrupt_handle();
    let transport =
        NativeTransport::connect(tcp, &device.identity, GatewayTrust::Pinned(device.pin))
            .map_err(|_| ProbeRefusal::Tls)?;
    let mut channel = EnrollmentChannel::new(transport);
    let mut opening = RECORD_HEAD_PROBE_PREFIX.to_vec();
    opening.extend(
        serde_json::to_vec(&json!({
            "credential": device.credential,
            "receiver": device.receiver,
            "epoch": device.access_epoch,
            "scope": wire_scope(scope),
        }))
        .unwrap(),
    );
    channel
        .send_envelope(&opening)
        .map_err(|_| ProbeRefusal::Opening)?;
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
    let native = open_probe(device, relay, scope)?;
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
        Err(ProbeRefusal::Binding(_))
    ));
    assert!(matches!(
        probe(&device, Some(&relay), before.scope()),
        Err(ProbeRefusal::Binding(_))
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
        // Revocation closes the opted-in probe before NSR1. Await that close
        // without racing a binary write against it; no byte or timeout qualifies.
        let mut opened = open_probe(&device, route, before.scope()).unwrap();
        match opened.read(&mut [0_u8; 1]) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset
                ) => {}
            outcome => panic!("revoked probe did not close before its binary phase: {outcome:?}"),
        }
    }
    assert_eq!(saved(), before);
}
