//! One bounded stop representation for live rows, retirement and audit.
use super::{
    audit::{Cause, AUDIT_BYTES},
    integer, invalid, uuid,
};
use crate::webhooks::{
    application::{OutboxError, RegistrationStop, StopFact},
    domain::{ExpiryObserver, StopObservation, StopReason},
};
use nessa_auth::{
    application::{dto::CredentialTransitionDto, revocation::RevocationRecord},
    domain::{Initiator, PrincipalId},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SavedStop {
    cause: Cause,
    initiator: String,
    at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revocation: Option<CredentialTransitionDto>,
}
impl SavedStop {
    pub(super) fn of(stop: &RegistrationStop) -> Self {
        Self {
            cause: super::audit::stop_cause(stop.observation()),
            initiator: actor(stop.observation()),
            at_ms: stop.observation().at_ms(),
            revocation: match stop.fact() {
                StopFact::Revoked(record) => Some(record.to_dto()),
                StopFact::Expired { .. } => None,
            },
        }
    }
    pub(super) fn checked(&self) -> Result<RegistrationStop, OutboxError> {
        integer(self.at_ms)?;
        let stop = match (&self.cause, &self.revocation) {
            (Cause::CredentialRevoked, Some(record)) => RegistrationStop::restore_revocation(
                RevocationRecord::decode(record.clone()).map_err(invalid)?,
                self.at_ms,
            ),
            (Cause::CredentialExpired, None) => {
                let reader = if self.initiator == "restart" {
                    ExpiryObserver::Restart
                } else if let Some(owner) = self.initiator.strip_prefix("owner:") {
                    ExpiryObserver::Owner(PrincipalId::new(owner).map_err(invalid)?)
                } else if let Some(source) = self.initiator.strip_prefix("source:") {
                    ExpiryObserver::Source(uuid(source)?)
                } else {
                    ExpiryObserver::Delivery(uuid(
                        self.initiator
                            .strip_prefix("delivery:")
                            .ok_or(OutboxError::Invalid)?,
                    )?)
                };
                RegistrationStop::expired(reader, self.at_ms)
            }
            _ => return Err(OutboxError::Invalid),
        };
        if Self::of(&stop) != *self {
            return Err(OutboxError::Invalid);
        }
        Ok(stop)
    }
    pub(super) fn encode(&self) -> Result<String, OutboxError> {
        self.checked()?;
        let text = serde_json::to_string(self).map_err(invalid)?;
        if text.len() > AUDIT_BYTES {
            return Err(OutboxError::Invalid);
        }
        Ok(text)
    }
    pub(super) fn decode(text: &str) -> Result<RegistrationStop, OutboxError> {
        if text.len() > AUDIT_BYTES {
            return Err(OutboxError::Invalid);
        }
        let saved: Self = serde_json::from_str(text).map_err(invalid)?;
        saved.checked()
    }
}

pub(super) fn actor(stop: &StopObservation) -> String {
    match stop.reason() {
        StopReason::Revoked { initiator, .. } => match initiator {
            Initiator::Principal(id) => format!("principal:{}", id.as_str()),
            Initiator::LocalOperator => "local_operator".into(),
        },
        StopReason::Expired { observed_by } => match observed_by {
            ExpiryObserver::Owner(id) => format!("owner:{}", id.as_str()),
            ExpiryObserver::Delivery(id) => format!("delivery:{id}"),
            ExpiryObserver::Source(id) => format!("source:{id}"),
            ExpiryObserver::Restart => "restart".into(),
        },
    }
}
