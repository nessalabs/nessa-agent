use super::{discovery, octets, original, request, PhoneDtoError};
use nessa_auth::domain::{
    pairing::{ConsentClass, DeviceKey, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource},
    AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_protocol::product::generated::PhoneOriginal;
use serde_json::{json, Value};
use std::net::SocketAddr;

fn canonical(endpoint: &str) -> OwnerPhoneRequest {
    let resource = Resource::new(
        OrganizationId::new("organization-one").unwrap(),
        ResourceId::new("gateway-\"one\\").unwrap(),
    );
    let source = OwnerPhoneSource::new(
        PrincipalId::new("owner-one").unwrap(),
        MembershipId::new("member-one").unwrap(),
        AudienceId::new("gateway-\"one\\").unwrap(),
        CredentialId::new("bootstrap-one").unwrap(),
        resource,
    )
    .unwrap();
    let mut pin = [7; 44];
    pin[..12].copy_from_slice(&[48, 42, 48, 5, 6, 3, 43, 101, 112, 3, 33, 0]);
    OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([1; 16]).unwrap(),
        source,
        pin,
        endpoint.parse().unwrap(),
        DeviceKey::new([2; 32]),
    )
    .unwrap()
}
fn wire(request: &OwnerPhoneRequest) -> Value {
    serde_json::to_value(original(request).unwrap()).unwrap()
}
fn decoded(value: Value) -> Result<OwnerPhoneRequest, PhoneDtoError> {
    request(serde_json::from_value::<PhoneOriginal>(value).unwrap())
}
#[test]
fn original_mapping_retains_all_source_fields_key_pin_uuid_and_transcript() {
    for endpoint in ["127.0.0.1:19744", "[::1]:19744"] {
        let expected = canonical(endpoint);
        let mapped = original(&expected).unwrap();
        assert_eq!(mapped.discovery.native_endpoint, endpoint);
        let class = ConsentClass::OwnerPhoneReadWrite;
        let capabilities = class.grants(expected.source().resource().clone()).unwrap();
        assert_eq!(mapped.discovery.consent.class, class.as_str());
        assert_eq!(mapped.discovery.consent.grants.len(), capabilities.len());
        for (wire, domain) in mapped.discovery.consent.grants.iter().zip(capabilities) {
            assert_eq!(wire.action, domain.action().as_str());
            assert_eq!(wire.resource.id, domain.resource().id().as_str());
            assert_eq!(
                wire.resource.organization_id,
                domain.resource().organization_id().as_str()
            );
        }
        let actual = request(mapped).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.transcript(), expected.transcript());
    }
}
#[test]
fn checked_octets_require_exact_width_and_representable_values() {
    for width in [0, 31, 33, 63, 65] {
        let value = vec![1; width];
        let result = if width < 40 {
            octets::<32>(&value).map(|_| ())
        } else {
            octets::<64>(&value).map(|_| ())
        };
        assert_eq!(result, Err(PhoneDtoError::Length));
    }
    for value in [256, u64::MAX] {
        let mut values = [0; 64];
        values[63] = value;
        assert_eq!(octets::<64>(&values), Err(PhoneDtoError::Octet));
    }
    assert_eq!(octets::<32>(&[255; 32]), Ok([255; 32]));
    assert_eq!(octets::<64>(&[0; 64]), Ok([0; 64]));
    let expected = canonical("127.0.0.1:19744");
    for path in [vec!["requestId"], vec!["discovery", "gatewayKey"]] {
        for alteration in ["short", "long", "octet"] {
            let mut value = wire(&expected);
            let mut field = &mut value;
            for key in &path {
                field = &mut field[*key];
            }
            let bytes = field.as_array_mut().unwrap();
            let failure = match alteration {
                "short" => {
                    bytes.pop();
                    PhoneDtoError::Length
                }
                "long" => {
                    bytes.push(json!(0));
                    PhoneDtoError::Length
                }
                _ => {
                    bytes[0] = json!(256);
                    PhoneDtoError::Octet
                }
            };
            assert_eq!(decoded(value).err(), Some(failure));
        }
    }
}
#[test]
fn identity_and_original_refusals_come_from_auth_value_constructors() {
    let expected = canonical("127.0.0.1:19744");
    for field in [
        "ownerId",
        "organizationId",
        "membershipId",
        "audience",
        "credentialId",
        "resourceId",
    ] {
        for invalid in [
            "".to_owned(),
            " leading".to_owned(),
            "control\n".to_owned(),
            "é".repeat(129),
        ] {
            let mut value = wire(&expected);
            value["discovery"]["source"][field] = json!(invalid);
            assert_eq!(decoded(value).err(), Some(PhoneDtoError::Source), "{field}");
        }
    }
    let mut foreign = wire(&expected);
    foreign["discovery"]["source"]["audience"] = json!("another-gateway");
    assert_eq!(decoded(foreign).err(), Some(PhoneDtoError::Source));
    let mut zero = wire(&expected);
    zero["requestId"] = json!(vec![0; 16]);
    assert_eq!(decoded(zero).err(), Some(PhoneDtoError::Original));
}
#[test]
fn usable_endpoint_is_auth_owned_without_constructing_a_fake_request_or_key() {
    let expected = canonical("127.0.0.1:19744");
    for endpoint in ["0.0.0.0:19744", "[::]:19744", "127.0.0.1:0", "[::1]:0"] {
        assert_eq!(
            discovery(
                expected.source(),
                endpoint.parse::<SocketAddr>().unwrap(),
                *expected.gateway_pin()
            )
            .err(),
            Some(PhoneDtoError::Endpoint)
        );
        let mut value = wire(&expected);
        value["discovery"]["nativeEndpoint"] = json!(endpoint);
        assert_eq!(decoded(value).err(), Some(PhoneDtoError::Original));
    }
    for endpoint in ["localhost:19744", "ws://127.0.0.1:19744", "127.0.0.1", ""] {
        let mut value = wire(&expected);
        value["discovery"]["nativeEndpoint"] = json!(endpoint);
        assert_eq!(decoded(value).err(), Some(PhoneDtoError::Endpoint));
    }
}
#[test]
fn consent_compares_complete_ordered_auth_publication_without_retyped_actions() {
    let expected = canonical("127.0.0.1:19744");
    for alteration in [
        "legacy",
        "missing",
        "reversed",
        "extra",
        "foreign-resource",
        "foreign-organization",
    ] {
        let mut value = wire(&expected);
        let consent = &mut value["discovery"]["consent"];
        match alteration {
            "legacy" => consent["class"] = json!(ConsentClass::DeviceRead.as_str()),
            "missing" => {
                consent["grants"].as_array_mut().unwrap().pop();
            }
            "reversed" => consent["grants"].as_array_mut().unwrap().reverse(),
            "extra" => {
                let extra = consent["grants"][0].clone();
                consent["grants"].as_array_mut().unwrap().push(extra);
            }
            "foreign-resource" => consent["grants"][0]["resource"]["id"] = json!("other-gateway"),
            _ => consent["grants"][0]["resource"]["organizationId"] = json!("other-organization"),
        }
        assert_eq!(
            decoded(value).err(),
            Some(PhoneDtoError::Consent),
            "{alteration}"
        );
    }
}
