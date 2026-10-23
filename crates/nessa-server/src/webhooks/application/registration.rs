//! Registration intent is durable before credential issuance. Composition must
//! supervise this use case and serialize all issue modes for a principal.
use super::{MacKey, OutboxError, Registration, Target};
use crate::webhooks::domain::Destination;
use async_trait::async_trait;
use nessa_auth::application::{
    credential_admin::{
        CredentialAdmin, CredentialAdminError, CredentialIssuePurpose, IssueCredentialOutcome,
        IssueCredentialRequest, ListCredentialsRequest,
    },
    dto::CredentialMetadataDto,
    ports::{AccessAdmission, AccessError, Clock},
};
use nessa_auth::domain::{
    Credential, CredentialId, CredentialLifecycle, CredentialTransition, Initiator, IssuanceCause,
    Membership, OrganizationId, PrincipalId, TransitionCause,
};
use std::sync::Arc;
use uuid::Uuid;

/// Private endpoint configuration. Diagnostics omit both values.
#[derive(Clone)]
pub struct RegistrationConfig {
    pub destination: Destination,
    pub key: Arc<MacKey>,
}

/// Selected IDs and original inputs retained through uncertain acknowledgements.
/// No bearer evidence is stored here. Debug is deliberately unavailable.
#[derive(Clone)]
pub struct RegistrationIntent {
    pub id: Uuid,
    pub issue: IssueCredentialRequest,
    pub config: RegistrationConfig,
    pub observed_at_ms: u64,
}

impl RegistrationIntent {
    pub fn registration(&self) -> Result<Registration, OutboxError> {
        if self.issue.issued_at != self.observed_at_ms / 1000 {
            return Err(OutboxError::Invalid);
        }
        Ok(Registration {
            id: self.id,
            target: Target {
                credential: CredentialId::new(&self.issue.credential_id)
                    .map_err(|_| OutboxError::Invalid)?,
                principal: PrincipalId::new(&self.issue.principal.id)
                    .map_err(|_| OutboxError::Invalid)?,
                organization: OrganizationId::new(&self.issue.membership.organization_id)
                    .map_err(|_| OutboxError::Invalid)?,
                owner: PrincipalId::new(&self.issue.issuer_principal_id)
                    .map_err(|_| OutboxError::Invalid)?,
            },
            destination: self.config.destination.clone(),
            key: self.config.key.clone(),
        })
    }
}

#[async_trait]
pub trait RegistrationIntents: Send + Sync {
    /// Same issuer/request and inputs reuse the original selected IDs/time.
    /// Different configuration or caller inputs conflict before auth issuance.
    async fn reserve(
        &self,
        proposed: RegistrationIntent,
    ) -> Result<RegistrationIntent, OutboxError>;
    /// Installation and issue evidence commit together. A matching installed
    /// intent acknowledges its original registration after audit publication.
    async fn install(&self, intent: RegistrationIntent) -> Result<(), OutboxError>;
    /// Only a proven unissued/conflicting intent may be discarded. An installed
    /// registration and its retained cause cannot be erased by this operation.
    async fn discard(&self, id: Uuid) -> Result<(), OutboxError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationFailure {
    Auth(CredentialAdminError),
    Store(OutboxError),
    Inconsistent,
    Stopped,
}

pub struct RegisterWebhook<'a> {
    pub credentials: &'a dyn CredentialAdmin,
    pub admission: &'a dyn AccessAdmission,
    pub intents: &'a dyn RegistrationIntents,
    pub clock: &'a dyn Clock,
}

impl RegisterWebhook<'_> {
    /// This is the inner command, not a socket lifetime owner or W1 gate. The
    /// runtime coordinator must retain that ownership across caller loss.
    pub async fn execute(
        &self,
        mut proposed: RegistrationIntent,
    ) -> Result<IssueCredentialOutcome, RegistrationFailure> {
        proposed.issue.purpose = CredentialIssuePurpose::Webhook;
        proposed.issue.issued_at = proposed.observed_at_ms / 1000;
        let intent = self
            .intents
            .reserve(proposed)
            .await
            .map_err(RegistrationFailure::Store)?;
        let outcome = match self.credentials.issue(intent.issue.clone()).await {
            Ok(outcome) => outcome,
            Err(error @ (CredentialAdminError::Conflict | CredentialAdminError::Capacity)) => {
                // These auth outcomes did not issue this command. An uncertain
                // storage failure is handled below without discarding intent.
                self.intents
                    .discard(intent.id)
                    .await
                    .map_err(RegistrationFailure::Store)?;
                return Err(RegistrationFailure::Auth(error));
            }
            Err(error) => return Err(RegistrationFailure::Auth(error)),
        };
        let (metadata, transitions) = match &outcome {
            IssueCredentialOutcome::Issued {
                metadata,
                transitions,
                ..
            }
            | IssueCredentialOutcome::ExistingSecretUnavailable {
                metadata,
                transitions,
            } => (metadata, transitions),
        };
        if matches!(
            &outcome,
            IssueCredentialOutcome::ExistingSecretUnavailable { .. }
        ) && metadata.id != intent.issue.credential_id
        {
            // A retired request may replay an older selected identity. Ask the
            // auth owner whether this proposal was issued; a diagnostic from
            // hold() is not proof of absence or rollback.
            let current = self
                .credentials
                .list(ListCredentialsRequest {
                    organization_id: intent.issue.membership.organization_id.clone(),
                })
                .await
                .map_err(RegistrationFailure::Auth)?;
            if current
                .iter()
                .any(|saved| saved.organization_id != intent.issue.membership.organization_id)
                || !current.iter().any(|saved| saved.id == metadata.id)
            {
                return Err(RegistrationFailure::Inconsistent);
            }
            if !current
                .iter()
                .any(|saved| saved.id == intent.issue.credential_id)
            {
                self.intents
                    .discard(intent.id)
                    .await
                    .map_err(RegistrationFailure::Store)?;
                return Err(RegistrationFailure::Auth(CredentialAdminError::Conflict));
            }
        }
        let expected = CredentialTransition::new(
            CredentialId::new(&intent.issue.credential_id)
                .map_err(|_| RegistrationFailure::Inconsistent)?,
            None,
            CredentialLifecycle {
                issued_at: intent.issue.issued_at,
                expires_at: intent.issue.expires_at,
                revoked_at: None,
            },
            TransitionCause::Issued(IssuanceCause::AdminIssue),
            Initiator::Principal(
                PrincipalId::new(&intent.issue.issuer_principal_id)
                    .map_err(|_| RegistrationFailure::Inconsistent)?,
            ),
            intent.issue.issued_at,
        )
        .map_err(|_| RegistrationFailure::Inconsistent)?;
        if !matches_request(&intent.issue, metadata)
            || transitions.len() != 1
            || transitions[0].correlation.as_deref() != Some(&intent.issue.request_id)
            || CredentialTransition::try_from(transitions[0].clone()).as_ref() != Ok(&expected)
        {
            return Err(RegistrationFailure::Inconsistent);
        }
        let credential = Credential::try_from(metadata.clone())
            .map_err(|_| RegistrationFailure::Inconsistent)?;
        let membership = Membership::try_from(intent.issue.membership.clone())
            .map_err(|_| RegistrationFailure::Inconsistent)?;
        let hold = self
            .admission
            .hold(credential.id())
            .await
            .map_err(|error| match error {
                AccessError::Unavailable
                | AccessError::StaleRevision
                | AccessError::Unsupported => {
                    RegistrationFailure::Auth(CredentialAdminError::Unavailable)
                }
                AccessError::InvalidCredential
                | AccessError::Denied
                | AccessError::CredentialRevoked
                | AccessError::CredentialExpired
                | AccessError::InactiveMembership
                | AccessError::IdentityMismatch => RegistrationFailure::Stopped,
            })?;
        if !hold.snapshot.membership.is_active()
            || !hold
                .snapshot
                .credential
                .is_valid_at(self.clock.unix_seconds())
        {
            return Err(RegistrationFailure::Stopped);
        }
        if hold.snapshot.credential != credential || hold.snapshot.membership != membership {
            return Err(RegistrationFailure::Inconsistent);
        }
        self.intents
            .install(intent)
            .await
            .map_err(RegistrationFailure::Store)?;
        drop(hold);
        Ok(outcome)
    }
}

fn matches_request(request: &IssueCredentialRequest, metadata: &CredentialMetadataDto) -> bool {
    metadata.id == request.credential_id
        && metadata.principal_id == request.principal.id
        && metadata.organization_id == request.membership.organization_id
        && metadata.audience_id == request.audience_id
        && metadata.issued_at == request.issued_at
        && metadata.expires_at == request.expires_at
        && metadata.grants == request.grants
}
