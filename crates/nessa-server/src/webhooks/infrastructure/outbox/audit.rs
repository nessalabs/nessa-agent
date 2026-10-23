//! Closed historical evidence. Writers and restoration use the same validator;
//! a retired registration's original actor is never read from current authority.
use super::codec::{self, SavedDelivery, SavedScope, SourceStop};
use super::cursor;
use super::registry::Stopped;
use super::stored::CheckedDelivery;
use super::{invalid, uuid};
use crate::webhooks::application::{
    DeliveryCause, OutboxError, QueueInitiator, Registration, StopReason, Target,
};
use crate::webhooks::domain::{Delivery, Phase};
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(super) const AUDIT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Cause {
    CredentialIssue,
    CredentialRevoked,
    CredentialExpired,
    Drained,
    Admission,
    Response,
    Deadline,
    AdmissionRefused,
    Restart,
    SourceChanged,
    SourceCheckpoint,
}

/// Original credential lifecycle evidence, distinct from this observation's time.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SavedStop {
    cause: Cause,
    initiator: String,
    at_ms: u64,
}
impl SavedStop {
    fn of(stop: &Stopped) -> Self {
        Self {
            cause: stop_cause(stop),
            initiator: stop.initiator(),
            at_ms: stop.at_ms,
        }
    }
    fn checked(&self) -> Result<Stopped, OutboxError> {
        check_stop(&self.cause, &self.initiator, self.at_ms)
    }
}

/// No open JSON fields or secret-bearing registration DTO can enter evidence.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields, rename_all_fields = "camelCase")]
pub(super) enum Record {
    #[serde(rename = "webhook_registration")]
    Registration {
        record_id: String,
        registration: String,
        credential: String,
        principal: String,
        organization: String,
        owner: String,
        before: String,
        after: String,
        cause: Cause,
        initiator: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_cause: Option<Cause>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_at_ms: Option<u64>,
        at_ms: u64,
    },
    #[serde(rename = "webhook_delivery")]
    Delivery {
        record_id: String,
        registration: String,
        credential: String,
        principal: String,
        organization: String,
        owner: String,
        delivery_id: String,
        conversation_id: String,
        incarnation: String,
        position: u64,
        scope: Box<SavedScope>,
        source_stop: Option<SourceStop>,
        credential_stop: Option<SavedStop>,
        before_state: Option<SavedDelivery>,
        after_state: SavedDelivery,
        before: String,
        after: String,
        epoch: u64,
        attempt: u8,
        timestamp_ms: Option<u64>,
        due_at_ms: u64,
        cause: Cause,
        initiator: String,
        at_ms: u64,
    },
    #[serde(rename = "webhook_cursor")]
    Cursor {
        record_id: String,
        registration: String,
        conversation_id: String,
        before: String,
        after: String,
        prior_scope: SavedScope,
        prior_position: u64,
        observed_scope: SavedScope,
        cause: Cause,
        initiator: String,
        at_ms: u64,
    },
}

pub(super) enum RegistrationChange<'a> {
    Issue,
    Stop(&'a Stopped),
    Retire(&'a Stopped),
}

pub(super) struct DeliveryEvidence {
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub position: u64,
    pub scope: Scope,
    pub target: Target,
    pub before: Option<Delivery>,
    pub after: Delivery,
    pub source_stop: Option<SourceStop>,
    pub credential_stop: Option<Stopped>,
}
impl DeliveryEvidence {
    pub fn follows(&self, previous: &Self) -> bool {
        self.before.as_ref() == Some(&previous.after)
            && self.registration == previous.registration
            && self.conversation == previous.conversation
            && self.position == previous.position
            && self.scope == previous.scope
            && self.target == previous.target
            && previous
                .source_stop
                .is_none_or(|stop| self.source_stop == Some(stop))
    }
}
impl Record {
    pub fn registration(
        registration: &Registration,
        change: RegistrationChange<'_>,
        at_ms: u64,
    ) -> Self {
        let (before, after, cause, initiator, stop_cause, stop_at_ms) = match change {
            RegistrationChange::Issue => (
                "absent",
                "delivering",
                Cause::CredentialIssue,
                registration.target.owner.as_str().to_owned(),
                None,
                None,
            ),
            RegistrationChange::Stop(stop) => (
                "delivering",
                "stopped",
                stop_cause(stop),
                stop.initiator(),
                None,
                None,
            ),
            RegistrationChange::Retire(stop) => (
                "stopped",
                "absent",
                Cause::Drained,
                stop.initiator(),
                Some(stop_cause(stop)),
                Some(stop.at_ms),
            ),
        };
        Self::Registration {
            record_id: Uuid::new_v4().to_string(),
            registration: registration.id.to_string(),
            credential: registration.target.credential.as_str().into(),
            principal: registration.target.principal.as_str().into(),
            organization: registration.target.organization.as_str().into(),
            owner: registration.target.owner.as_str().into(),
            before: before.into(),
            after: after.into(),
            cause,
            initiator,
            stop_cause,
            stop_at_ms,
            at_ms,
        }
    }
    pub fn delivery(
        saved: &CheckedDelivery,
        target: &Target,
        before: Option<&Delivery>,
        cause: DeliveryCause,
        stop: Option<&Stopped>,
        at_ms: u64,
    ) -> Result<Self, OutboxError> {
        let credential_stop = if cause == DeliveryCause::AuthorityStopped {
            stop.map(SavedStop::of)
        } else {
            None
        };
        let (cause, initiator) = match cause {
            DeliveryCause::AuthorityStopped => {
                let stop = stop.ok_or(OutboxError::Invalid)?;
                (stop_cause(stop), stop.initiator())
            }
            DeliveryCause::SourceChanged { initiator } => {
                (Cause::SourceChanged, initiator.label(saved.registration))
            }
            DeliveryCause::SourceQueued { initiator } => {
                (Cause::SourceCheckpoint, initiator.label(saved.registration))
            }
            DeliveryCause::Restart => (Cause::Restart, "restart".into()),
            other => (
                match other {
                    DeliveryCause::Admission => Cause::Admission,
                    DeliveryCause::Response => Cause::Response,
                    DeliveryCause::Deadline => Cause::Deadline,
                    DeliveryCause::Refused => Cause::AdmissionRefused,
                    _ => return Err(OutboxError::Invalid),
                },
                format!("delivery:{}", saved.delivery.id()),
            ),
        };
        let after = &saved.delivery;
        Ok(Self::Delivery {
            record_id: Uuid::new_v4().to_string(),
            registration: saved.registration.to_string(),
            credential: target.credential.as_str().into(),
            principal: target.principal.as_str().into(),
            organization: target.organization.as_str().into(),
            owner: target.owner.as_str().into(),
            delivery_id: after.id().to_string(),
            conversation_id: saved.conversation.to_string(),
            incarnation: saved.scope.incarnation().as_str().into(),
            position: saved.position,
            scope: Box::new(SavedScope::of(&saved.scope)),
            source_stop: saved.source_stop,
            credential_stop,
            before_state: before.map(SavedDelivery::of),
            after_state: SavedDelivery::of(after),
            before: before.map(|d| codec::phase(d).0).unwrap_or("absent").into(),
            after: codec::phase(after).0.into(),
            epoch: after.epoch(),
            attempt: after.attempts(),
            timestamp_ms: after.timestamp_ms(),
            due_at_ms: codec::phase(after).1,
            cause,
            initiator,
            at_ms,
        })
    }
    pub fn cursor(
        registration: Uuid,
        conversation: &ConversationId,
        prior: &Scope,
        position: u64,
        observed: &Scope,
        initiator: QueueInitiator,
        at_ms: u64,
    ) -> Self {
        Self::Cursor {
            record_id: Uuid::new_v4().to_string(),
            registration: registration.to_string(),
            conversation_id: conversation.to_string(),
            before: "following".into(),
            after: "reset_required".into(),
            prior_scope: SavedScope::of(prior),
            prior_position: position,
            observed_scope: SavedScope::of(observed),
            cause: Cause::SourceChanged,
            initiator: initiator.label(registration),
            at_ms,
        }
    }
    pub fn delivery_evidence(&self) -> Result<Option<DeliveryEvidence>, OutboxError> {
        if let Self::Delivery {
            registration,
            credential,
            principal,
            organization,
            owner,
            conversation_id,
            position,
            scope,
            before_state,
            after_state,
            source_stop,
            credential_stop,
            ..
        } = self
        {
            Ok(Some(DeliveryEvidence {
                registration: uuid(registration)?,
                conversation: ConversationId::new(conversation_id).map_err(invalid)?,
                position: *position,
                scope: scope.checked()?,
                target: checked_target(credential, principal, organization, owner)?,
                before: before_state
                    .as_ref()
                    .map(SavedDelivery::checked)
                    .transpose()?,
                after: after_state.checked()?,
                source_stop: *source_stop,
                credential_stop: credential_stop
                    .as_ref()
                    .map(SavedStop::checked)
                    .transpose()?,
            }))
        } else {
            Ok(None)
        }
    }
    pub fn keys(&self) -> Result<(Uuid, Uuid, Option<Uuid>), OutboxError> {
        let (id, reg, delivery) = match self {
            Self::Registration {
                record_id,
                registration,
                ..
            }
            | Self::Cursor {
                record_id,
                registration,
                ..
            } => (record_id, registration, None),
            Self::Delivery {
                record_id,
                registration,
                delivery_id,
                ..
            } => (record_id, registration, Some(delivery_id)),
        };
        Ok((
            uuid(id)?,
            uuid(reg)?,
            delivery.map(|id| uuid(id)).transpose()?,
        ))
    }
    pub fn encode(&self) -> Result<String, OutboxError> {
        self.checked()?;
        let text = serde_json::to_string(self).map_err(invalid)?;
        if text.len() > AUDIT_BYTES {
            return Err(OutboxError::Invalid);
        }
        Ok(text)
    }
    pub fn decode(text: &str) -> Result<Self, OutboxError> {
        if text.len() > AUDIT_BYTES {
            return Err(OutboxError::Invalid);
        }
        let record: Self = serde_json::from_str(text).map_err(invalid)?;
        record.checked()?;
        Ok(record)
    }
    fn checked(&self) -> Result<(), OutboxError> {
        let (_, registration, delivery_id) = self.keys()?;
        match self {
            Self::Registration {
                credential,
                principal,
                organization,
                owner,
                before,
                after,
                cause,
                initiator,
                stop_cause,
                stop_at_ms,
                at_ms,
                ..
            } => {
                checked_target(credential, principal, organization, owner)?;
                match (
                    before.as_str(),
                    after.as_str(),
                    cause,
                    stop_cause,
                    stop_at_ms,
                ) {
                    ("absent", "delivering", Cause::CredentialIssue, None, None)
                        if initiator == owner => {}
                    (
                        "delivering",
                        "stopped",
                        cause @ (Cause::CredentialRevoked | Cause::CredentialExpired),
                        None,
                        None,
                    ) => {
                        check_stop(cause, initiator, *at_ms)?;
                    }
                    ("stopped", "absent", Cause::Drained, Some(cause), Some(at)) => {
                        check_stop(cause, initiator, *at)?;
                    }
                    _ => return Err(OutboxError::Invalid),
                }
            }
            Self::Delivery {
                conversation_id,
                incarnation,
                position,
                source_stop,
                before,
                after,
                epoch,
                attempt,
                timestamp_ms,
                due_at_ms,
                cause,
                initiator,
                at_ms,
                ..
            } => {
                let evidence = self.delivery_evidence()?.ok_or(OutboxError::Invalid)?;
                let scope = evidence.scope;
                cursor::check_identity(&registration.to_string(), conversation_id, &scope)?;
                let next = evidence.after;
                let previous = evidence.before;
                if next.id() != delivery_id.ok_or(OutboxError::Invalid)?
                    || *position == 0
                    || *position > i64::MAX as u64
                    || scope.incarnation().as_str() != incarnation
                    || scope.access_epoch().as_str() != format!("epoch-{}", next.epoch())
                    || *epoch != next.epoch()
                    || *attempt != next.attempts()
                    || *timestamp_ms != next.timestamp_ms()
                    || (after.as_str(), *due_at_ms) != codec::phase(&next)
                    || before.as_str()
                        != previous
                            .as_ref()
                            .map(|d| codec::phase(d).0)
                            .unwrap_or("absent")
                    || source_stop.is_some() && next.phase() == Phase::Queued
                {
                    return Err(OutboxError::Invalid);
                }
                if !matches!(cause, Cause::CredentialRevoked | Cause::CredentialExpired)
                    && evidence.credential_stop.is_some()
                {
                    return Err(OutboxError::Invalid);
                }
                let transition = match cause {
                    Cause::CredentialRevoked | Cause::CredentialExpired => {
                        let stop = evidence
                            .credential_stop
                            .as_ref()
                            .ok_or(OutboxError::Invalid)?;
                        if stop_cause(stop) != *cause || stop.initiator() != *initiator {
                            return Err(OutboxError::Invalid);
                        }
                        DeliveryCause::AuthorityStopped
                    }
                    Cause::SourceChanged => {
                        let actor = check_reader(initiator, registration)?;
                        if source_stop
                            .as_ref()
                            .is_some_and(|stop| stop.initiator != actor)
                        {
                            return Err(OutboxError::Invalid);
                        }
                        DeliveryCause::SourceChanged { initiator: actor }
                    }
                    Cause::SourceCheckpoint => DeliveryCause::SourceQueued {
                        initiator: check_reader(initiator, registration)?,
                    },
                    Cause::Restart if initiator == "restart" => DeliveryCause::Restart,
                    cause if initiator == &format!("delivery:{}", next.id()) => match cause {
                        Cause::Admission => DeliveryCause::Admission,
                        Cause::Response => DeliveryCause::Response,
                        Cause::Deadline => DeliveryCause::Deadline,
                        Cause::AdmissionRefused => DeliveryCause::Refused,
                        _ => return Err(OutboxError::Invalid),
                    },
                    _ => return Err(OutboxError::Invalid),
                };
                match previous {
                    Some(previous) if transition.allows(&previous, &next, *at_ms) => {}
                    None if matches!(transition, DeliveryCause::SourceQueued { .. })
                        && next == Delivery::queued(next.id(), next.epoch()) => {}
                    _ => return Err(OutboxError::Invalid),
                }
            }
            Self::Cursor {
                conversation_id,
                before,
                after,
                prior_scope,
                prior_position,
                observed_scope,
                cause,
                initiator,
                ..
            } => {
                let prior = prior_scope.checked()?;
                let observed = observed_scope.checked()?;
                cursor::check_replacement(
                    &registration.to_string(),
                    conversation_id,
                    &prior,
                    &observed,
                )?;
                if before != "following"
                    || after != "reset_required"
                    || !matches!(cause, Cause::SourceChanged)
                    || *prior_position > i64::MAX as u64
                {
                    return Err(OutboxError::Invalid);
                }
                check_reader(initiator, registration)?;
            }
        }
        Ok(())
    }
}
pub(super) fn checked_target(
    credential: &str,
    principal: &str,
    organization: &str,
    owner: &str,
) -> Result<Target, OutboxError> {
    Ok(Target {
        credential: CredentialId::new(credential).map_err(invalid)?,
        principal: PrincipalId::new(principal).map_err(invalid)?,
        organization: OrganizationId::new(organization).map_err(invalid)?,
        owner: PrincipalId::new(owner).map_err(invalid)?,
    })
}

pub(super) fn stop_cause(stop: &Stopped) -> Cause {
    match stop.reason() {
        StopReason::Revoked { .. } => Cause::CredentialRevoked,
        StopReason::Expired { .. } => Cause::CredentialExpired,
    }
}
pub(super) fn check_stop(
    cause: &Cause,
    initiator: &str,
    at_ms: u64,
) -> Result<Stopped, OutboxError> {
    let cause = match cause {
        Cause::CredentialRevoked => "credential_revoked",
        Cause::CredentialExpired => "credential_expired",
        _ => return Err(OutboxError::Invalid),
    };
    Stopped::restore(cause, initiator, at_ms)
}
pub(super) fn check_reader(actor: &str, registration: Uuid) -> Result<QueueInitiator, OutboxError> {
    if actor == QueueInitiator::Live.label(registration) {
        Ok(QueueInitiator::Live)
    } else if actor == "restart" {
        Ok(QueueInitiator::Restart)
    } else {
        Err(OutboxError::Invalid)
    }
}
