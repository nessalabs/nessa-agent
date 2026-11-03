//! Complete compact envelopes; real TLS admission and local adoption are separate owners.
use super::super::{
    dto::{WireConsent, WireReply, WireStatus},
    encode,
};
use super::{claimed_records, projected, public, transition};
use crate::pairing::{
    wire::{
        decode_reply, decode_request, encode_owner_phone_recovery, encode_request, encode_status,
        NativePairingReply, NativePairingRequest, NativePairingStatus, NativeWireError,
        MAX_ENROLLMENT_ENVELOPE_BYTES,
    },
    DevicePairingStatus,
};
use nessa_auth::domain::{
    pairing::{
        AttemptFailure, AttemptId, AttemptOutcome, ConsentClass, ConsentIntent, ConsentIntentId,
        DeviceKey, DisclosedConsent, InvitationId, OwnerPhoneRequestId, PairingError, PairingEvent,
        PairingInitiator, PairingPhase, PairingPolicy, PairingRecord, PublicIntent, TerminalCause,
    },
    AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    MAX_IDENTIFIER_BYTES,
};
use serde_json::{json, Value};

fn original() -> OwnerPhoneRequestId {
    OwnerPhoneRequestId::new([255; 16]).unwrap()
}

// Pure domain events describe codec evidence; they are not signature or policy proof.
fn largest_records(escaped: &str) -> Vec<PairingRecord> {
    assert_eq!(escaped.len(), MAX_IDENTIFIER_BYTES);
    let resource = Resource::new(
        OrganizationId::new(escaped).unwrap(),
        ResourceId::new(escaped).unwrap(),
    );
    let intent = ConsentIntent::new(
        ConsentIntentId::new([255; 16]),
        u64::MAX,
        AudienceId::new(escaped).unwrap(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("member").unwrap(),
        resource,
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let policy = PairingPolicy::initial();
    let at = u64::MAX - policy.lifetime_ms();
    let initial =
        PairingRecord::new_owner_phone(InvitationId::new([255; 16]), intent, at, policy).unwrap();
    let device = DeviceKey::new([255; 32]);
    let owner = PairingInitiator::Principal(PrincipalId::new("owner").unwrap());
    let approved = transition(
        &initial,
        PairingEvent::owner_phone_approved(original(), device, [255; 32]),
        owner.clone(),
        at + 1,
    );
    let credential = CredentialId::new(escaped).unwrap();
    let stage = AttemptId::new([255; 16]);
    let staging = transition(
        &approved,
        PairingEvent::stage(credential.clone(), stage, u64::MAX),
        owner.clone(),
        at + 2,
    );
    let paired = transition(
        &staging,
        PairingEvent::receiver(
            credential,
            stage,
            ResourceId::new(escaped).unwrap(),
            1,
            u64::MAX,
        ),
        PairingInitiator::System,
        at + 3,
    );
    let active = transition(
        &paired,
        PairingEvent::activate(u64::MAX),
        owner.clone(),
        at + 4,
    );
    let terminal = transition(
        &active,
        PairingEvent::end(TerminalCause::Cancelled),
        owner,
        at + 5,
    );
    vec![approved, staging, active, terminal]
}

fn reply(record: PairingRecord) -> (Vec<u8>, PublicIntent) {
    let public = PublicIntent::from_record(&record, AttemptId::new(*original().bytes())).unwrap();
    let status = if record.phase() == PairingPhase::Active {
        DevicePairingStatus::Active {
            record: Box::new(record),
            access_epoch: u64::MAX,
            tunnel_secret: crate::tunnel::TunnelSecret::new([9; 32]),
        }
    } else {
        DevicePairingStatus::Claimed(Box::new(record))
    };
    (
        encode_owner_phone_recovery(original(), [255; 32], public, &status).unwrap(),
        public,
    )
}

#[test]
fn recovery_request_retains_exact_original_uuid_and_digest() {
    let request = NativePairingRequest::RecoverOwnerPhone {
        request_id: original(),
        digest: [255; 32],
    };
    let bytes = encode_request(&request).unwrap();
    assert!(bytes.len() <= MAX_ENROLLMENT_ENVELOPE_BYTES);
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["kind"], "recoverOwnerPhone");
    assert_eq!(value["requestId"], json!(vec![255; 16]));
    assert_eq!(value["digest"], json!(vec![255; 32]));
    match decode_request(&bytes).unwrap() {
        NativePairingRequest::RecoverOwnerPhone { request_id, digest } => {
            assert_eq!(request_id, original());
            assert_eq!(digest, [255; 32]);
        }
        _ => panic!("expected exact owner-phone recovery request"),
    }
}

#[test]
fn largest_valid_complete_replies_fit_unchanged_enrollment_bound() {
    assert_eq!(MAX_ENROLLMENT_ENVELOPE_BYTES, 4096);
    for escaped in [
        "\"".repeat(MAX_IDENTIFIER_BYTES),
        "\\".repeat(MAX_IDENTIFIER_BYTES),
    ] {
        for record in largest_records(&escaped) {
            let phase = record.phase();
            let (bytes, expected_public) = reply(record);
            eprintln!(
                "compact {phase:?}, escaped byte {}, complete envelope {} bytes",
                escaped.as_bytes()[0],
                bytes.len()
            );
            assert!(bytes.len() <= MAX_ENROLLMENT_ENVELOPE_BYTES, "{phase:?}");
            let decoded = decode_reply(&bytes).unwrap();
            match decoded {
                NativePairingReply::OwnerPhoneRecovery {
                    request_id,
                    digest,
                    status,
                } => {
                    assert_eq!(request_id, original());
                    assert_eq!(digest, [255; 32]);
                    assert_eq!(status.public(), expected_public);
                    assert_eq!(status.public().expiry_ms(), u64::MAX);
                    let consent = status.consent().unwrap();
                    let canonical = ConsentClass::OwnerPhoneReadWrite
                        .grants(consent.grant().resource().clone())
                        .unwrap();
                    assert!(consent.grants().eq(canonical.iter()));
                    if phase == PairingPhase::Active {
                        match status {
                            NativePairingStatus::Active {
                                credential,
                                receiver,
                                access_epoch,
                                ..
                            } => {
                                assert_eq!(credential.as_str(), escaped);
                                assert_eq!(receiver.as_str(), escaped);
                                assert_eq!(access_epoch, u64::MAX);
                            }
                            _ => panic!("expected Active with original issuance and current epoch"),
                        }
                    }
                }
                _ => panic!("expected complete owner-phone recovery envelope"),
            }
        }
    }
}

#[test]
fn every_terminal_wire_cause_fits_with_maximal_complete_consent() {
    let record = largest_records(&"\"".repeat(MAX_IDENTIFIER_BYTES)).remove(3);
    let public = PublicIntent::from_record(&record, AttemptId::new(*original().bytes())).unwrap();
    let consent = DisclosedConsent::from_intent(public, record.intent()).unwrap();
    // This is a representation-capacity fixture for every published terminal
    // variant, not a claim that these causes came from the same record history.
    for cause in [
        TerminalCause::CredentialRevoked,
        TerminalCause::Denied,
        TerminalCause::Cancelled,
        TerminalCause::Expired,
        TerminalCause::Restarted,
    ] {
        let bytes = encode(&WireReply::OwnerPhoneRecovery {
            request_id: *original().bytes(),
            digest: [255; 32],
            status: WireStatus::Terminal {
                consent: WireConsent::from_domain(&consent),
                cause: cause.into(),
            },
        })
        .unwrap();
        assert!(bytes.len() <= MAX_ENROLLMENT_ENVELOPE_BYTES);
        eprintln!(
            "compact terminal {cause:?}, complete envelope {} bytes",
            bytes.len()
        );
        match decode_reply(&bytes).unwrap() {
            NativePairingReply::OwnerPhoneRecovery {
                status: NativePairingStatus::Terminal { cause: actual, .. },
                ..
            } => {
                assert_eq!(actual, cause);
            }
            _ => panic!("expected original terminal cause"),
        }
    }
}

fn request_value() -> Value {
    serde_json::from_slice(
        &encode_request(&NativePairingRequest::RecoverOwnerPhone {
            request_id: original(),
            digest: [255; 32],
        })
        .unwrap(),
    )
    .unwrap()
}
fn reply_value() -> Value {
    let bytes = reply(largest_records(&"\"".repeat(MAX_IDENTIFIER_BYTES)).remove(2)).0;
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn recovery_arrays_and_original_uuid_refuse_structural_contradictions() {
    for reply in [false, true] {
        let base = if reply {
            reply_value()
        } else {
            request_value()
        };
        for field in ["requestId", "digest"] {
            for alteration in ["short", "long", "octet", "negative", "fraction", "missing"] {
                let mut value = base.clone();
                if alteration == "missing" {
                    value.as_object_mut().unwrap().remove(field);
                } else {
                    let bytes = value[field].as_array_mut().unwrap();
                    match alteration {
                        "short" => {
                            bytes.pop();
                        }
                        "long" => bytes.push(json!(0)),
                        "octet" => bytes[0] = json!(256),
                        "negative" => bytes[0] = json!(-1),
                        "fraction" => bytes[0] = json!(0.5),
                        _ => unreachable!(),
                    }
                }
                let bytes = serde_json::to_vec(&value).unwrap();
                let failure = if reply {
                    decode_reply(&bytes).err()
                } else {
                    decode_request(&bytes).err()
                };
                assert_eq!(
                    failure,
                    Some(NativeWireError::Invalid),
                    "{field}/{alteration}"
                );
            }
        }
        let mut zero = base;
        zero["requestId"] = json!(vec![0; 16]);
        let bytes = serde_json::to_vec(&zero).unwrap();
        let failure = if reply {
            decode_reply(&bytes).err()
        } else {
            decode_request(&bytes).err()
        };
        assert_eq!(
            failure,
            Some(NativeWireError::Correlation(PairingError::Invalid))
        );
    }
}

#[test]
fn recovery_envelopes_refuse_unknown_and_duplicate_fields_and_whole_frame_overflow() {
    for reply in [false, true] {
        let base = if reply {
            reply_value()
        } else {
            request_value()
        };
        let encoded = serde_json::to_vec(&base).unwrap();
        for field in ["kind", "requestId", "digest"] {
            let duplicate = format!(
                "{{\"{field}\":{},{}",
                base[field],
                std::str::from_utf8(&encoded[1..]).unwrap()
            );
            let failure = if reply {
                decode_reply(duplicate.as_bytes()).err()
            } else {
                decode_request(duplicate.as_bytes()).err()
            };
            assert_eq!(failure, Some(NativeWireError::Invalid));
        }
        let mut unknown = base;
        unknown["unexpected"] = json!(true);
        let bytes = serde_json::to_vec(&unknown).unwrap();
        let failure = if reply {
            decode_reply(&bytes).err()
        } else {
            decode_request(&bytes).err()
        };
        assert_eq!(failure, Some(NativeWireError::Invalid));
        let mut exact = encoded;
        exact.resize(MAX_ENROLLMENT_ENVELOPE_BYTES, b' ');
        assert!(if reply {
            decode_reply(&exact).is_ok()
        } else {
            decode_request(&exact).is_ok()
        });
        exact.push(b' ');
        let failure = if reply {
            decode_reply(&exact).err()
        } else {
            decode_request(&exact).err()
        };
        assert_eq!(failure, Some(NativeWireError::TooLarge));
    }
}

#[test]
fn recovery_uses_existing_record_correlation_and_epoch_owner() {
    let mut records = largest_records(&"\\".repeat(MAX_IDENTIFIER_BYTES));
    let approved = records.remove(0);
    let active = records.remove(1);
    let active_public =
        PublicIntent::from_record(&active, AttemptId::new(*original().bytes())).unwrap();
    let wrong = active_public.with_attempt(AttemptId::new([1; 16]));
    assert_eq!(
        encode_owner_phone_recovery(
            original(),
            [255; 32],
            wrong,
            &DevicePairingStatus::Active {
                record: Box::new(active.clone()),
                access_epoch: u64::MAX,
                tunnel_secret: crate::tunnel::TunnelSecret::new([9; 32]),
            }
        )
        .err(),
        Some(NativeWireError::Correlation(PairingError::Conflict))
    );
    assert_eq!(
        encode_owner_phone_recovery(
            original(),
            [255; 32],
            active_public,
            &DevicePairingStatus::Claimed(Box::new(active))
        )
        .err(),
        Some(NativeWireError::Invalid)
    );
    let approved_public =
        PublicIntent::from_record(&approved, AttemptId::new(*original().bytes())).unwrap();
    assert_eq!(
        encode_owner_phone_recovery(
            original(),
            [255; 32],
            approved_public,
            &DevicePairingStatus::Active {
                record: Box::new(approved),
                access_epoch: u64::MAX,
                tunnel_secret: crate::tunnel::TunnelSecret::new([9; 32]),
            }
        )
        .err(),
        Some(NativeWireError::Invalid)
    );
}

#[test]
fn legacy_status_bytes_stay_equal_to_original_fixtures() {
    let fixtures: Vec<String> = serde_json::from_str(include_str!("legacy_status.json")).unwrap();
    let mut statuses = vec![
        DevicePairingStatus::Pending,
        DevicePairingStatus::Unclaimed {
            outcome: AttemptOutcome::Failed(AttemptFailure::InvalidProof),
            terminal: None,
        },
        DevicePairingStatus::Unclaimed {
            outcome: AttemptOutcome::Superseded,
            terminal: Some(TerminalCause::Cancelled),
        },
    ];
    statuses.extend(
        claimed_records("gateway", "org", "gateway")
            .into_iter()
            .map(projected),
    );
    assert_eq!(statuses.len(), fixtures.len());
    for (status, expected) in statuses.iter().zip(fixtures) {
        assert_eq!(
            encode_status(public(), status).unwrap(),
            expected.as_bytes()
        );
    }
}
