//! Pairing history lives inside the existing registry, with one replay owner.
use super::owner_phone_original::StoredOwnerPhoneOriginal;
use crate::application::dto::{
    CredentialMetadataDto, CredentialTransitionDto, InitiatorDto, IssuanceCauseDto,
    TransitionCauseDto,
};
use crate::application::pairing::{OwnerPhoneOriginalReceipt, VerifiedOwnerPhoneDeviceProof};
use crate::domain::{
    pairing::{
        AttemptFailure, AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DeviceKey, Event,
        InvitationId, OwnerPhoneRequest, OwnerPhoneRequestId, PairingError, PairingEvent,
        PairingInitiator, PairingPhase, PairingPolicy, PairingRecord, PairingTransition,
        TerminalCause,
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
    /// Who the invitation enrolls. Absent means a device: every enrollment
    /// before peer gateways was one, and a device enrollment is still written
    /// without it.
    #[serde(default, skip_serializing_if = "StoredClass::is_device")]
    class: StoredClass,
    history: Vec<StoredStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_phone_original: Option<StoredOwnerPhoneOriginal>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredClass {
    #[default]
    Device,
    Peer,
    Environment,
    OwnerPhone,
}
impl StoredClass {
    fn is_device(&self) -> bool {
        *self == Self::Device
    }
    fn from_domain(class: ConsentClass) -> Self {
        match class {
            ConsentClass::DeviceRead => Self::Device,
            ConsentClass::PeerRead => Self::Peer,
            ConsentClass::PeerEnvironment => Self::Environment,
            ConsentClass::OwnerPhoneReadWrite => Self::OwnerPhone,
        }
    }
    fn domain(self) -> ConsentClass {
        match self {
            Self::Device => ConsentClass::DeviceRead,
            Self::Peer => ConsentClass::PeerRead,
            Self::Environment => ConsentClass::PeerEnvironment,
            Self::OwnerPhone => ConsentClass::OwnerPhoneReadWrite,
        }
    }
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
    OwnerPhoneApproved {
        request: [u8; 16],
        key: [u8; 32],
        digest: [u8; 32],
    },
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
            class: StoredClass::from_domain(intent.class()),
            history: Vec::new(),
            owner_phone_original: None,
        })
    }
    pub(super) fn new_owner_phone(
        record: &PairingRecord,
        proof: &VerifiedOwnerPhoneDeviceProof,
    ) -> Result<Self, PairingError> {
        if record.intent().class() != ConsentClass::OwnerPhoneReadWrite {
            return Err(PairingError::Invalid);
        }
        let mut stored = Self::new(record)?;
        stored.owner_phone_original = Some(StoredOwnerPhoneOriginal::from_proof(proof));
        Ok(stored)
    }
    pub(crate) fn owner_phone_request(&self) -> Result<Option<OwnerPhoneRequest>, PairingError> {
        self.owner_phone_original
            .as_ref()
            .map(StoredOwnerPhoneOriginal::restore)
            .transpose()
    }
    pub(super) fn owner_phone_receipt(
        &self,
        canonical: &[CredentialTransitionDto],
    ) -> Result<OwnerPhoneOriginalReceipt, PairingError> {
        // restore owns source/approval/key/digest agreement with the stored original.
        let record = self.restore(canonical)?;
        let original = self.owner_phone_request()?.ok_or(PairingError::Conflict)?;
        let (_, _, digest) = record
            .owner_phone_approval()
            .ok_or(PairingError::Conflict)?;
        Ok(OwnerPhoneOriginalReceipt::restored(
            original, digest, record,
        ))
    }
    /// Who this enrollment enrolls, as stored.
    pub(crate) fn class(&self) -> ConsentClass {
        self.class.domain()
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
                && record
                    .credential_principal()
                    .is_some_and(|principal| principal.as_str() == metadata.principal_id)
                && credential.organization_id() == intent.resource().organization_id()
                && metadata.audience_id == intent.audience().as_str()
                && metadata.expires_at.is_none()
                && metadata.grants.len() == intent.grants().count()
                && metadata.grants.iter().zip(intent.grants()).all(|(stored, grant)| {
                    stored.action == grant.action().as_str()
                        && stored.resource.organization_id == grant.resource().organization_id().as_str()
                        && stored.resource.id == grant.resource().id().as_str()
                })
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
            self.class.domain(),
        )?;
        // A stored original request is what makes an enrollment a signed one.
        let constructor = if self.owner_phone_original.is_none() {
            PairingRecord::new
        } else {
            PairingRecord::new_owner_phone
        };
        let mut record = constructor(
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
            let revocation = record.credential().and_then(|id| {
                canonical.iter().find(|entry| {
                    entry.credential_id == id.as_str()
                        && matches!(entry.cause, TransitionCauseDto::Revoked { .. })
                })
            });
            let revocation = revocation
                .map(|entry| CredentialTransition::try_from(entry.clone()))
                .transpose()
                .map_err(invalid)?;
            transition.verify_cancellation_revocation(revocation.as_ref())?;
            record = transition.after().clone();
        }
        match (&self.owner_phone_original, self.class.domain()) {
            (Some(original), ConsentClass::OwnerPhoneReadWrite) => {
                let request = original.restore()?;
                let source = request.source();
                let intent = record.intent();
                if source.owner() != intent.owner()
                    || source.membership() != intent.membership()
                    || source.audience() != intent.audience()
                    || source.resource() != intent.resource()
                {
                    return Err(PairingError::Conflict);
                }
                if !self.history.is_empty()
                    && record.owner_phone_approval()
                        != Some((request.id(), request.device_key(), original.digest()))
                {
                    return Err(PairingError::Conflict);
                }
            }
            (None, _) => {}
            _ => return Err(PairingError::Conflict),
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
            Event::OwnerPhoneApproved(request, key, digest) => Self::OwnerPhoneApproved {
                request: *request.bytes(),
                key: *key.bytes(),
                digest: *digest,
            },
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
            Self::OwnerPhoneApproved {
                request,
                key,
                digest,
            } => PairingEvent::owner_phone_approved(
                OwnerPhoneRequestId::new(*request)?,
                DeviceKey::new(*key),
                *digest,
            ),
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
