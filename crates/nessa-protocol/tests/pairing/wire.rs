//! Public codec tests. Domain transitions prove representation, not enrollment effects.
use crate::pairing::{
    wire::{
        decode_reply, decode_request, encode_challenge, encode_hello, encode_refused,
        encode_request, encode_status, NativePairingReply, NativePairingRequest,
        NativePairingStatus, NativeWireError, MAX_ENROLLMENT_ENVELOPE_BYTES,
    },
    DevicePairingStatus,
};
use nessa_auth::{
    adapters::pairing::{
        ClientAttempt, FilePairingState, GatewayTrust, ManualCode, NativeIdentity, NativeTransport,
        OsEntropy, PairingCryptoError, ServerInvitation, MAX_ENROLLMENT_MESSAGE_BYTES,
    },
    application::pairing::ClientPendingStore,
    domain::{
        pairing::{
            AttemptFailure, AttemptId, AttemptOutcome, ConsentClass, ConsentIntent,
            ConsentIntentId, DeviceKey, DisclosedConsent, InvitationId, PairingError, PairingEvent,
            PairingInitiator, PairingPhase, PairingPolicy, PairingRecord, PublicIntent,
            TerminalCause,
        },
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
        MAX_IDENTIFIER_BYTES,
    },
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    net::{TcpListener, TcpStream},
    path::Path,
    thread,
    time::Duration,
};

fn public() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([2; AttemptId::LENGTH]),
        ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
        1,
        600_000,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}

fn initial_record(audience: &str, organization: &str, resource: &str) -> PairingRecord {
    let intent = ConsentIntent::new(
        public().consent(),
        public().generation(),
        AudienceId::new(audience).unwrap(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("membership").unwrap(),
        Resource::new(
            OrganizationId::new(organization).unwrap(),
            ResourceId::new(resource).unwrap(),
        ),
        ConsentClass::DeviceRead,
    )
    .unwrap();
    PairingRecord::new(public().invitation(), intent, 0, PairingPolicy::initial()).unwrap()
}

fn transition(
    record: &PairingRecord,
    event: PairingEvent,
    actor: PairingInitiator,
    at: u64,
) -> PairingRecord {
    // These public pure events are not a substitute for application proof/admission.
    record.transition(event, actor, at).unwrap().after().clone()
}

/// The receiver epoch a test's Active status reports as current; deliberately
/// not the epoch the record's pair receipt holds.
const CURRENT_EPOCH: u64 = 3;
/// The projection the status query returns for `record`.
fn projected(record: PairingRecord) -> DevicePairingStatus {
    if record.phase() == PairingPhase::Active {
        DevicePairingStatus::Active {
            record: Box::new(record),
            access_epoch: CURRENT_EPOCH,
        }
    } else {
        DevicePairingStatus::Claimed(Box::new(record))
    }
}
fn active_record() -> PairingRecord {
    claimed_records("gateway", "org", "gateway").remove(3)
}
fn claimed_records(audience: &str, organization: &str, resource: &str) -> Vec<PairingRecord> {
    let device = DeviceKey::new([4; DeviceKey::LENGTH]);
    let owner = PairingInitiator::Principal(PrincipalId::new("owner").unwrap());
    let reserved = transition(
        &initial_record(audience, organization, resource),
        PairingEvent::reserve(public().attempt(), device, [5; 32]),
        PairingInitiator::Device(device),
        1,
    );
    let claimed = transition(
        &reserved,
        PairingEvent::claim(public().attempt(), device),
        PairingInitiator::Device(device),
        2,
    );
    let approved = transition(
        &claimed,
        PairingEvent::approve(device, public().generation()),
        owner.clone(),
        3,
    );
    let credential = CredentialId::new("device-credential").unwrap();
    let request = AttemptId::new([6; AttemptId::LENGTH]);
    let staging = transition(
        &approved,
        PairingEvent::stage(credential.clone(), request, public().generation()),
        owner.clone(),
        4,
    );
    let received = transition(
        &staging,
        PairingEvent::receiver(
            credential,
            request,
            ResourceId::new("receiver").unwrap(),
            1,
            public().generation(),
        ),
        PairingInitiator::System,
        5,
    );
    let active = transition(
        &received,
        PairingEvent::activate(public().generation()),
        owner.clone(),
        6,
    );
    let terminal = transition(
        &active,
        PairingEvent::end(TerminalCause::Cancelled),
        owner,
        7,
    );
    vec![claimed, approved, staging, active, terminal]
}

fn status(bytes: &[u8]) -> NativePairingStatus {
    match decode_reply(bytes).unwrap() {
        NativePairingReply::Status(status) => status,
        _ => panic!("expected enrollment status"),
    }
}

fn object_keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn current_enrollment_messages_preserve_their_representation() {
    for request in [
        NativePairingRequest::Hello(public().attempt()),
        NativePairingRequest::Begin {
            public: public(),
            request: vec![0, 1, 255],
        },
        NativePairingRequest::Confirm {
            public: public(),
            message: vec![255, 0],
        },
        NativePairingRequest::Status(public()),
        NativePairingRequest::OpenProduct,
    ] {
        let bytes = encode_request(&request).unwrap();
        let decoded = decode_request(&bytes).unwrap();
        assert_eq!(encode_request(&decoded).unwrap(), bytes);
    }
    // Row PR1: the selector's spelling is the literal tag, with no fields.
    assert_eq!(
        encode_request(&NativePairingRequest::OpenProduct).unwrap(),
        br#"{"kind":"openProduct"}"#
    );
    assert_eq!(
        decode_request(br#"{"kind":"openProduct","credential":"x"}"#).unwrap_err(),
        NativeWireError::Invalid
    );
    assert!(
        matches!(decode_reply(&encode_hello(public()).unwrap()).unwrap(), NativePairingReply::Hello(value) if value == public())
    );
    assert!(
        matches!(decode_reply(&encode_challenge(public(), &[0, 255]).unwrap()).unwrap(), NativePairingReply::Challenge { public: value, response } if value == public() && response == [0, 255])
    );
    assert!(matches!(
        decode_reply(&encode_refused().unwrap()).unwrap(),
        NativePairingReply::Refused
    ));
    for record in claimed_records("gateway", "org", "gateway") {
        let consent = DisclosedConsent::from_intent(public(), record.intent()).unwrap();
        let bytes = encode_status(public(), &projected(record.clone())).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let phase = match record.phase() {
            PairingPhase::Claimed => "claimed",
            PairingPhase::Approved => "approved",
            PairingPhase::Staging => "staging",
            PairingPhase::Active => "active",
            PairingPhase::Terminal => "terminal",
            PairingPhase::Available => panic!("fixture has not claimed"),
        };
        assert_eq!(value["status"]["stage"], json!(phase));
        let projected = status(&bytes);
        assert_eq!(projected.public(), public());
        assert_eq!(projected.consent(), Some(&consent));
        let expected = match record.phase() {
            PairingPhase::Claimed => NativePairingStatus::Claimed(Box::new(consent)),
            PairingPhase::Approved => NativePairingStatus::Approved(Box::new(consent)),
            PairingPhase::Staging => NativePairingStatus::Staging(Box::new(consent)),
            PairingPhase::Active => NativePairingStatus::Active {
                consent: Box::new(consent),
                credential: record.credential().unwrap().clone(),
                receiver: record.receiver_binding().unwrap().0.clone(),
                access_epoch: CURRENT_EPOCH,
            },
            PairingPhase::Terminal => NativePairingStatus::Terminal {
                consent: Box::new(consent),
                cause: record.terminal().unwrap().0,
            },
            PairingPhase::Available => panic!("fixture has not claimed"),
        };
        assert_eq!(projected, expected);
    }
    // Escaping and maximum valid domain metadata remain representable.
    let escaped = "\\\"".repeat(MAX_IDENTIFIER_BYTES / 2);
    for record in claimed_records(&escaped, &escaped, &escaped) {
        let bytes = encode_status(public(), &projected(record.clone())).unwrap();
        assert!(bytes.len() <= MAX_ENROLLMENT_ENVELOPE_BYTES);
        assert_eq!(
            status(&bytes).consent(),
            Some(&DisclosedConsent::from_intent(public(), record.intent()).unwrap())
        );
    }
}

#[test]
fn envelope_limits_apply_before_decode_and_during_encode() {
    // The current external enrollment contract accepts 4096 encoded bytes.
    // Keep this fixture independent of the implementation's bound publication.
    let encoded_contract_bytes = 4096;
    let mut exact = encode_refused().unwrap();
    exact.resize(encoded_contract_bytes, b' ');
    assert!(matches!(
        decode_reply(&exact).unwrap(),
        NativePairingReply::Refused
    ));
    exact.push(b' ');
    assert_eq!(decode_reply(&exact).unwrap_err(), NativeWireError::TooLarge);
    let mut exact_request = encode_request(&NativePairingRequest::Status(public())).unwrap();
    exact_request.resize(encoded_contract_bytes, b' ');
    assert!(
        matches!(decode_request(&exact_request).unwrap(), NativePairingRequest::Status(value) if value == public())
    );
    exact_request.push(b' ');
    assert_eq!(
        decode_request(&exact_request).unwrap_err(),
        NativeWireError::TooLarge
    );
    // This malformed document must fail the size boundary before syntax decoding.
    assert_eq!(
        decode_request(&vec![b'!'; MAX_ENROLLMENT_ENVELOPE_BYTES + 1]).unwrap_err(),
        NativeWireError::TooLarge
    );
    // Numeric-array stress input exercises the public encoder, not KE2 validity.
    let metadata_bytes = encode_challenge(public(), &[]).unwrap().len();
    let payload_bytes = encoded_contract_bytes - metadata_bytes;
    let mut exact_payload = vec![0; payload_bytes.div_ceil(2)];
    if payload_bytes.is_multiple_of(2) {
        exact_payload[0] = 10;
    }
    let encoded = encode_challenge(public(), &exact_payload).unwrap();
    assert_eq!(encoded.len(), encoded_contract_bytes);
    assert!(
        matches!(decode_reply(&encoded).unwrap(), NativePairingReply::Challenge { public: value, response } if value == public() && response == exact_payload)
    );
    // Replacing a second zero with ten adds exactly one encoded decimal byte.
    exact_payload[1] = 10;
    assert_eq!(
        encode_challenge(public(), &exact_payload),
        Err(NativeWireError::TooLarge)
    );
    // The payload is arbitrary representation input, not a method-valid KE2.
    let oversized = vec![255; MAX_ENROLLMENT_MESSAGE_BYTES];
    assert_eq!(
        encode_challenge(public(), &oversized),
        Err(NativeWireError::TooLarge)
    );
    for request in [
        NativePairingRequest::Begin {
            public: public(),
            request: oversized.clone(),
        },
        NativePairingRequest::Confirm {
            public: public(),
            message: oversized,
        },
    ] {
        assert_eq!(encode_request(&request), Err(NativeWireError::TooLarge));
    }
}

#[test]
fn enrollment_syntax_is_strict_and_domain_values_are_validated() {
    for malformed in [
        br#"{"kind":"refused","extra":true}"#.as_slice(),
        br#"{"kind":"refused","kind":"refused"}"#.as_slice(),
        br#"{"kind":"refused"}{}"#.as_slice(),
        br#"{"kind":"future"}"#.as_slice(),
    ] {
        assert_eq!(
            decode_reply(malformed).unwrap_err(),
            NativeWireError::Invalid
        );
    }
    for malformed in [
        br#"{"kind":"hello","attempt":[],"extra":true}"#.as_slice(),
        br#"{"kind":"hello","kind":"hello","attempt":[]}"#.as_slice(),
        br#"{"kind":"openProduct","kind":"openProduct"}"#.as_slice(),
        br#"{"kind":"openProduct"}{}"#.as_slice(),
        br#"{"kind":"future"}"#.as_slice(),
    ] {
        assert_eq!(
            decode_request(malformed).unwrap_err(),
            NativeWireError::Invalid
        );
    }
    let mut valid_request: Value = serde_json::from_slice(
        &encode_request(&NativePairingRequest::Hello(public().attempt())).unwrap(),
    )
    .unwrap();
    valid_request["authority"] = json!(true);
    assert_eq!(
        decode_request(&serde_json::to_vec(&valid_request).unwrap()).unwrap_err(),
        NativeWireError::Invalid
    );
    let hello: Value = serde_json::from_slice(&encode_hello(public()).unwrap()).unwrap();
    for field in ["invitation", "attempt", "consent"] {
        for length in [InvitationId::LENGTH - 1, InvitationId::LENGTH + 1] {
            let mut invalid = hello.clone();
            invalid["public"][field] = json!(vec![0; length]);
            assert_eq!(
                decode_reply(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
                NativeWireError::Invalid
            );
        }
        for byte in [json!(-1), json!(256), json!("0")] {
            let mut invalid = hello.clone();
            invalid["public"][field][0] = byte;
            assert_eq!(
                decode_reply(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
                NativeWireError::Invalid
            );
        }
    }
    for field in ["generation", "expiryMs"] {
        let mut invalid = hello.clone();
        invalid["public"][field] = json!(0);
        assert_eq!(
            decode_reply(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
            NativeWireError::Correlation(PairingError::Invalid)
        );
    }
    let mut invalid = hello.clone();
    invalid["public"]["class"] = json!("conversation.write");
    assert_eq!(
        decode_reply(&serde_json::to_vec(&invalid).unwrap()).unwrap_err(),
        NativeWireError::Invalid
    );
    let mut extra = hello.clone();
    extra["public"]["proof"] = json!(true);
    assert_eq!(
        decode_reply(&serde_json::to_vec(&extra).unwrap()).unwrap_err(),
        NativeWireError::Invalid
    );
    let public_json = serde_json::to_string(&hello["public"]).unwrap();
    let duplicate = format!(r#"{{"kind":"hello","public":{public_json},"public":{public_json}}}"#);
    assert_eq!(
        decode_reply(duplicate.as_bytes()).unwrap_err(),
        NativeWireError::Invalid
    );

    let approved = claimed_records("gateway", "org", "gateway").remove(1);
    let good: Value =
        serde_json::from_slice(&encode_status(public(), &projected(approved)).unwrap()).unwrap();
    for field in ["status", "consent"] {
        let mut extra = good.clone();
        if field == "status" {
            extra["status"]["extra"] = json!(true);
        } else {
            extra["status"]["consent"]["extra"] = json!(true);
        }
        assert_eq!(
            decode_reply(&serde_json::to_vec(&extra).unwrap()).unwrap_err(),
            NativeWireError::Invalid
        );
    }
    let unclaimed = encode_status(
        public(),
        &DevicePairingStatus::Unclaimed {
            outcome: AttemptOutcome::Failed(AttemptFailure::InvalidProof),
            terminal: None,
        },
    )
    .unwrap();
    let mut extra: Value = serde_json::from_slice(&unclaimed).unwrap();
    extra["status"]["outcome"]["extra"] = json!(true);
    assert_eq!(
        decode_reply(&serde_json::to_vec(&extra).unwrap()).unwrap_err(),
        NativeWireError::Invalid
    );
    let mut bad_grant = good.clone();
    bad_grant["status"]["consent"]["action"] = json!("conversation.write");
    assert_eq!(
        decode_reply(&serde_json::to_vec(&bad_grant).unwrap()).unwrap_err(),
        NativeWireError::Correlation(PairingError::Invalid)
    );
    for field in ["audience", "organization", "resource"] {
        for invalid in [
            "".to_owned(),
            " leading".to_owned(),
            "bad\nvalue".to_owned(),
            "a".repeat(MAX_IDENTIFIER_BYTES + 1),
        ] {
            let mut value = good.clone();
            value["status"]["consent"][field] = json!(invalid);
            assert_eq!(
                decode_reply(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
                NativeWireError::Invalid
            );
        }
    }
    let active = claimed_records("gateway", "org", "gateway").remove(3);
    let value: Value =
        serde_json::from_slice(&encode_status(public(), &projected(active)).unwrap()).unwrap();
    for (field, invalid) in [
        ("credential", json!("")),
        ("receiver", json!("")),
        ("accessEpoch", json!(-1)),
    ] {
        let mut invalid_value = value.clone();
        invalid_value["status"][field] = invalid;
        assert_eq!(
            decode_reply(&serde_json::to_vec(&invalid_value).unwrap()).unwrap_err(),
            NativeWireError::Invalid
        );
    }
    // The current epoch is the one the status carries, not the record's
    // original pair receipt.
    assert_eq!(value["status"]["accessEpoch"], json!(CURRENT_EPOCH));
    assert_ne!(active_record().receiver_binding().unwrap().1, CURRENT_EPOCH);
}

/// An Active status carries the current epoch, and only an Active status does:
/// the projection and the record's phase must agree before anything is encoded.
#[test]
fn only_an_active_record_carries_a_current_epoch() {
    for record in claimed_records("gateway", "org", "gateway") {
        let active = record.phase() == PairingPhase::Active;
        let mismatched = if active {
            DevicePairingStatus::Claimed(Box::new(record))
        } else {
            DevicePairingStatus::Active {
                record: Box::new(record),
                access_epoch: CURRENT_EPOCH,
            }
        };
        assert_eq!(
            encode_status(public(), &mismatched).unwrap_err(),
            NativeWireError::Invalid
        );
    }
}

#[test]
fn public_projection_contains_only_its_declared_fields() {
    let exact_refusal: Value = serde_json::from_slice(&encode_refused().unwrap()).unwrap();
    assert_eq!(exact_refusal, json!({"kind":"refused"}));
    let public_fields = BTreeSet::from([
        "invitation",
        "attempt",
        "consent",
        "generation",
        "expiryMs",
        "class",
    ]);
    let hello: Value = serde_json::from_slice(&encode_hello(public()).unwrap()).unwrap();
    assert_eq!(object_keys(&hello), BTreeSet::from(["kind", "public"]));
    assert_eq!(object_keys(&hello["public"]), public_fields);
    let pending: Value =
        serde_json::from_slice(&encode_status(public(), &DevicePairingStatus::Pending).unwrap())
            .unwrap();
    assert_eq!(object_keys(&pending), BTreeSet::from(["kind", "status"]));
    assert_eq!(
        object_keys(&pending["status"]),
        BTreeSet::from(["stage", "public"])
    );
    assert_eq!(object_keys(&pending["status"]["public"]), public_fields);
    assert_eq!(pending["status"]["stage"], json!("pending"));
    assert_eq!(
        status(&serde_json::to_vec(&pending).unwrap()),
        NativePairingStatus::Pending(public())
    );
}

#[test]
fn unclaimed_projection_preserves_outcome_and_cause_without_private_scope() {
    for outcome in [
        AttemptOutcome::Superseded,
        AttemptOutcome::Failed(AttemptFailure::InvalidProof),
        AttemptOutcome::Failed(AttemptFailure::ConnectionClosed),
        AttemptOutcome::Failed(AttemptFailure::HandshakeDeadline),
        AttemptOutcome::Failed(AttemptFailure::VerifierUnavailable),
    ] {
        for terminal in [
            None,
            Some(TerminalCause::CredentialRevoked),
            Some(TerminalCause::Denied),
            Some(TerminalCause::Cancelled),
            Some(TerminalCause::Expired),
            Some(TerminalCause::Restarted),
        ] {
            let bytes = encode_status(
                public(),
                &DevicePairingStatus::Unclaimed { outcome, terminal },
            )
            .unwrap();
            let projected = status(&bytes);
            assert_eq!(
                projected,
                NativePairingStatus::Unclaimed {
                    public: public(),
                    outcome,
                    terminal
                }
            );
            assert!(projected.consent().is_none());
            projected.correlate(public(), None).unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["status"]["stage"], json!("unclaimed"));
            let expected_outcome = match outcome {
                AttemptOutcome::Superseded => json!({"kind":"superseded"}),
                AttemptOutcome::Failed(cause) => json!({"kind":"failed", "cause": match cause {
                    AttemptFailure::InvalidProof => "invalidProof",
                    AttemptFailure::ConnectionClosed => "connectionClosed",
                    AttemptFailure::HandshakeDeadline => "handshakeDeadline",
                    AttemptFailure::VerifierUnavailable => "verifierUnavailable",
                }}),
                AttemptOutcome::Pending | AttemptOutcome::Claimed => unreachable!(),
            };
            let expected_terminal = match terminal {
                None => Value::Null,
                Some(cause) => json!(match cause {
                    TerminalCause::CredentialRevoked => "credentialRevoked",
                    TerminalCause::Denied => "denied",
                    TerminalCause::Cancelled => "cancelled",
                    TerminalCause::Expired => "expired",
                    TerminalCause::Restarted => "restarted",
                }),
            };
            assert_eq!(value["status"]["outcome"], expected_outcome);
            assert_eq!(value["status"]["terminal"], expected_terminal);
            assert_eq!(
                object_keys(&value["status"]),
                BTreeSet::from(["stage", "public", "outcome", "terminal"])
            );
            assert_eq!(
                object_keys(&value["status"]["public"]),
                BTreeSet::from([
                    "invitation",
                    "attempt",
                    "consent",
                    "generation",
                    "expiryMs",
                    "class"
                ])
            );
        }
    }
    for outcome in [AttemptOutcome::Pending, AttemptOutcome::Claimed] {
        assert_eq!(
            encode_status(
                public(),
                &DevicePairingStatus::Unclaimed {
                    outcome,
                    terminal: None
                }
            ),
            Err(NativeWireError::Correlation(PairingError::Conflict))
        );
    }
}

#[test]
fn canonical_status_refuses_foreign_operation_before_encoding() {
    let original = public();
    for record in claimed_records("gateway", "org", "gateway") {
        encode_status(original, &projected(record.clone())).unwrap();
        for foreign in [
            PublicIntent::new(
                InvitationId::new([9; InvitationId::LENGTH]),
                original.attempt(),
                original.consent(),
                original.generation(),
                original.expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            original.with_attempt(AttemptId::new([9; AttemptId::LENGTH])),
            PublicIntent::new(
                original.invitation(),
                original.attempt(),
                ConsentIntentId::new([9; ConsentIntentId::LENGTH]),
                original.generation(),
                original.expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            PublicIntent::new(
                original.invitation(),
                original.attempt(),
                original.consent(),
                original.generation() + 1,
                original.expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            PublicIntent::new(
                original.invitation(),
                original.attempt(),
                original.consent(),
                original.generation(),
                original.expiry_ms() + 1,
                ConsentClass::DeviceRead,
            )
            .unwrap(),
        ] {
            assert_eq!(
                encode_status(foreign, &projected(record.clone())),
                Err(NativeWireError::Correlation(PairingError::Conflict))
            );
        }
    }
    let initial = initial_record("gateway", "org", "gateway");
    assert_eq!(
        encode_status(original, &projected(initial.clone())),
        Err(NativeWireError::Correlation(PairingError::Conflict))
    );
    let terminal = transition(
        &initial,
        PairingEvent::end(TerminalCause::Cancelled),
        PairingInitiator::Principal(PrincipalId::new("owner").unwrap()),
        1,
    );
    assert_eq!(
        encode_status(original, &projected(terminal)),
        Err(NativeWireError::Correlation(PairingError::Conflict))
    );
}

#[test]
fn received_status_cannot_replace_prior_scope() {
    let approved = claimed_records("gateway", "org", "gateway").remove(1);
    let prior = DisclosedConsent::from_intent(public(), approved.intent()).unwrap();
    let incoming = status(&encode_status(public(), &projected(approved)).unwrap());
    incoming.correlate(public(), None).unwrap();
    incoming.correlate(public(), Some(&prior)).unwrap();
    let foreign_attempt = public().with_attempt(AttemptId::new([9; AttemptId::LENGTH]));
    assert_eq!(
        incoming.correlate(foreign_attempt, None),
        Err(NativeWireError::Correlation(PairingError::Conflict))
    );
    for record in [
        claimed_records("other-gateway", "org", "gateway").remove(1),
        claimed_records("gateway", "other-org", "gateway").remove(1),
        claimed_records("gateway", "org", "other-resource").remove(1),
    ] {
        let replaced = status(&encode_status(public(), &projected(record)).unwrap());
        assert_eq!(
            replaced.correlate(public(), Some(&prior)),
            Err(NativeWireError::Correlation(PairingError::Conflict))
        );
        replaced.correlate(public(), None).unwrap();
    }
    for status_value in [
        DevicePairingStatus::Pending,
        DevicePairingStatus::Unclaimed {
            outcome: AttemptOutcome::Superseded,
            terminal: None,
        },
    ] {
        let preclaim = status(&encode_status(public(), &status_value).unwrap());
        preclaim.correlate(public(), None).unwrap();
        assert_eq!(
            preclaim.correlate(public(), Some(&prior)),
            Err(NativeWireError::Correlation(PairingError::Conflict))
        );
        assert_eq!(
            preclaim.correlate(foreign_attempt, None),
            Err(NativeWireError::Correlation(PairingError::Conflict))
        );
    }
}

fn native_channels(
    gateway: &NativeIdentity,
    device: &NativeIdentity,
) -> (NativeTransport<TcpStream>, NativeTransport<TcpStream>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for stream in [&client, &server] {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
    }
    thread::scope(|scope| {
        let accept = scope.spawn(|| NativeTransport::accept(server, gateway));
        let connected =
            NativeTransport::connect(client, device, GatewayTrust::Pinned(gateway.public_spki()));
        let accepted = accept.join();
        (accepted.unwrap().unwrap(), connected.unwrap())
    })
}

#[test]
fn selected_crypto_messages_fit_the_native_envelope() {
    // Actual public producer roles provide inputs; no enrollment route or receiver runs.
    let operation = PublicIntent::new(
        InvitationId::new([255; InvitationId::LENGTH]),
        AttemptId::new([255; AttemptId::LENGTH]),
        ConsentIntentId::new([255; ConsentIntentId::LENGTH]),
        u64::MAX,
        u64::MAX,
        ConsentClass::DeviceRead,
    )
    .unwrap();
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (server, client) = native_channels(&gateway, &device);
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation = ServerInvitation::register(
        &mut OsEntropy,
        &code,
        operation.invitation(),
        gateway.public_spki(),
    )
    .unwrap();
    let (attempt, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    let begin = encode_request(&NativePairingRequest::Begin {
        public: operation,
        request: request.clone(),
    })
    .unwrap();
    let NativePairingRequest::Begin {
        public: begun,
        request: decoded_request,
    } = decode_request(&begin).unwrap()
    else {
        panic!("expected KE1");
    };
    assert_eq!(begun, operation);
    assert_eq!(decoded_request, request);
    let (retained, response) = invitation
        .start(
            &mut OsEntropy,
            &decoded_request,
            server.pairing_context(operation).unwrap(),
        )
        .unwrap();
    let challenge = encode_challenge(operation, &response).unwrap();
    let NativePairingReply::Challenge {
        public: challenged,
        response: decoded_response,
    } = decode_reply(&challenge).unwrap()
    else {
        panic!("expected KE2");
    };
    assert_eq!(challenged, operation);
    assert_eq!(decoded_response, response);

    let temporary = tempfile::tempdir().unwrap();
    #[cfg(windows)]
    let anchor = temporary.path().join("root");
    #[cfg(not(windows))]
    let anchor = temporary.path().canonicalize().unwrap().join("root");
    nessa_local_storage::create_directory(&anchor).unwrap();
    nessa_local_storage::create_directory_beneath(&anchor, Path::new("private")).unwrap();
    let pending = FilePairingState::open(&anchor, Path::new("private")).unwrap();
    let finalization = attempt
        .finish(
            &mut OsEntropy,
            &code,
            &decoded_response,
            &client.pairing_context(operation).unwrap(),
            |context| {
                pending
                    .save_pending(
                        device.key_material(),
                        &context.gateway_spki(),
                        context.public(),
                        None,
                    )
                    .map_err(|_| PairingCryptoError::PendingStorage)
            },
        )
        .unwrap();
    assert_eq!(pending.load_pending().unwrap().unwrap().intent(), operation);
    let confirm = encode_request(&NativePairingRequest::Confirm {
        public: operation,
        message: finalization.clone(),
    })
    .unwrap();
    let NativePairingRequest::Confirm {
        public: confirmed,
        message: decoded_finalization,
    } = decode_request(&confirm).unwrap()
    else {
        panic!("expected KE3");
    };
    assert_eq!(confirmed, operation);
    assert_eq!(decoded_finalization, finalization);
    retained.finish(&decoded_finalization).unwrap();
    for bytes in [&begin, &challenge, &confirm] {
        assert!(bytes.len() <= MAX_ENROLLMENT_ENVELOPE_BYTES);
    }
}

/// Row H2 (`docs/design/auth/peer-gateways.md`): who an invitation enrolls is
/// public and carried as it is. A peer gateway's Hello names its class, and
/// decodes to the same intent, never to a device's; the two classes are
/// distinct spellings of one field.
#[test]
fn the_enrollment_class_is_carried_and_never_changes_in_transit() {
    let peer = PublicIntent::new(
        public().invitation(),
        public().attempt(),
        public().consent(),
        public().generation(),
        public().expiry_ms(),
        ConsentClass::PeerRead,
    )
    .unwrap();
    let bytes = encode_hello(peer).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["public"]["class"], "peer-gateway-conversation-read");
    assert!(
        matches!(decode_reply(&bytes).unwrap(), NativePairingReply::Hello(decoded) if decoded == peer && decoded != public())
    );
    let device: Value = serde_json::from_slice(&encode_hello(public()).unwrap()).unwrap();
    assert_eq!(device["public"]["class"], "gateway-conversation-read");
    for class in [ConsentClass::DeviceRead, ConsentClass::PeerRead] {
        assert_eq!(ConsentClass::parse(class.as_str()), Some(class));
    }
}
