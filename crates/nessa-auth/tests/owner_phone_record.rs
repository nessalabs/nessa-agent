//! Signed ownership is a separate causal path, not a manufactured charged PAKE claim.
use nessa_auth::domain::{
    pairing::{
        AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DeviceKey, InvitationId,
        OwnerPhoneRequestId, PairingError, PairingEvent, PairingInitiator, PairingPhase,
        PairingPolicy, PairingRecord,
    },
    AudienceId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
};
fn initial(class: ConsentClass) -> PairingRecord {
    let intent = ConsentIntent::new(
        ConsentIntentId::new([3; 16]),
        1,
        AudienceId::new("gateway").unwrap(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("member").unwrap(),
        Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
        class,
    )
    .unwrap();
    let ctor = if class.uses_pake() {
        PairingRecord::new
    } else {
        PairingRecord::new_owner_phone
    };
    ctor(
        InvitationId::new([4; 16]),
        intent,
        100,
        PairingPolicy::new(100, 3).unwrap(),
    )
    .unwrap()
}
#[test]
fn signed_owner_approval_is_atomic_distinct_and_requires_exact_owner_before_expiry() {
    let record = initial(ConsentClass::OwnerPhoneReadWrite);
    let request = OwnerPhoneRequestId::new([5; 16]).unwrap();
    let key = DeviceKey::new([6; 32]);
    let event = PairingEvent::owner_phone_approved(request, key, [7; 32]);
    for actor in [
        PairingInitiator::System,
        PairingInitiator::Device(key),
        PairingInitiator::Principal(PrincipalId::new("other").unwrap()),
    ] {
        assert_eq!(
            record.transition(event.clone(), actor, 101).unwrap_err(),
            PairingError::WrongActor
        );
    }
    let owner = PairingInitiator::Principal(record.intent().owner().clone());
    assert_eq!(
        record
            .transition(event.clone(), owner.clone(), 200)
            .unwrap_err(),
        PairingError::Expired
    );
    let transition = record
        .transition(event.clone(), owner.clone(), 101)
        .unwrap();
    transition.verify(&record).unwrap();
    let approved = transition.after();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    assert_eq!(
        approved.owner_phone_approval(),
        Some((request, key, [7; 32]))
    );
    assert_eq!(approved.charged_attempts(), 0);
    assert!(!approved.attempt_pending());
    assert!(approved
        .transition(event.clone(), owner.clone(), 102)
        .is_err());
    for legacy in [
        ConsentClass::DeviceRead,
        ConsentClass::PeerRead,
        ConsentClass::PeerEnvironment,
    ] {
        assert!(initial(legacy)
            .transition(event.clone(), owner.clone(), 101)
            .is_err());
    }
    assert_eq!(
        record
            .transition(
                PairingEvent::reserve(AttemptId::new([8; 16]), key, [9; 32]),
                PairingInitiator::Device(key),
                101
            )
            .unwrap_err(),
        PairingError::Ineligible
    );
    assert!(record
        .transition(
            PairingEvent::claim(AttemptId::new([8; 16]), key),
            PairingInitiator::Device(key),
            101
        )
        .is_err());
}
