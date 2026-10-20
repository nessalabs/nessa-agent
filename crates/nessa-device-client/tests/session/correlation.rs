//! Foreign passive correlation preserves the existing owners' exact values.
use super::*;
use crate::test_support::PrivateFixture;
use nessa_auth::domain::{
    pairing::{AttemptId, ConsentIntentId, DisclosedConsent, InvitationId},
    AudienceId, CredentialId, OrganizationId, Resource, ResourceId,
};
use nessa_client_core::retained::ArtifactFault;
use nessa_local_storage::create_directory_beneath;
use nessa_sync::replication::domain::MAX_ID_BYTES;
use std::{io::ErrorKind, net::TcpListener, path::Path, sync::Arc};

fn intent(class: ConsentClass, attempt: u8) -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([attempt; 16]),
        ConsentIntentId::new([3; 16]),
        u64::MAX,
        u64::MAX,
        class,
    )
    .unwrap()
}
fn consent(public: PublicIntent) -> Box<DisclosedConsent> {
    Box::new(
        DisclosedConsent::with_grants(
            public,
            AudienceId::new("gateway").unwrap(),
            public
                .class()
                .grants(Resource::new(
                    OrganizationId::new("org").unwrap(),
                    ResourceId::new("gateway").unwrap(),
                ))
                .unwrap(),
        )
        .unwrap(),
    )
}
#[test]
fn receipt_public_correlation_preserves_all_fields_for_every_phase_and_class() {
    for class in ConsentClass::ALL {
        let public = intent(class, 2);
        for value in [
            NativePairingStatus::Pending(public),
            NativePairingStatus::Unclaimed {
                public,
                outcome: AttemptOutcome::Pending,
                terminal: None,
            },
            NativePairingStatus::Claimed(consent(public)),
            NativePairingStatus::Approved(consent(public)),
            NativePairingStatus::Staging(consent(public)),
            NativePairingStatus::Active {
                consent: consent(public),
                credential: CredentialId::new("issued").unwrap(),
                receiver: ResourceId::new("receiver").unwrap(),
                access_epoch: u64::MAX,
            },
            NativePairingStatus::Terminal {
                consent: consent(public),
                cause: TerminalCause::CredentialRevoked,
            },
        ] {
            let receipt = NativeEnrollmentReceipt { value };
            let mapped = receipt.public_intent();
            assert_eq!(mapped.invitation(), vec![1; 16]);
            assert_eq!(mapped.attempt(), vec![2; 16]);
            assert_eq!(mapped.consent(), vec![3; 16]);
            assert_eq!(mapped.generation(), u64::MAX);
            assert_eq!(mapped.expiry_ms(), u64::MAX);
            let expected = match class {
                ConsentClass::DeviceRead => NativeConsentClass::DeviceRead,
                ConsentClass::PeerRead => NativeConsentClass::PeerRead,
                ConsentClass::PeerEnvironment => NativeConsentClass::PeerEnvironment,
                ConsentClass::OwnerPhoneReadWrite => NativeConsentClass::OwnerPhoneReadWrite,
            };
            assert_eq!(mapped.consent_class(), expected);
            // Compare the original owner publication byte-for-byte. Enum debug
            // names and normalization are not wire class names.
            let original_name = public.class().as_str();
            assert_eq!(
                mapped.consent_class_name().as_bytes(),
                original_name.as_bytes()
            );
            assert_eq!(receipt.public_intent().consent_class_name(), original_name);
            assert_ne!(mapped.consent_class_name(), format!("{class:?}"));
        }
    }
}
#[test]
fn changed_attempt_is_not_same_intent_but_preserves_the_enrollment_correlation() {
    let original = Arc::new(NativeEnrollmentReceipt {
        value: NativePairingStatus::Pending(intent(ConsentClass::DeviceRead, 2)),
    });
    let retried = Arc::new(NativeEnrollmentReceipt {
        value: NativePairingStatus::Pending(intent(ConsentClass::DeviceRead, 4)),
    });
    assert!(original.same_intent(original.clone()));
    assert!(!original.same_intent(retried.clone()));
    let old = original.public_intent();
    let new = retried.public_intent();
    assert_ne!(old.attempt(), new.attempt());
    assert_eq!(old.invitation(), new.invitation());
    assert_eq!(old.consent(), new.consent());
    assert_eq!(old.generation(), new.generation());
    assert_eq!(old.expiry_ms(), new.expiry_ms());
    assert_eq!(old.consent_class(), new.consent_class());
    assert_eq!(old.consent_class_name(), new.consent_class_name());
}
#[test]
fn saved_cursor_restoration_uses_id_owner_without_narrowing_or_normalizing() {
    for creation in [0, 1, u64::MAX] {
        let key = NativeEntryKey::new(creation, "  saved-id  ".into()).unwrap();
        assert_eq!(key.creation(), creation);
        assert_eq!(key.id(), "  saved-id  ");
    }
    for id in ["", " \t\n"] {
        assert!(matches!(
            NativeEntryKey::new(1, id.into()),
            Err(NativeCallError::InvalidArgument)
        ));
    }
    let maximum = "a".repeat(MAX_ID_BYTES);
    assert_eq!(
        NativeEntryKey::new(1, maximum.clone()).unwrap().id(),
        maximum
    );
    assert!(matches!(
        NativeEntryKey::new(1, "a".repeat(MAX_ID_BYTES + 1)),
        Err(NativeCallError::InvalidArgument)
    ));
    // The Id owner bounds UTF-8 bytes, independently of character counts.
    assert!(matches!(
        NativeEntryKey::new(1, "é".repeat(MAX_ID_BYTES)),
        Err(NativeCallError::InvalidArgument)
    ));
}

#[test]
fn follow_rejects_a_cursor_outside_the_published_contract_before_it_dials() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let session = NativeDeviceSession::open(
        root.path().to_str().unwrap().to_owned(),
        "private".to_owned(),
        address.to_string(),
    )
    .unwrap();
    let conversation = "00000000-0000-4000-8000-000000000001".to_owned();
    let stop = || Arc::new(NativeReadStop::new());
    for (incarnation, position) in [
        (Some(String::new()), Some("1".to_owned())),
        (Some("inc".to_owned()), Some(String::new())),
        (Some("inc".to_owned()), None),
        (None, Some("1".to_owned())),
        (Some("inc".to_owned()), Some("00".to_owned())),
        (Some("inc".to_owned()), Some("01".to_owned())),
        (Some("inc".to_owned()), Some("1a".to_owned())),
        (Some("inc".to_owned()), Some("1".repeat(21))),
        (Some("a".repeat(129)), Some("0".to_owned())),
        (Some("é".repeat(129)), Some("0".to_owned())),
        (
            Some("inc".to_owned()),
            Some("99999999999999999999".to_owned()),
        ),
    ] {
        let label = format!("{incarnation:?}/{position:?}");
        assert!(
            matches!(
                session.follow(conversation.clone(), incarnation, position, stop()),
                Err(NativeCallError::InvalidArgument)
            ),
            "{label}"
        );
    }
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    assert!(super::view_cursor(None, None).unwrap().is_none());
    let longest = "é".repeat(128);
    let position = "18446744073709551615".to_owned();
    let cursor = super::view_cursor(Some(longest.clone()), Some("0".to_owned()))
        .unwrap()
        .unwrap();
    assert_eq!(cursor.incarnation, longest);
    assert_eq!(cursor.position, "0");
    let cursor = super::view_cursor(Some("inc".to_owned()), Some(position.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(cursor.position, position);
    assert!(super::view_cursor(
        Some("inc".to_owned()),
        Some("99999999999999999999".to_owned())
    )
    .is_err());
}

#[test]
fn a_catalogue_presence_id_that_is_not_a_conversation_dials_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let session = NativeDeviceSession::open(
        root.path().to_str().unwrap().to_owned(),
        "private".to_owned(),
        address.to_string(),
    )
    .unwrap();
    let stamp = Arc::new(NativePhoneStamp {
        value: PhoneStamp::issue(uuid::Uuid::nil(), 1, "receiver-a"),
    });
    assert!(matches!(
        session.catalogue_presence(stamp.clone(), "receiver".to_owned()),
        Err(NativeCallError::InvalidArgument)
    ));
    assert!(matches!(
        session.catalogue_presence(stamp, "00000000-0000-4000-8000-000000000001".to_owned()),
        Err(NativeCallError::Session { .. })
    ));
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
}

#[test]
fn a_faulted_fetch_reports_the_bytes_it_resumed_from() {
    let fetch = NativeArtifactFetch {
        value: ArtifactFetch::decided(
            Err(ArtifactFault::progress(ArtifactFaultKind::Integrity, 9, 4)),
            3,
        ),
    };
    assert_eq!(fetch.reached(), Some(9));
    assert_eq!(fetch.resumed_from(), Some(4));
    assert_eq!(fetch.gateway_resumed_from(), None);
    assert_eq!(fetch.elapsed_ms(), 3);
}

#[test]
fn a_malformed_artifact_name_is_an_invalid_argument_and_dials_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let session = NativeDeviceSession::open(
        root.path().to_str().unwrap().to_owned(),
        "private".to_owned(),
        address.to_string(),
    )
    .unwrap();
    let stamp = Arc::new(NativePhoneStamp {
        value: PhoneStamp::issue(uuid::Uuid::nil(), 1, "receiver-a"),
    });
    let stop = Arc::new(NativeReadStop::new());
    let digest = "sha256:fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603";
    let media = "application/octet-stream";
    let output = root.path().join("held.bin");
    let request = |digest: &str, media: &str, size: u64| NativeArtifactRequest {
        conversation: "00000000-0000-4000-8000-000000000001".into(),
        digest: digest.into(),
        media_type: media.into(),
        size,
        output: output.display().to_string(),
    };
    for bad in [
        request("sha256:nope", media, 2),
        request(digest, "Application/octet-stream", 2),
        request(digest, media, 0),
        request(digest, media, 64 * 1024 * 1024 + 1),
    ] {
        assert!(matches!(
            session.fetch_artifact(stamp.clone(), bad, stop.clone()),
            Err(NativeCallError::InvalidArgument)
        ));
    }
    assert!(matches!(
        session.fetch_artifact(stamp, request(digest, media, 64 * 1024 * 1024), stop),
        Err(NativeCallError::Session { .. })
    ));
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
}
