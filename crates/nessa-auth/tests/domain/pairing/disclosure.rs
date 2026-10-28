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
            ConsentClass::DeviceRead
                .grant(resource("org-2", "gateway-1"))
                .unwrap(),
        )
        .unwrap(),
        DisclosedConsent::new(
            public(),
            intent.audience().clone(),
            ConsentClass::DeviceRead
                .grant(resource("org-1", "gateway-2"))
                .unwrap(),
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
        ConsentClass::DeviceRead
            .grant(resource("org-2", "gateway-2"))
            .unwrap(),
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
/// Each class discloses the one grant it asks for and no other: an
/// environment consent carries `environment.serve` and never the read, and a
/// reading consent never `environment.serve`, so a relabelled grant does not
/// decode.
#[test]
fn each_class_discloses_only_its_own_grant() {
    let gateway = resource("org-1", "gateway-1");
    for class in ConsentClass::ALL {
        let public = PublicIntent::new(
            InvitationId::new([1; 16]),
            AttemptId::new([2; 16]),
            ConsentIntentId::new([3; 16]),
            1,
            600_000,
            class,
        )
        .unwrap();
        let intent = ConsentIntent::new(
            public.consent(),
            1,
            AudienceId::new("gateway-1").unwrap(),
            PrincipalId::new("owner").unwrap(),
            MembershipId::new("membership").unwrap(),
            gateway.clone(),
            class,
        )
        .unwrap();
        assert_eq!(intent.grant().action().as_str(), class.grant_action());
        DisclosedConsent::from_intent(public, &intent).unwrap();
        for other in ["conversation.read", "environment.serve"] {
            if other == class.grant_action() {
                continue;
            }
            assert_eq!(
                DisclosedConsent::new(
                    public,
                    intent.audience().clone(),
                    Grant::new(Action::new(other).unwrap(), gateway.clone()),
                ),
                Err(PairingError::Invalid),
                "{class:?} disclosing {other}"
            );
        }
    }
    assert_eq!(
        ConsentClass::PeerEnvironment.grant_action(),
        "environment.serve"
    );
    assert_eq!(ConsentClass::PeerRead.grant_action(), "conversation.read");
    assert_eq!(ConsentClass::DeviceRead.grant_action(), "conversation.read");
}

#[test]
fn owner_phone_disclosure_requires_full_same_resource_ordered_capabilities() {
    let gateway = resource("org-1", "gateway-1");
    let class = ConsentClass::OwnerPhoneReadWrite;
    let public = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        600_000,
        class,
    )
    .unwrap();
    let audience = AudienceId::new("gateway-1").unwrap();
    let grants = class.grants(gateway.clone()).unwrap();
    assert_eq!(
        grants
            .iter()
            .map(|g| g.action().as_str())
            .collect::<Vec<_>>(),
        ["conversation.read", "conversation.write", "server.read"]
    );
    assert!(DisclosedConsent::with_grants(public, audience.clone(), grants.clone()).is_ok());
    for omitted in 0..grants.len() {
        let mut input = grants.clone();
        input.remove(omitted);
        assert_eq!(
            DisclosedConsent::with_grants(public, audience.clone(), input),
            Err(PairingError::Invalid)
        );
    }
    let mut extra = grants.clone();
    extra.push(Grant::new(
        Action::new("credential.manage").unwrap(),
        gateway,
    ));
    let mut reordered = grants.clone();
    reordered.swap(1, 2);
    let mut foreign = grants.clone();
    foreign[1] = Grant::new(
        Action::new("conversation.write").unwrap(),
        resource("org-1", "gateway-2"),
    );
    for input in [extra, reordered, foreign] {
        assert_eq!(
            DisclosedConsent::with_grants(public, audience.clone(), input),
            Err(PairingError::Invalid)
        );
    }
    assert_eq!(
        DisclosedConsent::new(public, audience, grants[0].clone()),
        Err(PairingError::Invalid)
    );
    // A device client takes an owner-phone invitation; a peer client does not.
    assert!(ConsentClass::DeviceRead.accepts(class));
    assert!(!ConsentClass::PeerRead.accepts(class));
    assert!(!ConsentClass::PeerEnvironment.accepts(class));
    assert!(!class.accepts(ConsentClass::DeviceRead));
    for legacy in [
        ConsentClass::DeviceRead,
        ConsentClass::PeerRead,
        ConsentClass::PeerEnvironment,
    ] {
        assert!(legacy.accepts(legacy));
        assert_eq!(
            legacy.grants(resource("org-1", "gateway-1")).unwrap().len(),
            1
        );
    }
}
