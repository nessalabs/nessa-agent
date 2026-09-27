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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// The lifecycle fields a credential transition can change.
pub struct CredentialLifecycleDto {
    /// Inclusive start of validity in Unix seconds.
    pub issued_at: u64,
    /// Exclusive expiry in Unix seconds.
    pub expires_at: Option<u64>,
    /// Recorded revocation time in Unix seconds, or None.
    pub revoked_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Which command created a credential.
pub enum IssuanceCauseDto {
    Bootstrap,
    AdminIssue,
    SurfaceProvision,
    OwnerRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Which automatic command replaced a credential.
pub enum SupersessionDto {
    Provision,
    OwnerRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
/// Why a credential stopped being valid.
pub enum RevocationCauseDto {
    /// Deliberately revoked by the initiator.
    Explicit,
    /// Automatically replaced by credential `by` as part of the named command.
    #[serde(rename_all = "camelCase")]
    Superseded {
        by: String,
        supersession: SupersessionDto,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
/// Serialized lifecycle cause. Expiry is never a cause: nothing happens then.
pub enum TransitionCauseDto {
    Issued { cause: IssuanceCauseDto },
    Revoked { cause: RevocationCauseDto },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "kind")]
/// Serialized initiator. Parsing one does not verify anybody.
pub enum InitiatorDto {
    /// Verified principal that issued the command.
    Principal { id: String },
    /// Operating-system owner running an offline command.
    LocalOperator,
}

/// One committed credential lifecycle change, secret-free. `sequence`,
/// `revision`, and `correlation` are assigned by the storage adapter that
/// committed it; the rest is the domain evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CredentialTransitionDto {
    /// Position in the registry's append-only transition list, from 1.
    pub sequence: u64,
    /// Registry revision that committed this transition.
    pub revision: u64,
    /// The command's idempotency key, when the command had one.
    pub correlation: Option<String>,
    /// Credential this transition changed.
    pub credential_id: String,
    /// Lifecycle before the change; None for issuance.
    pub before: Option<CredentialLifecycleDto>,
    /// Lifecycle after the change.
    pub after: CredentialLifecycleDto,
    /// Why it happened.
    pub cause: TransitionCauseDto,
    /// Who caused it.
    pub initiator: InitiatorDto,
    /// The command's own time in Unix seconds, not an observation time.
    pub at: u64,
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
    fn transition_round_trips_and_names_its_cause_explicitly() {
        let transition = CredentialTransitionDto {
            sequence: 2,
            revision: 3,
            correlation: Some("request-1".into()),
            credential_id: "credential-1".into(),
            before: Some(CredentialLifecycleDto {
                issued_at: 100,
                expires_at: None,
                revoked_at: None,
            }),
            after: CredentialLifecycleDto {
                issued_at: 100,
                expires_at: None,
                revoked_at: Some(150),
            },
            cause: TransitionCauseDto::Revoked {
                cause: RevocationCauseDto::Superseded {
                    by: "credential-2".into(),
                    supersession: SupersessionDto::Provision,
                },
            },
            initiator: InitiatorDto::Principal {
                id: "principal-1".into(),
            },
            at: 150,
        };
        let json = serde_json::to_string(&transition).expect("transition serializes");
        assert!(json.contains(r#""kind":"superseded""#));
        assert!(json.contains(r#""supersession":"provision""#));
        assert!(!json.contains("verifier"));
        assert_eq!(
            serde_json::from_str::<CredentialTransitionDto>(&json).expect("deserializes"),
            transition
        );
        assert!(serde_json::from_str::<CredentialTransitionDto>(
            &json.replace(r#""at":150"#, r#""at":150,"secret":"x""#)
        )
        .is_err());
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
