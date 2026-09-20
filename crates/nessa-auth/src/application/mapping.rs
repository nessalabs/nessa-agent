//! Validation mappings, not authorization. Only trusted use cases persist results.
use super::dto::*;
use crate::domain::*;

impl TryFrom<PrincipalInputDto> for Principal {
    type Error = DomainError;
    fn try_from(dto: PrincipalInputDto) -> Result<Self, Self::Error> {
        Ok(Self::new(
            PrincipalId::new(dto.id)?,
            match dto.kind {
                PrincipalKindDto::Human => PrincipalKind::Human,
                PrincipalKindDto::Agent => PrincipalKind::Agent,
                PrincipalKindDto::Integration => PrincipalKind::Integration,
            },
        ))
    }
}
impl TryFrom<OrganizationInputDto> for Organization {
    type Error = DomainError;
    fn try_from(dto: OrganizationInputDto) -> Result<Self, Self::Error> {
        Ok(Self::new(OrganizationId::new(dto.id)?))
    }
}
impl TryFrom<MembershipInputDto> for Membership {
    type Error = DomainError;
    fn try_from(dto: MembershipInputDto) -> Result<Self, Self::Error> {
        Ok(Self::new(
            MembershipId::new(dto.id)?,
            PrincipalId::new(dto.principal_id)?,
            OrganizationId::new(dto.organization_id)?,
            match dto.role {
                MembershipRoleDto::Admin => MembershipRole::Admin,
                MembershipRoleDto::Member => MembershipRole::Member,
            },
            match dto.state {
                MembershipStateDto::Active => MembershipStatus::Active,
                MembershipStateDto::Disabled => MembershipStatus::Disabled,
            },
        ))
    }
}
impl TryFrom<CredentialGrantDto> for Grant {
    type Error = DomainError;
    fn try_from(dto: CredentialGrantDto) -> Result<Self, Self::Error> {
        Ok(Self::new(
            Action::new(dto.action)?,
            Resource::new(
                OrganizationId::new(dto.resource.organization_id)?,
                ResourceId::new(dto.resource.id)?,
            ),
        ))
    }
}
impl TryFrom<CredentialMetadataDto> for Credential {
    type Error = DomainError;
    fn try_from(dto: CredentialMetadataDto) -> Result<Self, Self::Error> {
        let grants = dto
            .grants
            .into_iter()
            .map(Grant::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let mut credential = Self::new(
            CredentialId::new(dto.id)?,
            PrincipalId::new(dto.principal_id)?,
            OrganizationId::new(dto.organization_id)?,
            AudienceId::new(dto.audience_id)?,
            dto.issued_at,
            dto.expires_at,
            grants,
        )?;
        if let Some(at) = dto.revoked_at {
            credential.restore_revoked_at(at)?;
        }
        Ok(credential)
    }
}

impl TryFrom<CredentialTransitionDto> for CredentialTransition {
    type Error = DomainError;
    /// Validate recorded evidence under the same rule a live change satisfies.
    /// Storage-assigned `sequence`, `revision`, and `correlation` are not part
    /// of the domain record and are checked by the adapter that owns them.
    fn try_from(dto: CredentialTransitionDto) -> Result<Self, Self::Error> {
        let lifecycle = |value: CredentialLifecycleDto| CredentialLifecycle {
            issued_at: value.issued_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
        };
        let cause = match dto.cause {
            TransitionCauseDto::Issued { cause } => TransitionCause::Issued(match cause {
                IssuanceCauseDto::Bootstrap => IssuanceCause::Bootstrap,
                IssuanceCauseDto::AdminIssue => IssuanceCause::AdminIssue,
                IssuanceCauseDto::SurfaceProvision => IssuanceCause::SurfaceProvision,
                IssuanceCauseDto::OwnerRecovery => IssuanceCause::OwnerRecovery,
            }),
            TransitionCauseDto::Revoked { cause } => TransitionCause::Revoked(match cause {
                RevocationCauseDto::Explicit => RevocationCause::Explicit,
                RevocationCauseDto::Superseded { by, supersession } => {
                    RevocationCause::Superseded {
                        by: CredentialId::new(by)?,
                        kind: match supersession {
                            SupersessionDto::Provision => Supersession::Provision,
                            SupersessionDto::OwnerRecovery => Supersession::OwnerRecovery,
                        },
                    }
                }
            }),
            TransitionCauseDto::PredatesJournal => TransitionCause::PredatesJournal,
        };
        let initiator = match dto.initiator {
            InitiatorDto::Principal { id } => Initiator::Principal(PrincipalId::new(id)?),
            InitiatorDto::LocalOperator => Initiator::LocalOperator,
            InitiatorDto::Unknown => Initiator::Unknown,
        };
        Self::new(
            CredentialId::new(dto.credential_id)?,
            dto.before.map(lifecycle),
            lifecycle(dto.after),
            cause,
            initiator,
            dto.at,
        )
    }
}

impl CredentialTransitionDto {
    /// Record domain evidence under the identity its committing adapter assigns.
    pub fn record(
        transition: &CredentialTransition,
        sequence: u64,
        revision: u64,
        correlation: Option<String>,
    ) -> Self {
        let lifecycle = |value: &CredentialLifecycle| CredentialLifecycleDto {
            issued_at: value.issued_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
        };
        Self {
            sequence,
            revision,
            correlation,
            credential_id: transition.credential_id().as_str().to_owned(),
            before: transition.before().map(lifecycle),
            after: lifecycle(transition.after()),
            cause: match transition.cause() {
                TransitionCause::Issued(cause) => TransitionCauseDto::Issued {
                    cause: match cause {
                        IssuanceCause::Bootstrap => IssuanceCauseDto::Bootstrap,
                        IssuanceCause::AdminIssue => IssuanceCauseDto::AdminIssue,
                        IssuanceCause::SurfaceProvision => IssuanceCauseDto::SurfaceProvision,
                        IssuanceCause::OwnerRecovery => IssuanceCauseDto::OwnerRecovery,
                    },
                },
                TransitionCause::Revoked(cause) => TransitionCauseDto::Revoked {
                    cause: match cause {
                        RevocationCause::Explicit => RevocationCauseDto::Explicit,
                        RevocationCause::Superseded { by, kind } => {
                            RevocationCauseDto::Superseded {
                                by: by.as_str().to_owned(),
                                supersession: match kind {
                                    Supersession::Provision => SupersessionDto::Provision,
                                    Supersession::OwnerRecovery => SupersessionDto::OwnerRecovery,
                                },
                            }
                        }
                    },
                },
                TransitionCause::PredatesJournal => TransitionCauseDto::PredatesJournal,
            },
            initiator: match transition.initiator() {
                Initiator::Principal(id) => InitiatorDto::Principal {
                    id: id.as_str().to_owned(),
                },
                Initiator::LocalOperator => InitiatorDto::LocalOperator,
                Initiator::Unknown => InitiatorDto::Unknown,
            },
            at: transition.at(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deserialization_does_not_bypass_domain_validation() {
        let dto: OrganizationInputDto = serde_json::from_str(r#"{"id":""}"#).unwrap();
        assert!(Organization::try_from(dto).is_err());
    }
    #[test]
    fn metadata_mapping_rejects_cross_tenant_grants() {
        let dto = CredentialMetadataDto {
            id: "credential".into(),
            principal_id: "person".into(),
            organization_id: "org".into(),
            audience_id: "local".into(),
            issued_at: 1,
            expires_at: Some(2),
            revoked_at: None,
            grants: vec![CredentialGrantDto {
                action: "read".into(),
                resource: ResourceDto {
                    organization_id: "other".into(),
                    id: "resource".into(),
                },
            }],
        };
        assert!(Credential::try_from(dto).is_err());
    }

    #[test]
    fn transition_evidence_round_trips_through_its_record() {
        let mut credential = Credential::new(
            CredentialId::new("credential-1").unwrap(),
            PrincipalId::new("principal-1").unwrap(),
            OrganizationId::new("organization-1").unwrap(),
            AudienceId::new("gateway").unwrap(),
            100,
            None,
            vec![Grant::new(
                Action::new("server.read").unwrap(),
                Resource::new(
                    OrganizationId::new("organization-1").unwrap(),
                    ResourceId::new("gateway").unwrap(),
                ),
            )],
        )
        .unwrap();
        let issued = credential
            .issued(
                IssuanceCause::AdminIssue,
                Initiator::Principal(PrincipalId::new("owner").unwrap()),
            )
            .unwrap();
        let mut forged = CredentialTransitionDto::record(&issued, 1, 1, None);
        forged.initiator = InitiatorDto::Unknown;
        assert!(CredentialTransition::try_from(forged).is_err());
        let superseded = credential
            .supersede(
                50,
                CredentialId::new("credential-2").unwrap(),
                Supersession::OwnerRecovery,
                Initiator::LocalOperator,
            )
            .unwrap();
        for transition in [issued, superseded] {
            let record = CredentialTransitionDto::record(&transition, 1, 2, Some("r".into()));
            assert_eq!(CredentialTransition::try_from(record), Ok(transition));
        }
        assert!(credential
            .issued(IssuanceCause::Bootstrap, Initiator::LocalOperator)
            .is_err());
    }
}
