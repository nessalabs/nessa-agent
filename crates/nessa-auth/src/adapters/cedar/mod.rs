//! Embedded Cedar authorization for Nessa's initial local policy profile.
//!
//! Cedar schema, policy, entities, and diagnostics stay inside this adapter.
//! Callers supply only Nessa domain values through the application-owned port.

use crate::{
    application::ports::{AccessError, AccessSnapshot, Decision, PolicyEvaluator},
    domain::{Action, AuthContext, MembershipRole, Resource},
};
use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, Entities, Entity, EntityId, EntityTypeName,
    EntityUid, PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use std::{collections::HashMap, str::FromStr};

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");
const SERVER_READ: &str = "server.read";
const CREDENTIAL_MANAGE: &str = "credential.manage";

/// A pre-parsed, strictly validated embedded Cedar policy bundle.
///
/// Construct this once in composition and inject it behind [`PolicyEvaluator`].
/// Evaluation fails closed when requests, entities, or Cedar diagnostics are invalid.
pub struct CedarPolicyEvaluator {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

impl CedarPolicyEvaluator {
    /// Load and strictly validate the checked-in schema and policy bundle.
    pub fn new() -> Result<Self, AccessError> {
        Self::from_sources(SCHEMA, POLICIES)
    }

    fn from_sources(schema_source: &str, policy_source: &str) -> Result<Self, AccessError> {
        let schema = Schema::from_json_str(schema_source).map_err(|_| AccessError::Unavailable)?;
        let policies = PolicySet::from_str(policy_source).map_err(|_| AccessError::Unavailable)?;
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            return Err(AccessError::Unavailable);
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }

    fn cedar_uid(entity_type: &str, id: &str) -> Result<EntityUid, AccessError> {
        let entity_type =
            EntityTypeName::from_str(entity_type).map_err(|_| AccessError::Unavailable)?;
        Ok(EntityUid::from_type_name_and_id(
            entity_type,
            EntityId::new(id),
        ))
    }

    fn entities(
        &self,
        context: &AuthContext,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Entities, AccessError> {
        let role = match snapshot.membership.role() {
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
        };
        let actor = Entity::new(
            Self::cedar_uid("Nessa::Actor", context.principal_id().as_str())?,
            HashMap::from([
                (
                    "organizationId".to_owned(),
                    RestrictedExpression::new_string(
                        snapshot.membership.organization_id().as_str().to_owned(),
                    ),
                ),
                (
                    "role".to_owned(),
                    RestrictedExpression::new_string(role.to_owned()),
                ),
                (
                    "active".to_owned(),
                    RestrictedExpression::new_bool(snapshot.membership.is_active()),
                ),
            ]),
            Default::default(),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let resource = Entity::new(
            Self::cedar_uid("Nessa::Resource", resource.id().as_str())?,
            HashMap::from([(
                "organizationId".to_owned(),
                RestrictedExpression::new_string(resource.organization_id().as_str().to_owned()),
            )]),
            Default::default(),
        )
        .map_err(|_| AccessError::Unavailable)?;
        Entities::from_entities([actor, resource], Some(&self.schema))
            .map_err(|_| AccessError::Unavailable)
    }

    fn identity_matches(context: &AuthContext, snapshot: &AccessSnapshot) -> bool {
        snapshot.credential.id() == context.credential_id()
            && snapshot.credential.principal_id() == context.principal_id()
            && snapshot.credential.organization_id() == context.organization_id()
            && snapshot.credential.audience_id() == context.audience_id()
            && snapshot.membership.id() == context.membership_id()
            && snapshot.membership.principal_id() == context.principal_id()
            && snapshot.membership.organization_id() == context.organization_id()
    }

    fn credential_allows(action: &Action, resource: &Resource, snapshot: &AccessSnapshot) -> bool {
        snapshot.credential.grants().iter().any(|grant| {
            grant.action() == action
                && grant.resource().id() == resource.id()
                && grant.resource().organization_id() == resource.organization_id()
        })
    }
}

impl PolicyEvaluator for CedarPolicyEvaluator {
    fn evaluate(
        &self,
        context: &AuthContext,
        action: &Action,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Decision, AccessError> {
        if !matches!(
            action.as_str(),
            SERVER_READ | CREDENTIAL_MANAGE | "conversation.write"
        ) {
            return Ok(Decision::Deny);
        }

        let principal = Self::cedar_uid("Nessa::Actor", context.principal_id().as_str())?;
        let action_uid = Self::cedar_uid("Nessa::Action", action.as_str())?;
        let resource_uid = Self::cedar_uid("Nessa::Resource", resource.id().as_str())?;
        let cedar_context = Context::from_json_value(
            serde_json::json!({
                "credentialAllows": Self::credential_allows(action, resource, snapshot),
                "identityMatches": Self::identity_matches(context, snapshot),
            }),
            Some((&self.schema, &action_uid)),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let request = Request::new(
            principal,
            action_uid,
            resource_uid,
            cedar_context,
            Some(&self.schema),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let entities = self.entities(context, resource, snapshot)?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);

        if response.diagnostics().errors().next().is_some() {
            return Ok(Decision::Deny);
        }
        Ok(match response.decision() {
            CedarDecision::Allow => Decision::Allow,
            CedarDecision::Deny => Decision::Deny,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AudienceId, Credential, CredentialId, Grant, Membership, MembershipId, MembershipStatus,
        OrganizationId, PrincipalId, ResourceId,
    };

    fn request_values() -> (AuthContext, Action, Resource, AccessSnapshot) {
        let principal_id = PrincipalId::new("principal-1").unwrap();
        let organization_id = OrganizationId::new("organization-1").unwrap();
        let membership_id = MembershipId::new("membership-1").unwrap();
        let credential_id = CredentialId::new("credential-1").unwrap();
        let audience_id = AudienceId::new("gateway-1").unwrap();
        let action = Action::new(SERVER_READ).unwrap();
        let resource = Resource::new(
            organization_id.clone(),
            ResourceId::new("gateway-1").unwrap(),
        );
        let credential = Credential::new(
            credential_id.clone(),
            principal_id.clone(),
            organization_id.clone(),
            audience_id.clone(),
            100,
            200,
            vec![Grant::new(action.clone(), resource.clone())],
        )
        .unwrap();
        let membership = Membership::new(
            membership_id.clone(),
            principal_id.clone(),
            organization_id.clone(),
            MembershipRole::Member,
            MembershipStatus::Active,
        );
        let context = AuthContext::new(
            principal_id,
            organization_id,
            membership_id,
            credential_id,
            audience_id,
        );
        (
            context,
            action,
            resource,
            AccessSnapshot {
                credential,
                membership,
                revision: 1,
            },
        )
    }

    #[test]
    fn checked_in_bundle_is_valid() {
        CedarPolicyEvaluator::new().expect("checked-in Cedar bundle must validate");
    }

    #[test]
    fn malformed_schema_is_rejected() {
        assert!(CedarPolicyEvaluator::from_sources("not json", POLICIES).is_err());
    }

    #[test]
    fn malformed_or_invalid_policy_is_rejected() {
        assert!(CedarPolicyEvaluator::from_sources(SCHEMA, "permit(").is_err());
        assert!(CedarPolicyEvaluator::from_sources(
            SCHEMA,
            r#"permit(principal, action == Nessa::Action::"unknown", resource);"#,
        )
        .is_err());
    }

    #[test]
    fn evaluation_diagnostics_fail_closed_even_when_another_policy_permits() {
        let policies = r#"
            permit(
                principal is Nessa::Actor,
                action == Nessa::Action::"server.read",
                resource is Nessa::Resource
            );
            permit(
                principal is Nessa::Actor,
                action == Nessa::Action::"server.read",
                resource is Nessa::Resource
            ) when { 9223372036854775807 * 2 == 0 };
        "#;
        let evaluator = CedarPolicyEvaluator::from_sources(SCHEMA, policies).unwrap();
        let (context, action, resource, snapshot) = request_values();

        assert_eq!(
            evaluator.evaluate(&context, &action, &resource, &snapshot),
            Ok(Decision::Deny)
        );
    }
}
