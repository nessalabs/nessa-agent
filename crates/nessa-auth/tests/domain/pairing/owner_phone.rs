//! One endpoint rule before disclosure, signing, or retained authorization.
use nessa_auth::domain::{
    pairing::{
        DeviceKey, OwnerPhoneContext, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource,
        PairingError,
    },
    AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
};

#[test]
fn endpoint_validation_is_shared_by_context_and_signed_request() {
    let source = OwnerPhoneSource::new(
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("membership").unwrap(),
        AudienceId::new("gateway").unwrap(),
        CredentialId::new("bootstrap").unwrap(),
        Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
    )
    .unwrap();
    for (address, accepted) in [
        ("192.0.2.10:47650", true),
        ("[::ffff:192.0.2.10]:47650", true),
        ("10.0.0.1:47650", true),
        ("[2001:db8::1]:47650", true),
        ("[fd00::1]:47650", true),
        ("127.0.0.1:47650", true),
        ("[::1]:47650", true),
        ("0.0.0.0:47650", false),
        ("0.1.2.3:47650", false),
        ("255.255.255.255:47650", false),
        ("224.0.0.1:47650", false),
        ("[::ffff:224.0.0.1]:47650", false),
        ("[::ffff:0.1.2.3]:47650", false),
        ("[::ffff:255.255.255.255]:47650", false),
        ("[::]:47650", false),
        ("[ff02::1]:47650", false),
        ("[fe80::1]:47650", false),
        ("[fe80::1%3]:47650", false),
        ("[febf::1%3]:47650", false),
        ("192.0.2.10:0", false),
        ("[fd00::1]:0", false),
    ] {
        let endpoint = address.parse().unwrap();
        let validation = OwnerPhoneRequest::validate_endpoint(endpoint);
        assert_eq!(
            validation,
            if accepted {
                Ok(())
            } else {
                Err(PairingError::Invalid)
            },
            "{address}"
        );
        let context = OwnerPhoneContext::new(source.clone(), [1; 44], endpoint);
        let request = OwnerPhoneRequest::new(
            OwnerPhoneRequestId::new([1; 16]).unwrap(),
            source.clone(),
            [1; 44],
            endpoint,
            DeviceKey::new([2; 32]),
        );
        assert_eq!(context.is_ok(), accepted, "context: {address}");
        assert_eq!(request.is_ok(), accepted, "request: {address}");
    }
}
