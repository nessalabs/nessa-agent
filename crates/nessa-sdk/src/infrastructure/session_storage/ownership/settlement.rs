//! Required typed close proof codec; no historical proof defaults or repair.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum ProofDto {
    Resource {
        observations: [Option<ObservationDto>; 3],
    },
    Absence {
        proof: AbsenceDto,
        record: EvidenceDto,
        acknowledgement: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObservationDto {
    record: EvidenceDto,
    acknowledgement: String,
    provider_acknowledged: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum AbsenceDto {
    NeverTransferredRoot,
    PreparationRejectedWithoutOwner { request: String },
    AdmissionFailedBeforeFactory { request: String },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum DetailDto {
    ResourceObservation {
        physical: String,
        provider_evidence: String,
    },
    Absence {
        proof: AbsenceDto,
    },
    Completion,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceDto {
    parent: String,
    target: String,
    operation: String,
    cause: String,
    initiator: InitiatorDto,
    detail: DetailDto,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletionDto {
    root: String,
    record: EvidenceDto,
    acknowledgement: String,
}
fn evidence_name(value: EvidenceFact) -> String {
    match value {
        EvidenceFact::Pending => "pending",
        EvidenceFact::Failed => "failed",
        EvidenceFact::Acknowledged => "acknowledged",
    }
    .into()
}
fn parse_evidence(value: &str) -> Result<EvidenceFact, ()> {
    match value {
        "pending" => Ok(EvidenceFact::Pending),
        "failed" => Ok(EvidenceFact::Failed),
        "acknowledged" => Ok(EvidenceFact::Acknowledged),
        _ => Err(()),
    }
}
impl From<&AbsenceProof> for AbsenceDto {
    fn from(p: &AbsenceProof) -> Self {
        match p {
            AbsenceProof::NeverTransferredRoot => Self::NeverTransferredRoot,
            AbsenceProof::PreparationRejectedWithoutOwner(r) => {
                Self::PreparationRejectedWithoutOwner {
                    request: r.as_str().into(),
                }
            }
            AbsenceProof::AdmissionFailedBeforeFactory(r) => Self::AdmissionFailedBeforeFactory {
                request: r.as_str().into(),
            },
        }
    }
}
impl TryFrom<AbsenceDto> for AbsenceProof {
    type Error = ();
    fn try_from(p: AbsenceDto) -> Result<Self, ()> {
        Ok(match p {
            AbsenceDto::NeverTransferredRoot => Self::NeverTransferredRoot,
            AbsenceDto::PreparationRejectedWithoutOwner { request } => {
                Self::PreparationRejectedWithoutOwner(SpawnRequestId::new(request).map_err(|_| ())?)
            }
            AbsenceDto::AdmissionFailedBeforeFactory { request } => {
                Self::AdmissionFailedBeforeFactory(SpawnRequestId::new(request).map_err(|_| ())?)
            }
        })
    }
}
impl From<&OwnershipEvidence> for EvidenceDto {
    fn from(record: &OwnershipEvidence) -> Self {
        Self {
            parent: record.parent_lifetime.as_str().into(),
            target: record
                .child_lifetime
                .as_ref()
                .expect("validated close target")
                .as_str()
                .into(),
            operation: record
                .close_operation
                .as_ref()
                .expect("validated close operation")
                .as_str()
                .into(),
            cause: cause_name(record.cause.as_ref().expect("validated close cause")).into(),
            initiator: InitiatorDto::from(&record.initiator),
            detail: match record.close_detail.as_ref().expect("typed close evidence") {
                CloseEvidenceDetail::ResourceObservation {
                    physical,
                    provider_evidence,
                } => DetailDto::ResourceObservation {
                    physical: match physical {
                        PhysicalFact::Pending => "pending",
                        PhysicalFact::Failed => "failed",
                        PhysicalFact::Released => "released",
                    }
                    .into(),
                    provider_evidence: evidence_name(*provider_evidence),
                },
                CloseEvidenceDetail::Absence(proof) => DetailDto::Absence {
                    proof: AbsenceDto::from(proof),
                },
                CloseEvidenceDetail::Completion => DetailDto::Completion,
            },
        }
    }
}
impl TryFrom<EvidenceDto> for OwnershipEvidence {
    type Error = ();
    fn try_from(dto: EvidenceDto) -> Result<Self, ()> {
        let detail = match dto.detail {
            DetailDto::ResourceObservation {
                physical,
                provider_evidence,
            } => CloseEvidenceDetail::ResourceObservation {
                physical: match physical.as_str() {
                    "pending" => PhysicalFact::Pending,
                    "failed" => PhysicalFact::Failed,
                    "released" => PhysicalFact::Released,
                    _ => return Err(()),
                },
                provider_evidence: parse_evidence(&provider_evidence)?,
            },
            DetailDto::Absence { proof } => CloseEvidenceDetail::Absence(proof.try_into()?),
            DetailDto::Completion => CloseEvidenceDetail::Completion,
        };
        Ok(Self {
            parent_lifetime: AgentLifetimeId::new(dto.parent).map_err(|_| ())?,
            child_lifetime: Some(AgentLifetimeId::new(dto.target).map_err(|_| ())?),
            close_operation: Some(CloseOperationId::new(dto.operation).map_err(|_| ())?),
            cause: Some(parse_cause(&dto.cause)?),
            initiator: dto.initiator.try_into()?,
            before: OwnershipMeaning::Closing,
            after: if detail == CloseEvidenceDetail::Completion {
                OwnershipMeaning::Closed
            } else {
                OwnershipMeaning::Closing
            },
            close_detail: Some(detail),
        })
    }
}
impl From<&SettlementProof> for ProofDto {
    fn from(proof: &SettlementProof) -> Self {
        match proof {
            SettlementProof::Absence(a) => Self::Absence {
                proof: AbsenceDto::from(a.proof()),
                record: EvidenceDto::from(a.record()),
                acknowledgement: evidence_name(a.acknowledgement()),
            },
            SettlementProof::Resource(slots) => Self::Resource {
                observations: slots.clone().map(|a| {
                    a.map(|a| ObservationDto {
                        record: EvidenceDto::from(a.record()),
                        acknowledgement: evidence_name(a.acknowledgement()),
                        provider_acknowledged: a.provider_acknowledged(),
                    })
                }),
            },
        }
    }
}
impl TryFrom<ProofDto> for SettlementProof {
    type Error = ();
    fn try_from(dto: ProofDto) -> Result<Self, ()> {
        Ok(match dto {
            ProofDto::Absence {
                proof,
                record,
                acknowledgement,
            } => Self::Absence(AbsenceAudit::from_parts(
                proof.try_into()?,
                record.try_into()?,
                parse_evidence(&acknowledgement)?,
            )),
            ProofDto::Resource { observations } => {
                let mut slots = [None, None, None];
                for (slot, a) in observations.into_iter().enumerate() {
                    if let Some(a) = a {
                        slots[slot] = Some(ResourceObservationAudit::from_parts(
                            a.record.try_into()?,
                            parse_evidence(&a.acknowledgement)?,
                            a.provider_acknowledged,
                        ));
                    }
                }
                Self::Resource(slots)
            }
        })
    }
}
impl From<&CloseCompletionRow> for CompletionDto {
    fn from(row: &CloseCompletionRow) -> Self {
        Self {
            root: row.close_lifetime().as_str().into(),
            record: EvidenceDto::from(row.record()),
            acknowledgement: evidence_name(row.acknowledgement()),
        }
    }
}
impl TryFrom<CompletionDto> for CloseCompletionRow {
    type Error = ();
    fn try_from(dto: CompletionDto) -> Result<Self, ()> {
        Ok(Self::from_parts(
            AgentLifetimeId::new(dto.root).map_err(|_| ())?,
            dto.record.try_into()?,
            parse_evidence(&dto.acknowledgement)?,
        ))
    }
}
