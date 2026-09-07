//! Data-transfer objects accepted at the authentication application boundary.
//!
//! These types contain untrusted data. Deserializing one does not establish an
//! authenticated principal or authorize an action. Application use cases must
//! validate the values and resolve current Nessa-owned bindings, memberships,
//! and credential restrictions before constructing an `AuthContext`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Serialized actor classification, not an authority level.
pub enum PrincipalKindDto {
    Human,
    Integration,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// Principal metadata awaiting identifier validation and authorized application handling.
pub struct PrincipalInputDto {
    /// Opaque Nessa record ID; validated when mapped into the domain.
    pub id: String,
    /// Actor category; conveys no permission.
    pub kind: PrincipalKindDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// Organization identifier awaiting validation; parsing does not create a tenant.
pub struct OrganizationInputDto {
    /// Opaque Nessa record ID; validated when mapped into the domain.
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Requested or imported role; only the configured authority may apply it.
pub enum MembershipRoleDto {
    Admin,
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Serialized membership state supplied to an authorized use case.
pub enum MembershipStateDto {
    Active,
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// Membership metadata whose linkage and authority must be checked before persistence.
pub struct MembershipInputDto {
    /// Opaque Nessa record ID; validated when mapped into the domain.
    pub id: String,
    /// Nessa actor linked to this record.
    pub principal_id: String,
    /// Nessa ownership boundary; this input is not proof of membership.
    pub organization_id: String,
    /// Organization role, applied only by an authorized membership operation.
    pub role: MembershipRoleDto,
    /// Current or proposed membership state from the configured authority.
    pub state: MembershipStateDto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// Exact resource ID and claimed ownership; consumers must resolve trusted ownership.
pub struct ResourceDto {
    /// Nessa ownership boundary; this input is not proof of membership.
    pub organization_id: String,
    /// Opaque Nessa record ID; validated when mapped into the domain.
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// Exact action/resource restriction, with no wildcard expansion.
pub struct CredentialGrantDto {
    /// Application-owned action name, matched exactly.
    pub action: String,
    /// Exact resource covered by this restriction.
    pub resource: ResourceDto,
}

/// Public credential data. This DTO must never grow a token, verifier, or
/// other secret-bearing field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CredentialMetadataDto {
    /// Opaque Nessa record ID; validated when mapped into the domain.
    pub id: String,
    /// Nessa actor linked to this record.
    pub principal_id: String,
    /// Nessa ownership boundary; this input is not proof of membership.
    pub organization_id: String,
    /// Nessa deployment allowed to accept the credential.
    pub audience_id: String,
    /// Inclusive start of credential validity in Unix seconds.
    pub issued_at: u64,
    /// Exclusive expiry in Unix seconds.
    pub expires_at: Option<u64>,
    /// Recorded revocation time in Unix seconds, or None.
    pub revoked_at: Option<u64>,
    /// Credential restrictions; all resources must match the credential organization.
    pub grants: Vec<CredentialGrantDto>,
}

/// A normalized external identity that a trusted adapter may emit after it
/// verifies credential evidence. This DTO remains untrusted application data;
/// it carries no Nessa principal, organization, membership, role, or authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExternalIdentityDto {
    /// External issuer identifier to validate against configured trust.
    pub issuer: String,
    /// External subject scoped to its verified issuer; not a Nessa principal ID.
    pub subject: String,
    /// External proof audience, requiring verification by the adapter.
    pub audience: String,
    /// External credential/session reference to resolve through a trusted binding.
    pub credential_reference: String,
    /// Expiration as Unix seconds.
    /// Exclusive expiry in Unix seconds.
    pub expires_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_metadata_round_trips_without_secrets() {
        let metadata = CredentialMetadataDto {
            id: "credential-1".into(),
            principal_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            audience_id: "gateway-1".into(),
            issued_at: 1_725_000_000,
            expires_at: Some(1_725_086_400),
            revoked_at: None,
            grants: vec![CredentialGrantDto {
                action: "server.read".into(),
                resource: ResourceDto {
                    organization_id: "organization-1".into(),
                    id: "gateway-1".into(),
                },
            }],
        };

        let json = serde_json::to_string(&metadata).expect("metadata serializes");
        assert!(!json.contains("token"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("verifier"));
        assert_eq!(
            serde_json::from_str::<CredentialMetadataDto>(&json).expect("metadata deserializes"),
            metadata
        );
    }

    #[test]
    fn external_identity_round_trips_without_local_authority() {
        let identity = ExternalIdentityDto {
            issuer: "https://issuer.example".into(),
            subject: "external-user-7".into(),
            audience: "nessa-gateway".into(),
            credential_reference: "session-42".into(),
            expires_at: 1_725_086_400,
        };

        let json = serde_json::to_string(&identity).expect("external identity serializes");
        assert!(!json.contains("principalId"));
        assert!(!json.contains("organizationId"));
        assert!(!json.contains("role"));
        assert_eq!(
            serde_json::from_str::<ExternalIdentityDto>(&json)
                .expect("external identity deserializes"),
            identity
        );
    }

    #[test]
    fn identity_inputs_round_trip() {
        let principal = PrincipalInputDto {
            id: "principal-1".into(),
            kind: PrincipalKindDto::Human,
        };
        let organization = OrganizationInputDto {
            id: "organization-1".into(),
        };
        let membership = MembershipInputDto {
            id: "membership-1".into(),
            principal_id: principal.id.clone(),
            organization_id: organization.id.clone(),
            role: MembershipRoleDto::Admin,
            state: MembershipStateDto::Active,
        };

        let principal_json = serde_json::to_string(&principal).expect("principal serializes");
        let organization_json =
            serde_json::to_string(&organization).expect("organization serializes");
        let membership_json = serde_json::to_string(&membership).expect("membership serializes");

        assert_eq!(
            serde_json::from_str::<PrincipalInputDto>(&principal_json)
                .expect("principal deserializes"),
            principal
        );
        assert_eq!(
            serde_json::from_str::<OrganizationInputDto>(&organization_json)
                .expect("organization deserializes"),
            organization
        );
        assert_eq!(
            serde_json::from_str::<MembershipInputDto>(&membership_json)
                .expect("membership deserializes"),
            membership
        );
    }

    #[test]
    fn input_dtos_reject_unknown_fields() {
        let principal = r#"{
            "id":"principal-1",
            "kind":"human",
            "trusted":true
        }"#;
        assert!(serde_json::from_str::<PrincipalInputDto>(principal).is_err());

        let external_identity = r#"{
            "issuer":"https://issuer.example",
            "subject":"external-user-7",
            "audience":"nessa-gateway",
            "credentialReference":"session-42",
            "expiresAt":1725086400,
            "role":"admin"
        }"#;
        assert!(serde_json::from_str::<ExternalIdentityDto>(external_identity).is_err());
    }
}
