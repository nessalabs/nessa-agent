use super::*;
use nessa_auth::application::{
    ports::{
        AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier, PortFuture,
        VerifiedCredential,
    },
    session::AuthenticateSession,
};
use nessa_auth::domain::{
    AudienceId, Credential, CredentialId, Membership, MembershipId, MembershipRole,
    MembershipStatus, OrganizationId, PrincipalId, ResourceId,
};
use serde_json::json;

struct Identity(AccessSnapshot);
impl CredentialVerifier for Identity {
    fn verify<'a>(
        &'a self,
        _: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async {
            Ok(VerifiedCredential {
                credential_id: self.0.credential.id().clone(),
                expires_at: None,
            })
        })
    }
}
impl AccessReader for Identity {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}
impl Clock for Identity {
    fn unix_milliseconds(&self) -> u64 {
        10_000
    }
}
async fn context(owner: &str, membership: &str, credential: &str, audience: &str) -> AuthContext {
    let principal = PrincipalId::new(owner).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let audience = AudienceId::new(audience).unwrap();
    let identity = Identity({
        let credential = Credential::new(
            CredentialId::new(credential).unwrap(),
            principal.clone(),
            organization.clone(),
            audience.clone(),
            10,
            20,
            vec![],
        )
        .unwrap();
        let membership = Membership::new(
            MembershipId::new(membership).unwrap(),
            principal,
            organization,
            MembershipRole::Admin,
            MembershipStatus::Active,
        );
        let revision = 1;
        // This fixture explicitly creates its own original product issuance.
        let original = nessa_auth::domain::CredentialTransition::new(
            credential.id().clone(),
            None,
            nessa_auth::domain::CredentialLifecycle {
                issued_at: credential.issued_at(),
                expires_at: credential.expires_at(),
                revoked_at: None,
            },
            nessa_auth::domain::TransitionCause::Issued(
                nessa_auth::domain::IssuanceCause::AdminIssue,
            ),
            nessa_auth::domain::Initiator::LocalOperator,
            credential.issued_at(),
        )
        .unwrap();
        let origin = nessa_auth::application::origin::CredentialOrigin::restore(
            nessa_auth::application::dto::CredentialTransitionDto::record(&original, 1, 1, None),
            &credential,
        )
        .unwrap();
        AccessSnapshot {
            revocation: None,
            credential,
            membership,
            revision,
            origin,
        }
    });
    AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: &identity,
        access: &identity,
        clock: &identity,
    }
    .execute(
        &CredentialEvidence::new(b"endpoint-fixture".to_vec()).unwrap(),
        &audience,
    )
    .await
    .unwrap()
    .context()
    .clone()
}
fn gateway() -> Resource {
    Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    )
}

#[tokio::test]
async fn discovery_preserves_each_authenticated_source_and_exact_composed_destination() {
    let endpoint: SocketAddr = "[::1]:19744".parse().unwrap();
    let pin = [7; 44];
    for (owner, membership, credential) in [
        ("owner-a", "membership-a", "credential-a"),
        ("owner-b", "membership-b", "credential-b"),
    ] {
        let result = discovery(
            &context(owner, membership, credential, "gateway").await,
            &gateway(),
            endpoint,
            pin,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&result.source).unwrap(),
            json!({
                "ownerId": owner, "organizationId": "org", "membershipId": membership,
                "audience": "gateway", "credentialId": credential, "resourceId": "gateway"
            })
        );
        assert_eq!(result.native_endpoint, endpoint.to_string());
        assert_eq!(result.gateway_key, vec![7; 44]);
        // Consume the canonical Auth-backed mapper, rather than retype consent rules.
        let restored = owner_phone::context(result).unwrap();
        assert_eq!(restored.source().owner().as_str(), owner);
        assert_eq!(restored.endpoint(), endpoint);
        assert_eq!(restored.gateway_pin(), &pin);
    }
}

#[tokio::test]
async fn discovery_refuses_contradictory_audience_and_unusable_destinations() {
    let destination = "127.0.0.1:19744".parse().unwrap();
    assert!(matches!(
        discovery(
            &context("owner", "member", "credential", "another-gateway").await,
            &gateway(),
            destination,
            [7; 44]
        ),
        Err(PhoneDtoError::Source)
    ));
    for destination in ["127.0.0.1:0", "0.0.0.0:19744", "[::]:19744"] {
        assert!(matches!(
            discovery(
                &context("owner", "member", "credential", "gateway").await,
                &gateway(),
                destination.parse().unwrap(),
                [7; 44]
            ),
            Err(PhoneDtoError::Endpoint)
        ));
    }
}

#[tokio::test]
async fn source_refuses_a_composed_resource_in_another_authenticated_organization() {
    let other = Resource::new(
        OrganizationId::new("another-org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    assert_eq!(
        source(
            &context("owner", "member", "credential", "gateway").await,
            &other
        ),
        Err(PhoneDtoError::Source)
    );
}
