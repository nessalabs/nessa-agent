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
            credential.revoke(at)?;
        }
        Ok(credential)
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
}
