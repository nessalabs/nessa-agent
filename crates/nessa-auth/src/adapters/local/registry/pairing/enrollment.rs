use super::super::{
    domain_credential, record_transition, DeviceProofBinding, LocalCredentialStore, Registry,
    StoredCredential, StoredProof,
};
use super::projection::StoredPairing;
use crate::{
    application::{
        dto::{CredentialGrantDto, CredentialMetadataDto, MembershipStateDto, ResourceDto},
        pairing::{
            AttemptReservation, ConfirmedClaim, DeviceConnectionProof, GatewayKeyStore,
            NoReceiverCleanupProof, OwnerDecision, PairingAdmission, PairingStore,
            PairingStoreError, PrivateKeyMaterial, ReceiverOutcome, RuntimeEnd, StageGate,
            StageOwnership,
        },
        ports::{Clock, PortFuture},
    },
    domain::{
        pairing::{
            validate_pairing_collection, AttemptFailure, AttemptId, AttemptOutcome, InvitationId,
            PairingError, PairingEvent, PairingInitiator, PairingPhase, PairingRecord,
            PairingTransition, TerminalCause,
        },
        AudienceId, CredentialId, Initiator, IssuanceCause,
    },
};
use std::sync::Arc;
impl PairingStore for LocalCredentialStore {
    fn publish_first_gateway_key(
        &self,
        keys: &dyn GatewayKeyStore,
        key: &PrivateKeyMaterial,
        clock: &dyn Clock,
    ) -> Result<(), PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        if !current.pairings.is_empty() {
            return Err(PairingStoreError::GatewayKeyHistoryExists);
        }
        let gateway = AudienceId::new(current.gateway_id.clone())
            .map_err(|_| PairingStoreError::Unavailable)?;
        keys.save_gateway_key(key, &gateway, clock)
            .map_err(PairingStoreError::PrivateState)
    }

    fn acquire_stage(
        self: Arc<Self>,
        id: InvitationId,
    ) -> Result<StageOwnership, PairingStoreError> {
        let record = self.read_pairing(id)?;
        if !(record.phase() == PairingPhase::Staging
            || (record.phase() == PairingPhase::Terminal && record.cleanup_pending()))
        {
            return Err(PairingStoreError::Domain(PairingError::Conflict));
        }
        let gate = {
            let mut gates = self
                .stage_gates
                .lock()
                .map_err(|_| PairingStoreError::Unavailable)?;
            gates
                .entry(id)
                .or_insert_with(|| Arc::new(StageGate::new()))
                .clone()
        };
        let permit = gate.acquire()?;
        Ok(StageOwnership::new(record, self, permit))
    }
    fn finish_no_receiver(
        &self,
        proof: &NoReceiverCleanupProof,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        let admitted = proof.record();
        {
            let gates = self
                .stage_gates
                .lock()
                .map_err(|_| PairingStoreError::Unavailable)?;
            if !gates
                .get(&admitted.id())
                .is_some_and(|gate| proof.belongs_to(gate))
            {
                return Err(PairingStoreError::Domain(PairingError::Conflict));
            }
        }
        let (credential, request) = admitted
            .stage_binding()
            .ok_or(PairingStoreError::Domain(PairingError::Conflict))?;
        self.commit_pairing(admitted.id(), clock, |record, now| {
            if record.receiver_binding().is_some() {
                return Err(PairingError::Conflict);
            }
            if !record.cleanup_pending() {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::no_receiver_cleanup(
                        credential.clone(),
                        request,
                        admitted.intent().generation(),
                    ),
                    PairingInitiator::System,
                    now,
                )
                .map(Some)
        })
        .map(PairingCommit::record)
    }

    fn pending_pairings(&self) -> Result<Box<[PairingRecord]>, PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        let mut pending = Vec::new();
        for stored in &current.pairings {
            let record = stored
                .restore(&current.transitions)
                .map_err(PairingStoreError::Domain)?;
            if !matches!(
                record.phase(),
                PairingPhase::Active | PairingPhase::Terminal
            ) || record.cleanup_pending()
            {
                pending.push(record);
            }
        }
        Ok(pending.into_boxed_slice())
    }
    fn expire_pairing_if_due(
        &self,
        id: InvitationId,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(id, clock, |record, now| record.expire_if_due(now))
            .map(PairingCommit::record)
    }
    fn end_pairing(
        &self,
        id: InvitationId,
        cause: RuntimeEnd,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(id, clock, |record, now| {
            if record.phase() == PairingPhase::Terminal {
                return Ok(None);
            }
            let cause = match cause {
                RuntimeEnd::Expired => TerminalCause::Expired,
                RuntimeEnd::Restarted => TerminalCause::Restarted,
            };
            record
                .transition(PairingEvent::end(cause), PairingInitiator::System, now)
                .map(Some)
        })
        .map(PairingCommit::record)
    }

    fn read_pairing(&self, id: InvitationId) -> Result<PairingRecord, PairingStoreError> {
        let slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        current
            .pairings
            .iter()
            .find(|entry| entry.id() == id)
            .ok_or(PairingStoreError::NotFound)?
            .restore(&current.transitions)
            .map_err(PairingStoreError::Domain)
    }

    fn create_pairing<'a>(
        &'a self,
        record: &'a PairingRecord,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError> {
        Box::pin(async move {
            let mut slot = self
                .registry()
                .map_err(|_| PairingStoreError::Unavailable)?;
            let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
            let now = clock.unix_milliseconds();
            validate_pairing_admission(current, admission, now)?;
            if record.intent() != admission.intent()
                || record.created_at_ms() > now
                || now >= record.expires_at_ms()
            {
                return Err(PairingStoreError::Domain(PairingError::Invalid));
            }
            self.config
                .validate_pairing_count(
                    current
                        .pairings
                        .len()
                        .checked_add(1)
                        .ok_or(PairingStoreError::Domain(PairingError::Capacity))?,
                )
                .map_err(PairingStoreError::Domain)?;
            let mut records = current
                .pairings
                .iter()
                .map(|entry| entry.restore(&current.transitions))
                .collect::<Result<Vec<_>, _>>()
                .map_err(PairingStoreError::Domain)?;
            records.push(record.clone());
            validate_pairing_collection(&records).map_err(PairingStoreError::Domain)?;
            let mut next = current.clone();
            next.pairings
                .push(StoredPairing::new(record).map_err(PairingStoreError::Domain)?);
            next.revision = next
                .revision
                .checked_add(1)
                .ok_or(PairingStoreError::Domain(PairingError::Capacity))?;
            self.persist(&next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            let revision = next.revision;
            *slot = Some(next.clone());
            self.publish_snapshot(next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            self.publish_revision(revision);
            Ok(record.clone())
        })
    }
    fn reserve_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        input: [u8; 32],
        clock: &dyn Clock,
    ) -> Result<AttemptReservation, PairingStoreError> {
        let result = self.commit_pairing(id, clock, |record, now| {
            if record
                .reservation_status(attempt, device.key(), input)?
                .is_some()
            {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::reserve(attempt, device.key(), input),
                    PairingInitiator::Device(device.key()),
                    now,
                )
                .map(Some)
        })?;
        Ok(match result {
            PairingCommit::Applied(record) => AttemptReservation::Admitted(record),
            PairingCommit::Existing(record) => AttemptReservation::Existing(record),
        })
    }
    fn confirm_claim(
        &self,
        proof: &ConfirmedClaim,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(proof.invitation(), clock, |record, now| {
            if record.intent().id() != proof.consent()
                || record.intent().generation() != proof.generation()
                || record.expires_at_ms() != proof.expiry_ms()
            {
                return Err(PairingError::Conflict);
            }
            let outcome = record
                .reservation_status(proof.attempt(), proof.key(), proof.input())?
                .ok_or(PairingError::Conflict)?;
            if outcome == AttemptOutcome::Claimed {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::claim(proof.attempt(), proof.key()),
                    PairingInitiator::Device(proof.key()),
                    now,
                )
                .map(Some)
        })
        .map(PairingCommit::record)
    }
    fn fail_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        cause: AttemptFailure,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(id, clock, |record, now| {
            if record.attempt_status(attempt, device.key())? == AttemptOutcome::Failed(cause) {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::fail(attempt, device.key(), cause),
                    PairingInitiator::Device(device.key()),
                    now,
                )
                .map(Some)
        })
        .map(PairingCommit::record)
    }
    fn stage_pairing(
        &self,
        id: InvitationId,
        credential: CredentialId,
        request: AttemptId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_authorized_pairing(
            id,
            admission,
            clock,
            |registry, record, now| {
                if record.stage_binding().is_some_and(|(prior, correlation)| {
                    prior == &credential && correlation == request
                }) && matches!(record.phase(), PairingPhase::Staging | PairingPhase::Active)
                {
                    return Ok(None);
                }
                if !super::super::credential_identity_available(
                    registry,
                    credential.as_str(),
                    Some(id),
                )? {
                    return Err(PairingError::Conflict);
                }
                record
                    .transition(
                        PairingEvent::stage(credential, request, admission.intent().generation()),
                        PairingInitiator::Principal(admission.intent().owner().clone()),
                        now,
                    )
                    .map(Some)
            },
            |_, _| Ok(()),
        )
    }
    fn remember_receiver(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(id, clock, |record, now| {
            if record.intent().generation() != outcome.generation() {
                return Err(PairingError::StaleGeneration);
            }
            if record.stage_binding().is_some_and(|(credential, request)| {
                credential == outcome.credential() && request == outcome.request()
            }) && record.receiver_binding().is_some_and(|(receiver, epoch)| {
                receiver == outcome.receiver() && epoch == outcome.epoch()
            }) {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::receiver(
                        outcome.credential().clone(),
                        outcome.request(),
                        outcome.receiver().clone(),
                        outcome.epoch(),
                        outcome.generation(),
                    ),
                    PairingInitiator::System,
                    now,
                )
                .map(Some)
        })
        .map(PairingCommit::record)
    }
    fn publish_pairing(
        &self,
        id: InvitationId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_authorized_pairing(
            id,
            admission,
            clock,
            |_, record, now| {
                if record.phase() == PairingPhase::Active {
                    return Ok(None);
                }
                record
                    .transition(
                        PairingEvent::activate(admission.intent().generation()),
                        PairingInitiator::Principal(admission.intent().owner().clone()),
                        now,
                    )
                    .map(Some)
            },
            |next, transition| {
                if next.credentials.len() >= self.config.max_credentials {
                    return Err(PairingStoreError::Domain(PairingError::Capacity));
                }
                let record = transition.after();
                let intent = record.intent();
                let credential = record
                    .credential()
                    .ok_or(PairingStoreError::Domain(PairingError::Invalid))?;
                if next
                    .credentials
                    .iter()
                    .any(|entry| entry.metadata.id == credential.as_str())
                {
                    return Err(PairingStoreError::Domain(PairingError::Conflict));
                }
                let metadata = CredentialMetadataDto {
                    id: credential.as_str().to_owned(),
                    principal_id: intent.owner().as_str().to_owned(),
                    organization_id: intent.resource().organization_id().as_str().to_owned(),
                    audience_id: intent.audience().as_str().to_owned(),
                    issued_at: transition.ordering_time_ms() / 1000,
                    expires_at: None,
                    revoked_at: None,
                    grants: vec![CredentialGrantDto {
                        action: intent.grant().action().as_str().to_owned(),
                        resource: ResourceDto {
                            organization_id: intent
                                .resource()
                                .organization_id()
                                .as_str()
                                .to_owned(),
                            id: intent.resource().id().as_str().to_owned(),
                        },
                    }],
                };
                let credential =
                    domain_credential(&metadata).map_err(|_| PairingStoreError::Unavailable)?;
                let issuance = credential
                    .issued(
                        IssuanceCause::DevicePairing,
                        Initiator::Principal(intent.owner().clone()),
                    )
                    .map_err(|_| PairingStoreError::Unavailable)?;
                next.credentials.push(StoredCredential {
                    metadata,
                    verifier: StoredProof::Device(DeviceProofBinding::DevicePairing {
                        invitation: *id.bytes(),
                        generation: intent.generation(),
                    }),
                });
                record_transition(next, &issuance, None)
                    .map_err(|_| PairingStoreError::Unavailable)?;
                Ok(())
            },
        )
    }
    fn finish_cleanup(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError> {
        self.commit_pairing(id, clock, |record, now| {
            if record.intent().generation() != outcome.generation() {
                return Err(PairingError::StaleGeneration);
            }
            if record.phase() == PairingPhase::Terminal
                && !record.cleanup_pending()
                && record.stage_binding().is_some_and(|(credential, request)| {
                    credential == outcome.credential() && request == outcome.request()
                })
                && record.receiver_binding().is_some_and(|(receiver, epoch)| {
                    receiver == outcome.receiver() && epoch == outcome.epoch()
                })
            {
                return Ok(None);
            }
            record
                .transition(
                    PairingEvent::cleanup(
                        outcome.credential().clone(),
                        outcome.request(),
                        outcome.receiver().clone(),
                        outcome.epoch(),
                    ),
                    PairingInitiator::System,
                    now,
                )
                .map(Some)
        })
        .map(PairingCommit::record)
    }
    fn decide_pairing<'a>(
        &'a self,
        id: InvitationId,
        decision: OwnerDecision,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError> {
        Box::pin(async move {
            let mut slot = self
                .registry()
                .map_err(|_| PairingStoreError::Unavailable)?;
            let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
            let now = clock.unix_milliseconds();
            validate_pairing_admission(current, admission, now)?;
            let index = current
                .pairings
                .iter()
                .position(|entry| entry.id() == id)
                .ok_or(PairingStoreError::NotFound)?;
            let record = current.pairings[index]
                .restore(&current.transitions)
                .map_err(PairingStoreError::Domain)?;
            if record.intent() != admission.intent() {
                return Err(PairingStoreError::Domain(PairingError::Conflict));
            }
            let event = match decision {
                OwnerDecision::Approve(key) => {
                    if matches!(
                        record.phase(),
                        PairingPhase::Approved | PairingPhase::Staging | PairingPhase::Active
                    ) && record
                        .claim_binding()
                        .is_some_and(|(_, claimed)| key == claimed)
                    {
                        return Ok(record);
                    }
                    PairingEvent::approve(key, admission.intent().generation())
                }
                OwnerDecision::Deny => {
                    if record.phase() == PairingPhase::Terminal {
                        return Ok(record);
                    }
                    PairingEvent::end(TerminalCause::Denied)
                }
                OwnerDecision::Cancel => {
                    if record.phase() == PairingPhase::Terminal {
                        return Ok(record);
                    }
                    PairingEvent::end(TerminalCause::Cancelled)
                }
            };
            let transition = record
                .transition(
                    event,
                    PairingInitiator::Principal(admission.intent().owner().clone()),
                    now,
                )
                .map_err(PairingStoreError::Domain)?;
            let mut next = current.clone();
            next.pairings[index]
                .append(&transition, &next.transitions)
                .map_err(PairingStoreError::Domain)?;
            next.revision = next
                .revision
                .checked_add(1)
                .ok_or(PairingStoreError::Domain(PairingError::Capacity))?;
            if record.phase() == PairingPhase::Active && decision == OwnerDecision::Cancel {
                let credential_id = record
                    .credential()
                    .ok_or(PairingStoreError::Domain(PairingError::Invalid))?;
                let entry = next
                    .credentials
                    .iter_mut()
                    .find(|entry| entry.metadata.id == credential_id.as_str())
                    .ok_or(PairingStoreError::Unavailable)?;
                let mut credential = domain_credential(&entry.metadata)
                    .map_err(|_| PairingStoreError::Unavailable)?;
                if let Some(revocation) = credential
                    .revoke(
                        now / 1000,
                        Initiator::Principal(admission.intent().owner().clone()),
                    )
                    .map_err(|_| PairingStoreError::Unavailable)?
                {
                    entry.metadata.revoked_at = credential.revoked_at();
                    record_transition(&mut next, &revocation, None)
                        .map_err(|_| PairingStoreError::Unavailable)?;
                }
            }
            self.persist(&next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            let revision = next.revision;
            *slot = Some(next.clone());
            self.publish_snapshot(next)
                .map_err(|_| PairingStoreError::Unavailable)?;
            self.publish_revision(revision);
            Ok(transition.after().clone())
        })
    }
}

enum PairingCommit {
    Applied(PairingRecord),
    Existing(PairingRecord),
}
impl PairingCommit {
    fn record(self) -> PairingRecord {
        match self {
            Self::Applied(record) | Self::Existing(record) => record,
        }
    }
}
impl LocalCredentialStore {
    fn commit_authorized_pairing(
        &self,
        id: InvitationId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
        transition: impl FnOnce(
            &Registry,
            &PairingRecord,
            u64,
        ) -> Result<Option<PairingTransition>, PairingError>,
        effect: impl FnOnce(&mut Registry, &PairingTransition) -> Result<(), PairingStoreError>,
    ) -> Result<PairingRecord, PairingStoreError> {
        let mut slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        let now = clock.unix_milliseconds();
        validate_pairing_admission(current, admission, now)?;
        let index = current
            .pairings
            .iter()
            .position(|entry| entry.id() == id)
            .ok_or(PairingStoreError::NotFound)?;
        let record = current.pairings[index]
            .restore(&current.transitions)
            .map_err(PairingStoreError::Domain)?;
        if record.intent() != admission.intent() {
            return Err(PairingStoreError::Domain(PairingError::Conflict));
        }
        let Some(transition) =
            transition(current, &record, now).map_err(PairingStoreError::Domain)?
        else {
            return Ok(record);
        };
        let mut next = current.clone();
        next.pairings[index]
            .append(&transition, &next.transitions)
            .map_err(PairingStoreError::Domain)?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(PairingStoreError::Domain(PairingError::Capacity))?;
        effect(&mut next, &transition)?;
        self.persist(&next)
            .map_err(|_| PairingStoreError::Unavailable)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)
            .map_err(|_| PairingStoreError::Unavailable)?;
        self.publish_revision(revision);
        Ok(transition.after().clone())
    }
    fn commit_pairing(
        &self,
        id: InvitationId,
        clock: &dyn Clock,
        operation: impl FnOnce(&PairingRecord, u64) -> Result<Option<PairingTransition>, PairingError>,
    ) -> Result<PairingCommit, PairingStoreError> {
        let mut slot = self
            .registry()
            .map_err(|_| PairingStoreError::Unavailable)?;
        let current = slot.as_ref().ok_or(PairingStoreError::Unavailable)?;
        let index = current
            .pairings
            .iter()
            .position(|entry| entry.id() == id)
            .ok_or(PairingStoreError::NotFound)?;
        let record = current.pairings[index]
            .restore(&current.transitions)
            .map_err(PairingStoreError::Domain)?;
        let Some(transition) =
            operation(&record, clock.unix_milliseconds()).map_err(PairingStoreError::Domain)?
        else {
            return Ok(PairingCommit::Existing(record));
        };
        let mut next = current.clone();
        next.pairings[index]
            .append(&transition, &next.transitions)
            .map_err(PairingStoreError::Domain)?;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(PairingStoreError::Domain(PairingError::Capacity))?;
        self.persist(&next)
            .map_err(|_| PairingStoreError::Unavailable)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)
            .map_err(|_| PairingStoreError::Unavailable)?;
        self.publish_revision(revision);
        Ok(PairingCommit::Applied(transition.after().clone()))
    }
}

fn validate_pairing_admission(
    registry: &Registry,
    admission: &PairingAdmission,
    now_ms: u64,
) -> Result<(), PairingStoreError> {
    if registry.revision != admission.revision() {
        return Err(PairingStoreError::StaleRevision);
    }
    if admission
        .deadline_seconds()
        .is_some_and(|deadline| now_ms / 1000 >= deadline)
    {
        return Err(PairingStoreError::Domain(PairingError::Expired));
    }
    let intent = admission.intent();
    let issuer = registry
        .credentials
        .iter()
        .find(|entry| entry.metadata.id == admission.credential().as_str())
        .ok_or(PairingStoreError::Unavailable)?;
    let credential =
        domain_credential(&issuer.metadata).map_err(|_| PairingStoreError::Unavailable)?;
    let bound = registry.memberships.iter().any(|membership| {
        membership.id == intent.membership().as_str()
            && membership.principal_id == intent.owner().as_str()
            && membership.organization_id == intent.resource().organization_id().as_str()
            && membership.state == MembershipStateDto::Active
    });
    if !credential.is_valid_at(now_ms / 1000)
        || !bound
        || credential.principal_id() != intent.owner()
        || credential.organization_id() != intent.resource().organization_id()
        || credential.audience_id() != intent.audience()
    {
        return Err(PairingStoreError::Domain(PairingError::Invalid));
    }
    Ok(())
}
