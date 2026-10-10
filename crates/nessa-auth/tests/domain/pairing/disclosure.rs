use nessa_auth::domain::{
    pairing::{
        AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DisclosedConsent, InvitationId,
        PairingError, PublicIntent,
    },
    Action, AudienceId, Grant, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
};
fn resource(org: &str, id: &str) -> Resource {
    Resource::new(
        OrganizationId::new(org).unwrap(),
        ResourceId::new(id).unwrap(),
    )
}
fn public() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        600_000,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
fn canonical() -> ConsentIntent {
    ConsentIntent::new(
        public().consent(),
        1,
        AudienceId::new("gateway-1").unwrap(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("membership").unwrap(),
        resource("org-1", "gateway-1"),
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
#[test]
fn consent_generation_floor_and_read_class_are_owned_at_construction() {
    let intent = canonical();
    assert_eq!(
        ConsentIntent::new(
            intent.id(),
            0,
            intent.audience().clone(),
            intent.owner().clone(),
            intent.membership().clone(),
            intent.resource().clone(),
            ConsentClass::DeviceRead,
        ),
        Err(PairingError::Invalid)
    );
    let accepted = ConsentIntent::new(
        intent.id(),
        1,
        intent.audience().clone(),
        intent.owner().clone(),
        intent.membership().clone(),
        intent.resource().clone(),
        ConsentClass::DeviceRead,
    )
    .unwrap();
    assert_eq!(accepted.generation(), 1);
    assert_eq!(
        accepted.grant(),
        &Grant::new(
            Action::new("conversation.read").unwrap(),
            intent.resource().clone(),
        )
    );
}

#[test]
fn public_intent_refuses_each_zero_bound_and_accepts_positive_neighbor() {
    let intent = public();
    for (generation, expiry) in [(0, 1), (1, 0)] {
        assert_eq!(
            PublicIntent::new(
                intent.invitation(),
                intent.attempt(),
                intent.consent(),
                generation,
                expiry,
                ConsentClass::DeviceRead,
            ),
            Err(PairingError::Invalid)
        );
    }
    let accepted = PublicIntent::new(
        intent.invitation(),
        intent.attempt(),
        intent.consent(),
        1,
        1,
        ConsentClass::DeviceRead,
    )
    .unwrap();
    assert_eq!(accepted.generation(), 1);
    assert_eq!(accepted.expiry_ms(), 1);
}

#[test]
fn disclosure_correlates_whole_canonical_intent() {
    let intent = canonical();
    let expected = DisclosedConsent::from_intent(public(), &intent).unwrap();
    expected.verify_intent(&intent).unwrap();
    expected.correlate(public(), None).unwrap();
    for foreign in [
        DisclosedConsent::new(
            PublicIntent::new(
                public().invitation(),
                public().attempt(),
                ConsentIntentId::new([9; 16]),
                1,
                public().expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            intent.audience().clone(),
            intent.grant().clone(),
        )
        .unwrap(),
        DisclosedConsent::new(
            PublicIntent::new(
                public().invitation(),
                public().attempt(),
                public().consent(),
                2,
                public().expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            intent.audience().clone(),
            intent.grant().clone(),
        )
        .unwrap(),
        DisclosedConsent::new(
            public(),
            AudienceId::new("gateway-2").unwrap(),
            intent.grant().clone(),
        )
        .unwrap(),
        DisclosedConsent::new(
            public(),
            intent.audience().clone(),
            ConsentIntent::read_grant(resource("org-2", "gateway-1")).unwrap(),
        )
        .unwrap(),
        DisclosedConsent::new(
            public(),
            intent.audience().clone(),
            ConsentIntent::read_grant(resource("org-1", "gateway-2")).unwrap(),
        )
        .unwrap(),
    ] {
        assert_eq!(foreign.verify_intent(&intent), Err(PairingError::Conflict));
    }
    assert_eq!(
        DisclosedConsent::new(
            public(),
            intent.audience().clone(),
            Grant::new(
                Action::new("conversation.write").unwrap(),
                intent.resource().clone()
            )
        ),
        Err(PairingError::Invalid)
    );
    assert_eq!(
        DisclosedConsent::from_intent(
            PublicIntent::new(
                public().invitation(),
                public().attempt(),
                public().consent(),
                2,
                public().expiry_ms(),
                ConsentClass::DeviceRead,
            )
            .unwrap(),
            &intent
        ),
        Err(PairingError::Conflict)
    );
}
#[test]
fn disclosure_retains_received_scope_on_retry() {
    let intent = canonical();
    let received = DisclosedConsent::from_intent(public(), &intent).unwrap();
    received.correlate(public(), Some(&received)).unwrap();
    let foreign_private = DisclosedConsent::new(
        public(),
        AudienceId::new("gateway-2").unwrap(),
        ConsentIntent::read_grant(resource("org-2", "gateway-2")).unwrap(),
    )
    .unwrap();
    // The first client disclosure cannot infer undisclosed selectors from opaque IDs.
    foreign_private.correlate(public(), None).unwrap();
    assert_eq!(
        foreign_private.correlate(public(), Some(&received)),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        foreign_private.verify_intent(&intent),
        Err(PairingError::Conflict)
    );
    for pending in [
        PublicIntent::new(
            InvitationId::new([9; 16]),
            public().attempt(),
            public().consent(),
            1,
            public().expiry_ms(),
            ConsentClass::DeviceRead,
        )
        .unwrap(),
        PublicIntent::new(
            public().invitation(),
            AttemptId::new([9; 16]),
            public().consent(),
            1,
            public().expiry_ms(),
            ConsentClass::DeviceRead,
        )
        .unwrap(),
        PublicIntent::new(
            public().invitation(),
            public().attempt(),
            ConsentIntentId::new([9; 16]),
            1,
            public().expiry_ms(),
            ConsentClass::DeviceRead,
        )
        .unwrap(),
        PublicIntent::new(
            public().invitation(),
            public().attempt(),
            public().consent(),
            2,
            public().expiry_ms(),
            ConsentClass::DeviceRead,
        )
        .unwrap(),
        PublicIntent::new(
            public().invitation(),
            public().attempt(),
            public().consent(),
            1,
            public().expiry_ms() + 1,
            ConsentClass::DeviceRead,
        )
        .unwrap(),
    ] {
        assert_eq!(
            received.correlate(pending, None),
            Err(PairingError::Conflict)
        );
    }
}
