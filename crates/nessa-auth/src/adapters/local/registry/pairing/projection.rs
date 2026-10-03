//! Pairing history lives inside the existing registry, with one replay owner.
use crate::application::dto::{
    CredentialMetadataDto, CredentialTransitionDto, InitiatorDto, IssuanceCauseDto,
    TransitionCauseDto,
};
use crate::domain::{
    pairing::{
        AttemptFailure, AttemptId, ConsentIntent, ConsentIntentId, DeviceKey, Event, InvitationId,
        PairingError, PairingEvent, PairingInitiator, PairingPhase, PairingPolicy, PairingRecord,
        PairingTransition, TerminalCause,
    },
    AudienceId, CredentialId, CredentialTransition, MembershipId, OrganizationId, PrincipalId,
    Resource, ResourceId,
};
use serde::{Deserialize, Serialize};

const MAX_HISTORY_STEPS: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct StoredPairing {
    invitation: [u8; 16],
    consent: [u8; 16],
    generation: u64,
    audience: String,
    owner: String,
    membership: String,
    organization: String,
    resource: String,
    created_at_ms: u64,
    lifetime_ms: u64,
    attempts: u8,
    history: Vec<StoredStep>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredStep {
    event: StoredEvent,
    actor: StoredActor,
    ordering_time_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields, rename_all = "camelCase")]
enum StoredActor {
    Principal { id: String },
    Device { key: [u8; 32] },
    LocalOperator,
    System,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields, rename_all = "camelCase")]
enum StoredEvent {
    CredentialRevoked {
        sequence: u64,
    },
    Reserve {
        id: [u8; 16],
        key: [u8; 32],
        input: [u8; 32],
    },
    Fail {
        id: [u8; 16],
        key: [u8; 32],
        cause: StoredFailure,
    },
    Claim {
        id: [u8; 16],
        key: [u8; 32],
    },
    Approve {
        key: [u8; 32],
        generation: u64,
    },
    Stage {
        credential: String,
        request: [u8; 16],
        generation: u64,
    },
    Receiver {
        credential: String,
        request: [u8; 16],
        receiver: String,
        epoch: u64,
        generation: u64,
    },
    Activate {
        generation: u64,
    },
    End {
        cause: StoredTerminal,
    },
    NoReceiverCleanup {
        credential: String,
        request: [u8; 16],
        generation: u64,
    },
    Cleanup {
        credential: String,
        request: [u8; 16],
        receiver: String,
        epoch: u64,
    },
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredFailure {
    InvalidProof,
    ConnectionClosed,
    HandshakeDeadline,
    VerifierUnavailable,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredTerminal {
    CredentialRevoked,
    Denied,
    Cancelled,
    Expired,
    Restarted,
}

impl StoredPairing {
    pub(crate) fn new(record: &PairingRecord) -> Result<Self, PairingError> {
        if record.phase() != PairingPhase::Available || record.charged_attempts() != 0 {
            return Err(PairingError::Conflict);
        }
        let intent = record.intent();
        Ok(Self {
            invitation: *record.id().bytes(),
            consent: *intent.id().bytes(),
            generation: intent.generation(),
            audience: intent.audience().as_str().to_owned(),
            owner: intent.owner().as_str().to_owned(),
            membership: intent.membership().as_str().to_owned(),
            organization: intent.resource().organization_id().as_str().to_owned(),
            resource: intent.resource().id().as_str().to_owned(),
            created_at_ms: record.created_at_ms(),
            lifetime_ms: record.policy().lifetime_ms(),
            attempts: record.policy().attempts(),
            history: Vec::new(),
        })
    }
    pub(crate) fn was_activated(&self) -> bool {
        self.history
            .iter()
            .any(|step| matches!(step.event, StoredEvent::Activate { .. }))
    }
    pub(crate) fn credential_binding_matches(
        &self,
        metadata: &CredentialMetadataDto,
        generation: u64,
        canonical: &[CredentialTransitionDto],
    ) -> bool {
        let Ok(credential) = super::super::domain_credential(metadata) else {
            return false;
        };
        self.restore(canonical).is_ok_and(|record| {
            let intent = record.intent();
            let live = record.phase() == PairingPhase::Active && metadata.revoked_at.is_none();
            let ended = record.phase() == PairingPhase::Terminal
                && metadata.revoked_at.is_some()
                && self
                    .history
                    .iter()
                    .any(|step| matches!(step.event, StoredEvent::Activate { .. }));
            let original = canonical.iter().find(|entry| entry.credential_id == metadata.id);
            let activation = self.history.iter().find(|step| matches!(step.event, StoredEvent::Activate { .. }));
            let publication_matches = original.zip(activation).is_some_and(|(issued, activated)| {
                CredentialTransition::try_from(issued.clone()).is_ok()
                    && issued.cause == TransitionCauseDto::Issued { cause: IssuanceCauseDto::DevicePairing }
                    && issued.at == activated.ordering_time_ms / 1000
                    && metadata.issued_at == issued.at
                    && matches!((&issued.initiator, &activated.actor),
                        (InitiatorDto::Principal { id: issuer }, StoredActor::Principal { id: actor }) if issuer == actor)
            });
            publication_matches
                && intent.generation() == generation
                && record
                    .credential()
                    .is_some_and(|id| id.as_str() == metadata.id)
                && (live || ended)
                && metadata.principal_id == intent.owner().as_str()
                && credential.organization_id() == intent.resource().organization_id()
                && metadata.audience_id == intent.audience().as_str()
                && metadata.expires_at.is_none()
                && metadata.grants.len() == 1
                && metadata.grants[0].action == intent.grant().action().as_str()
                && metadata.grants[0].resource.id == intent.resource().id().as_str()
        })
    }
    pub(crate) fn consume_revocation(
        &mut self,
        transition: &CredentialTransition,
        canonical: &[CredentialTransitionDto],
    ) -> Result<(), PairingError> {
        let record = self.restore(canonical)?;
        if record.credential() != Some(transition.credential_id())
            || record.phase() == PairingPhase::Terminal
        {
            return Ok(());
        }
        let time = record.canonical_revocation_lower_bound_ms(transition)?;
        let event = record.transition(
            PairingEvent::credential_revoked(transition.clone()),
            PairingInitiator::from_credential(transition.initiator()),
            time,
        )?;
        self.append(&event, canonical)
    }
    pub(crate) fn id(&self) -> InvitationId {
        InvitationId::new(self.invitation)
    }
    pub(crate) fn restore(
        &self,
        canonical: &[CredentialTransitionDto],
    ) -> Result<PairingRecord, PairingError> {
        if self.history.len() > MAX_HISTORY_STEPS {
            return Err(PairingError::Capacity);
        }
        let invalid = |_| PairingError::Invalid;
        let intent = ConsentIntent::new(
            ConsentIntentId::new(self.consent),
            self.generation,
            AudienceId::new(self.audience.clone()).map_err(invalid)?,
            PrincipalId::new(self.owner.clone()).map_err(invalid)?,
            MembershipId::new(self.membership.clone()).map_err(invalid)?,
            Resource::new(
                OrganizationId::new(self.organization.clone()).map_err(invalid)?,
                ResourceId::new(self.resource.clone()).map_err(invalid)?,
            ),
        )?;
        let mut record = PairingRecord::new(
            self.id(),
            intent,
            self.created_at_ms,
            PairingPolicy::new(self.lifetime_ms, self.attempts)?,
        )?;
        for step in &self.history {
            let transition = record.transition(
                step.event.domain(canonical)?,
                step.actor.domain()?,
                step.ordering_time_ms,
            )?;
            record = transition.after().clone();
        }
        Ok(record)
    }
    pub(crate) fn append(
        &mut self,
        transition: &PairingTransition,
        canonical: &[CredentialTransitionDto],
    ) -> Result<(), PairingError> {
        transition.verify(&self.restore(canonical)?)?;
        self.history.push(StoredStep {
            event: StoredEvent::from_domain(transition.event(), canonical)?,
            actor: StoredActor::from_domain(transition.actor()),
            ordering_time_ms: transition.ordering_time_ms(),
        });
        Ok(())
    }
}
impl StoredActor {
    fn from_domain(actor: &PairingInitiator) -> Self {
        match actor {
            PairingInitiator::Principal(id) => Self::Principal {
                id: id.as_str().to_owned(),
            },
            PairingInitiator::Device(key) => Self::Device { key: *key.bytes() },
            PairingInitiator::System => Self::System,
            PairingInitiator::LocalOperator => Self::LocalOperator,
        }
    }
    fn domain(&self) -> Result<PairingInitiator, PairingError> {
        Ok(match self {
            Self::Principal { id } => PairingInitiator::Principal(
                PrincipalId::new(id.clone()).map_err(|_| PairingError::Invalid)?,
            ),
            Self::Device { key } => PairingInitiator::Device(DeviceKey::new(*key)),
            Self::System => PairingInitiator::System,
            Self::LocalOperator => PairingInitiator::LocalOperator,
        })
    }
}
impl StoredFailure {
    fn from_domain(cause: AttemptFailure) -> Self {
        match cause {
            AttemptFailure::InvalidProof => Self::InvalidProof,
            AttemptFailure::ConnectionClosed => Self::ConnectionClosed,
            AttemptFailure::HandshakeDeadline => Self::HandshakeDeadline,
            AttemptFailure::VerifierUnavailable => Self::VerifierUnavailable,
        }
    }
    fn domain(self) -> AttemptFailure {
        match self {
            Self::InvalidProof => AttemptFailure::InvalidProof,
            Self::ConnectionClosed => AttemptFailure::ConnectionClosed,
            Self::HandshakeDeadline => AttemptFailure::HandshakeDeadline,
            Self::VerifierUnavailable => AttemptFailure::VerifierUnavailable,
        }
    }
}
impl StoredTerminal {
    fn from_domain(cause: TerminalCause) -> Self {
        match cause {
            TerminalCause::CredentialRevoked => Self::CredentialRevoked,
            TerminalCause::Denied => Self::Denied,
            TerminalCause::Cancelled => Self::Cancelled,
            TerminalCause::Expired => Self::Expired,
            TerminalCause::Restarted => Self::Restarted,
        }
    }
    fn domain(self) -> TerminalCause {
        match self {
            Self::CredentialRevoked => TerminalCause::CredentialRevoked,
            Self::Denied => TerminalCause::Denied,
            Self::Cancelled => TerminalCause::Cancelled,
            Self::Expired => TerminalCause::Expired,
            Self::Restarted => TerminalCause::Restarted,
        }
    }
}
impl StoredEvent {
    fn from_domain(
        event: &PairingEvent,
        canonical: &[CredentialTransitionDto],
    ) -> Result<Self, PairingError> {
        Ok(match event.kind() {
            Event::CredentialRevoked(transition) => Self::CredentialRevoked {
                sequence: canonical
                    .iter()
                    .find(|entry| {
                        CredentialTransition::try_from((*entry).clone())
                            .is_ok_and(|original| &original == transition.as_ref())
                    })
                    .ok_or(PairingError::Conflict)?
                    .sequence,
            },
            Event::Reserve(id, key, input) => Self::Reserve {
                id: *id.bytes(),
                key: *key.bytes(),
                input: *input,
            },
            Event::Fail(id, key, cause) => Self::Fail {
                id: *id.bytes(),
                key: *key.bytes(),
                cause: StoredFailure::from_domain(*cause),
            },
            Event::Claim(id, key) => Self::Claim {
                id: *id.bytes(),
                key: *key.bytes(),
            },
            Event::Approve(key, generation) => Self::Approve {
                key: *key.bytes(),
                generation: *generation,
            },
            Event::Stage(credential, request, generation) => Self::Stage {
                credential: credential.as_str().to_owned(),
                request: *request.bytes(),
                generation: *generation,
            },
            Event::Receiver(credential, request, receiver, epoch, generation) => Self::Receiver {
                credential: credential.as_str().to_owned(),
                request: *request.bytes(),
                receiver: receiver.as_str().to_owned(),
                epoch: *epoch,
                generation: *generation,
            },
            Event::Activate(generation) => Self::Activate {
                generation: *generation,
            },
            Event::End(cause) => Self::End {
                cause: StoredTerminal::from_domain(*cause),
            },
            Event::NoReceiverCleanup(credential, request, generation) => Self::NoReceiverCleanup {
                credential: credential.as_str().to_owned(),
                request: *request.bytes(),
                generation: *generation,
            },
            Event::Cleanup(credential, request, receiver, epoch) => Self::Cleanup {
                credential: credential.as_str().to_owned(),
                request: *request.bytes(),
                receiver: receiver.as_str().to_owned(),
                epoch: *epoch,
            },
        })
    }
    fn domain(&self, canonical: &[CredentialTransitionDto]) -> Result<PairingEvent, PairingError> {
        let credential =
            |id: &String| CredentialId::new(id.clone()).map_err(|_| PairingError::Invalid);
        let receiver = |id: &String| ResourceId::new(id.clone()).map_err(|_| PairingError::Invalid);
        Ok(match self {
            Self::CredentialRevoked { sequence } => PairingEvent::credential_revoked(
                CredentialTransition::try_from(
                    canonical
                        .iter()
                        .find(|entry| entry.sequence == *sequence)
                        .ok_or(PairingError::Conflict)?
                        .clone(),
                )
                .map_err(|_| PairingError::Invalid)?,
            ),
            Self::Reserve { id, key, input } => {
                PairingEvent::reserve(AttemptId::new(*id), DeviceKey::new(*key), *input)
            }
            Self::Fail { id, key, cause } => {
                PairingEvent::fail(AttemptId::new(*id), DeviceKey::new(*key), cause.domain())
            }
            Self::Claim { id, key } => {
                PairingEvent::claim(AttemptId::new(*id), DeviceKey::new(*key))
            }
            Self::Approve { key, generation } => {
                PairingEvent::approve(DeviceKey::new(*key), *generation)
            }
            Self::Stage {
                credential: id,
                request,
                generation,
            } => PairingEvent::stage(credential(id)?, AttemptId::new(*request), *generation),
            Self::Receiver {
                credential: id,
                request,
                receiver: rid,
                epoch,
                generation,
            } => PairingEvent::receiver(
                credential(id)?,
                AttemptId::new(*request),
                receiver(rid)?,
                *epoch,
                *generation,
            ),
            Self::Activate { generation } => PairingEvent::activate(*generation),
            Self::End { cause } => PairingEvent::end(cause.domain()),
            Self::NoReceiverCleanup {
                credential: id,
                request,
                generation,
            } => PairingEvent::no_receiver_cleanup(
                credential(id)?,
                AttemptId::new(*request),
                *generation,
            ),
            Self::Cleanup {
                credential: id,
                request,
                receiver: rid,
                epoch,
            } => PairingEvent::cleanup(
                credential(id)?,
                AttemptId::new(*request),
                receiver(rid)?,
                *epoch,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pairing::{PairingPhase, PairingPolicy};

    fn record() -> PairingRecord {
        PairingRecord::new(
            InvitationId::new([1; 16]),
            ConsentIntent::new(
                ConsentIntentId::new([2; 16]),
                1,
                AudienceId::new("gateway").unwrap(),
                PrincipalId::new("owner").unwrap(),
                MembershipId::new("member").unwrap(),
                Resource::new(
                    OrganizationId::new("org").unwrap(),
                    ResourceId::new("conversation").unwrap(),
                ),
            )
            .unwrap(),
            1000,
            PairingPolicy::initial(),
        )
        .unwrap()
    }

    #[test]
    fn serialized_history_replays_exact_claim_and_consent() {
        let initial = record();
        let key = DeviceKey::new([3; 32]);
        let attempt = AttemptId::new([4; 16]);
        let mut stored = StoredPairing::new(&initial).unwrap();
        let reserve = initial
            .transition(
                PairingEvent::reserve(attempt, key, [5; 32]),
                PairingInitiator::Device(key),
                1001,
            )
            .unwrap();
        stored.append(&reserve, &[]).unwrap();
        let claim = reserve
            .after()
            .transition(
                PairingEvent::claim(attempt, key),
                PairingInitiator::Device(key),
                1002,
            )
            .unwrap();
        stored.append(&claim, &[]).unwrap();
        let approve = claim
            .after()
            .transition(
                PairingEvent::approve(key, 1),
                PairingInitiator::Principal(PrincipalId::new("owner").unwrap()),
                1003,
            )
            .unwrap();
        stored.append(&approve, &[]).unwrap();
        let bytes = serde_json::to_vec(&stored).unwrap();
        let reopened: StoredPairing = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&reopened.restore(&[]).unwrap(), approve.after());
        assert_eq!(
            reopened.restore(&[]).unwrap().phase(),
            PairingPhase::Approved
        );
        // Consent must not become a valid event when its actor is replaced.
        let mut changed = reopened.clone();
        changed.history[2].actor = StoredActor::Principal {
            id: "other-owner".to_owned(),
        };
        assert_eq!(changed.restore(&[]).unwrap_err(), PairingError::WrongActor);
        // A stale generation must fail the same transition owner on reopen.
        let mut changed = reopened;
        changed.generation = 2;
        assert_eq!(
            changed.restore(&[]).unwrap_err(),
            PairingError::StaleGeneration
        );
    }

    #[test]
    fn history_cannot_append_to_another_projection_or_overflow() {
        let initial = record();
        let key = DeviceKey::new([3; 32]);
        let transition = initial
            .transition(
                PairingEvent::reserve(AttemptId::new([4; 16]), key, [5; 32]),
                PairingInitiator::Device(key),
                1001,
            )
            .unwrap();
        let mut stored = StoredPairing::new(&initial).unwrap();
        stored.append(&transition, &[]).unwrap();
        assert_eq!(
            stored.append(&transition, &[]).unwrap_err(),
            PairingError::Conflict
        );
        while stored.history.len() <= MAX_HISTORY_STEPS {
            stored.history.push(stored.history[0].clone());
        }
        assert_eq!(stored.restore(&[]).unwrap_err(), PairingError::Capacity);
        assert_eq!(
            stored.append(&transition, &[]).unwrap_err(),
            PairingError::Capacity
        );
    }
}
