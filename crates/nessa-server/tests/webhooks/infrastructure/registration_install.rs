//! Proposed input fixtures for isolated outbox mechanics. This test-only helper
//! creates no auth receipt or permission; composed tests use RegisterWebhook.
use super::*;
use crate::webhooks::application::{RegistrationConfig, RegistrationIntent, RegistrationIntents};
use nessa_auth::application::{
    credential_admin::{CredentialIssuePurpose, IssueCredentialRequest},
    dto::{
        CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
        PrincipalInputDto, PrincipalKindDto, ResourceDto,
    },
};

impl SqlOutbox {
    pub async fn install(&self, registration: Registration, at_ms: u64) -> Result<(), OutboxError> {
        let target = &registration.target;
        let intent = self
            .reserve(RegistrationIntent {
                id: registration.id,
                observed_at_ms: at_ms,
                issue: IssueCredentialRequest {
                    purpose: CredentialIssuePurpose::Webhook,
                    request_id: format!("outbox-fixture:{}", registration.id),
                    issuer_principal_id: target.owner.as_str().into(),
                    credential_id: target.credential.as_str().into(),
                    principal: PrincipalInputDto {
                        id: target.principal.as_str().into(),
                        kind: PrincipalKindDto::Integration,
                    },
                    membership: MembershipInputDto {
                        id: "fixture-membership".into(),
                        principal_id: target.principal.as_str().into(),
                        organization_id: target.organization.as_str().into(),
                        role: MembershipRoleDto::Member,
                        state: MembershipStateDto::Active,
                    },
                    audience_id: "fixture-gateway".into(),
                    issued_at: at_ms / 1000,
                    expires_at: None,
                    grants: vec![CredentialGrantDto {
                        action: "conversation.read".into(),
                        resource: ResourceDto {
                            organization_id: target.organization.as_str().into(),
                            id: "fixture-gateway".into(),
                        },
                    }],
                },
                config: RegistrationConfig {
                    destination: registration.destination,
                    key: registration.key,
                },
            })
            .await?;
        RegistrationIntents::install(self, intent).await
    }
}
