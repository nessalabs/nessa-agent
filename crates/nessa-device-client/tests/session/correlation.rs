//! Foreign passive correlation preserves the existing owners' exact values.
use super::*;
use nessa_auth::domain::{
    pairing::{AttemptId, ConsentIntentId, DisclosedConsent, InvitationId},
    AudienceId, CredentialId, OrganizationId, Resource, ResourceId,
};
use nessa_sync::replication::domain::MAX_ID_BYTES;

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
        DisclosedConsent::new(
            public,
            AudienceId::new("gateway").unwrap(),
            public
                .class()
                .grant(Resource::new(
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
            };
            assert_eq!(mapped.consent_class(), expected);
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
