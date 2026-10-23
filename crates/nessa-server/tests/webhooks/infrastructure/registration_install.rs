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

impl SqlOutbox {
    /// Mechanics-only stop input. Like install above, this creates no Auth
    /// journal or permission. Actual composed tests use held Auth facts.
    pub async fn stop(
        &self,
        credential: CredentialId,
        reason: StopReason,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let stop = match reason {
            StopReason::Expired { observed_by } => RegistrationStop::expired(observed_by, at_ms),
            StopReason::Revoked { initiator, cause } => {
                let lookup = credential.clone();
                let issued_at = self
                    .run(move |connection, _| {
                        Ok(by_credential(connection, &lookup)?
                            .map(|saved| saved.issued_at_ms / 1000))
                    })
                    .await?;
                let Some(issued_at) = issued_at else {
                    return Ok(());
                };
                revocation(credential.clone(), issued_at, initiator, cause, at_ms)?
            }
        };
        self.stop_credential(credential, stop).await
    }
}

/// A mechanics input, not a claim that an Auth journal committed this event.
pub(in super::super) fn revocation(
    credential: CredentialId,
    issued_at: u64,
    initiator: nessa_auth::domain::Initiator,
    cause: nessa_auth::domain::RevocationCause,
    at_ms: u64,
) -> Result<RegistrationStop, OutboxError> {
    use nessa_auth::{
        application::{dto::CredentialTransitionDto, revocation::RevocationRecord},
        domain::{CredentialLifecycle, CredentialTransition, TransitionCause},
    };
    let at = at_ms / 1000;
    let before = CredentialLifecycle {
        issued_at,
        expires_at: None,
        revoked_at: None,
    };
    let after = CredentialLifecycle {
        revoked_at: Some(at.max(issued_at)),
        ..before
    };
    let transition = CredentialTransition::new(
        credential.clone(),
        Some(before),
        after,
        TransitionCause::Revoked(cause),
        initiator,
        at,
    )
    .map_err(invalid)?;
    Ok(RegistrationStop::restore_revocation(
        RevocationRecord::decode(CredentialTransitionDto::record(
            &transition,
            1,
            1,
            Some("mechanics-fixture".into()),
        ))
        .map_err(invalid)?,
        at_ms,
    ))
}
