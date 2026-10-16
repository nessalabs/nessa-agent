//! Independent device-signature boundary and exact original replay identity.
use nessa_auth::{
    adapters::pairing::{verify_owner_phone_request, NativeIdentity},
    application::pairing::PrivateKeyMaterial,
    domain::{
        pairing::{DeviceKey, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource},
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use zeroize::Zeroizing;
fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
fn request(key: DeviceKey, field: usize) -> OwnerPhoneRequest {
    let changed = |i, base: &str| {
        if field == i {
            format!("{base}-other")
        } else {
            base.into()
        }
    };
    let gateway = changed(5, "gateway");
    let source = OwnerPhoneSource::new(
        PrincipalId::new(changed(1, "owner")).unwrap(),
        MembershipId::new(changed(2, "membership")).unwrap(),
        AudienceId::new(gateway.clone()).unwrap(),
        CredentialId::new(changed(3, "bootstrap")).unwrap(),
        Resource::new(
            OrganizationId::new(changed(4, "org")).unwrap(),
            ResourceId::new(gateway).unwrap(),
        ),
    )
    .unwrap();
    OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([if field == 0 { 2 } else { 1 }; 16]).unwrap(),
        source,
        identity(if field == 6 { 11 } else { 10 }).public_spki(),
        format!("127.0.0.1:{}", if field == 7 { 19745 } else { 19744 })
            .parse()
            .unwrap(),
        key,
    )
    .unwrap()
}
#[test]
fn signature_binds_every_original_selector_and_is_not_a_tls_or_pake_proof() {
    let device = identity(20);
    let original = request(device.device_key(), usize::MAX);
    let signature = device.sign_owner_phone_request(&original).unwrap();
    let proof = verify_owner_phone_request(original.clone(), &signature).unwrap();
    assert_eq!(proof.request(), &original);
    assert_eq!(
        proof.digest(),
        verify_owner_phone_request(original.clone(), &signature)
            .unwrap()
            .digest()
    );
    for field in 0..8 {
        assert!(
            verify_owner_phone_request(request(device.device_key(), field), &signature).is_err(),
            "field {field}"
        );
    }
    assert!(
        verify_owner_phone_request(request(identity(21).device_key(), usize::MAX), &signature)
            .is_err()
    );
    assert!(identity(21).sign_owner_phone_request(&original).is_err());
    let relay_signature = device.sign_relay_challenge(&[1; 32]).unwrap();
    assert!(verify_owner_phone_request(original, &relay_signature).is_err());
}
#[test]
fn ambiguous_source_fields_have_distinct_length_delimited_transcripts() {
    let device = identity(20);
    let base = request(device.device_key(), usize::MAX);
    let source = |owner, membership| {
        OwnerPhoneSource::new(
            PrincipalId::new(owner).unwrap(),
            MembershipId::new(membership).unwrap(),
            base.source().audience().clone(),
            base.source().credential().clone(),
            base.source().resource().clone(),
        )
        .unwrap()
    };
    let make = |s| {
        OwnerPhoneRequest::new(
            base.id(),
            s,
            *base.gateway_pin(),
            base.endpoint(),
            base.device_key(),
        )
        .unwrap()
    };
    assert_ne!(
        make(source("ab", "c")).transcript(),
        make(source("a", "bc")).transcript()
    );
    assert!(OwnerPhoneRequestId::new([0; 16]).is_err());
    assert!(OwnerPhoneRequest::new(
        base.id(),
        base.source().clone(),
        *base.gateway_pin(),
        "0.0.0.0:19744".parse().unwrap(),
        base.device_key()
    )
    .is_err());
    assert!(OwnerPhoneRequest::new(
        base.id(),
        base.source().clone(),
        *base.gateway_pin(),
        "127.0.0.1:0".parse().unwrap(),
        base.device_key()
    )
    .is_err());
    let mut invalid_pin = *base.gateway_pin();
    invalid_pin[0] ^= 1;
    let malformed = OwnerPhoneRequest::new(
        base.id(),
        base.source().clone(),
        invalid_pin,
        base.endpoint(),
        base.device_key(),
    )
    .unwrap();
    assert!(device.sign_owner_phone_request(&malformed).is_err());
}
