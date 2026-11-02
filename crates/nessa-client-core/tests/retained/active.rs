//! Exact binding refusal fixtures; gateway authentication is tested by paired integration.
use super::*;
use nessa_auth::{
    application::pairing::{PendingEnrollment, PrivateKeyMaterial},
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntentId, DisclosedConsent, InvitationId, PublicIntent,
            TerminalCause,
        },
        AudienceId, CredentialId, OrganizationId, Resource, ResourceId,
    },
};
use zeroize::Zeroizing;
fn intent(attempt: u8) -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([attempt; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        9,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
fn consent(attempt: u8) -> Box<DisclosedConsent> {
    Box::new(
        DisclosedConsent::new(
            intent(attempt),
            AudienceId::new("gateway").unwrap(),
            ConsentClass::DeviceRead
                .grant(Resource::new(
                    OrganizationId::new("org").unwrap(),
                    ResourceId::new("owner").unwrap(),
                ))
                .unwrap(),
        )
        .unwrap(),
    )
}
fn issued() -> DeviceCredential {
    DeviceCredential::new(
        PendingEnrollment::new(
            PrivateKeyMaterial::new(Zeroizing::new([1; 32])),
            [0; 44],
            intent(2),
        ),
        CredentialId::new("credential").unwrap(),
        ResourceId::new("receiver").unwrap(),
    )
}
fn active() -> NativePairingStatus {
    NativePairingStatus::Active {
        consent: consent(2),
        credential: CredentialId::new("credential").unwrap(),
        receiver: ResourceId::new("receiver").unwrap(),
        access_epoch: 13,
        tunnel_secret: nessa_protocol::tunnel::TunnelSecret::new([9; 32]),
    }
}
#[test]
fn active_device_reader_retains_exact_binding_and_fresh_epoch() {
    let reader = RetainedDeviceReader::from_issued(&active(), issued())
        .unwrap()
        .unwrap();
    let access = reader.access("127.0.0.1:9".parse().unwrap(), "test");
    assert_eq!(access.role, ReaderRole::DeviceOwner);
    assert_eq!(access.receiver, "receiver");
    assert_eq!(access.credential, "credential");
    assert_eq!(access.access_epoch, 13);
    assert_eq!(access.pin, [0; 44]);
}
#[test]
fn changed_intent_credential_or_receiver_refuses_before_read_selection() {
    let mut different_intent = active();
    let mut different_credential = active();
    let mut different_receiver = active();
    if let NativePairingStatus::Active { consent: value, .. } = &mut different_intent {
        *value = consent(4);
    }
    if let NativePairingStatus::Active { credential, .. } = &mut different_credential {
        *credential = CredentialId::new("different").unwrap();
    }
    if let NativePairingStatus::Active { receiver, .. } = &mut different_receiver {
        *receiver = ResourceId::new("different").unwrap();
    }
    for status in [different_intent, different_credential, different_receiver] {
        assert!(matches!(
            RetainedDeviceReader::from_issued(&status, issued()),
            Err(NativeClientError::Storage(PrivateStateError::Conflict))
        ));
    }
}
#[test]
fn pending_and_terminal_select_no_reader_and_invalid_sync_receiver_is_typed() {
    for status in [
        NativePairingStatus::Pending(intent(2)),
        NativePairingStatus::Terminal {
            consent: consent(2),
            cause: TerminalCause::CredentialRevoked,
        },
    ] {
        assert!(RetainedDeviceReader::from_issued(&status, issued())
            .unwrap()
            .is_none());
    }
    let mut oversized = active();
    if let NativePairingStatus::Active { receiver, .. } = &mut oversized {
        *receiver =
            ResourceId::new("r".repeat(nessa_sync::replication::domain::MAX_ID_BYTES + 1)).unwrap();
    }
    assert!(matches!(
        RetainedDeviceReader::from_issued(&oversized, issued()),
        Err(NativeClientError::Wire(NativeWireError::Invalid))
    ));
}
