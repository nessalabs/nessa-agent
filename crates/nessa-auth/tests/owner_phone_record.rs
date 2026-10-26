//! Signed ownership is a separate causal path, not a manufactured charged PAKE
//! claim; an owner phone redeemed by code is the ordinary PAKE path, which the
//! owner approves for the exact key.
use nessa_auth::domain::{
    pairing::{
        AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DeviceKey, InvitationId,
        OwnerPhoneRequestId, PairingError, PairingEvent, PairingInitiator, PairingPhase,
        PairingPolicy, PairingRecord,
    },
    AudienceId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
};
/// A record of `class`, made for a signed original request or redeemed by code.
fn initial(class: ConsentClass, signed: bool) -> PairingRecord {
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
    let ctor = if signed {
        PairingRecord::new_owner_phone
    } else {
        PairingRecord::new
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
    let record = initial(ConsentClass::OwnerPhoneReadWrite, true);
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
    assert_eq!(approved.owner_approved_key(), Some(key));
    assert_eq!(approved.charged_attempts(), 0);
    assert!(!approved.attempt_pending());
    assert!(approved
        .transition(event.clone(), owner.clone(), 102)
        .is_err());
    // A record redeemed by code, of any class, takes no signed approval.
    for class in ConsentClass::ALL {
        assert!(initial(class, false)
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

/// An owner phone redeemed by code takes the ordinary path: one key claims it,
/// and only the owner's approval of that exact key makes it owner-approved.
#[test]
fn an_owner_phone_redeemed_by_code_is_owner_approved_only_for_the_claimed_key() {
    let record = initial(ConsentClass::OwnerPhoneReadWrite, false);
    assert!(!record.signed_original());
    let key = DeviceKey::new([6; 32]);
    let attempt = AttemptId::new([8; 16]);
    let device = PairingInitiator::Device(key);
    let owner = PairingInitiator::Principal(record.intent().owner().clone());
    let reserved = record
        .transition(
            PairingEvent::reserve(attempt, key, [9; 32]),
            device.clone(),
            101,
        )
        .unwrap();
    let claimed = reserved
        .after()
        .transition(PairingEvent::claim(attempt, key), device, 102)
        .unwrap();
    let claimed = claimed.after();
    assert_eq!(claimed.phase(), PairingPhase::Claimed);
    assert_eq!(claimed.owner_approved_key(), None);
    // Another key, the device itself, or a signed approval cannot stand in.
    assert_eq!(
        claimed
            .transition(
                PairingEvent::approve(DeviceKey::new([7; 32]), 1),
                owner.clone(),
                103
            )
            .unwrap_err(),
        PairingError::Conflict
    );
    assert!(claimed
        .transition(
            PairingEvent::approve(key, 1),
            PairingInitiator::Device(key),
            103
        )
        .is_err());
    assert!(claimed
        .transition(
            PairingEvent::owner_phone_approved(
                OwnerPhoneRequestId::new([5; 16]).unwrap(),
                key,
                [7; 32]
            ),
            owner.clone(),
            103
        )
        .is_err());
    let approved = claimed
        .transition(PairingEvent::approve(key, 1), owner, 103)
        .unwrap();
    let approved = approved.after();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    assert_eq!(approved.owner_approved_key(), Some(key));
    assert_eq!(approved.owner_phone_approval(), None);
}

/// A signed owner-phone record admits no code attempt, and a code-redeemed
/// record of any class does.
#[test]
fn only_a_signed_record_refuses_a_code_attempt() {
    let key = DeviceKey::new([6; 32]);
    let reserve = PairingEvent::reserve(AttemptId::new([8; 16]), key, [9; 32]);
    assert_eq!(
        initial(ConsentClass::OwnerPhoneReadWrite, true)
            .transition(reserve.clone(), PairingInitiator::Device(key), 101)
            .unwrap_err(),
        PairingError::Ineligible
    );
    for class in ConsentClass::ALL {
        assert!(initial(class, false)
            .transition(reserve.clone(), PairingInitiator::Device(key), 101)
            .is_ok());
    }
    for class in [
        ConsentClass::DeviceRead,
        ConsentClass::PeerRead,
        ConsentClass::PeerEnvironment,
    ] {
        assert!(PairingRecord::new_owner_phone(
            InvitationId::new([4; 16]),
            initial(class, false).intent().clone(),
            100,
            PairingPolicy::new(100, 3).unwrap(),
        )
        .is_err());
    }
}
