//! Embedded bundle validation and application policy-port implementation.

use crate::{
    application::ports::{AccessError, AccessSnapshot, Decision, PolicyEvaluator},
    domain::{Action, AuthContext, Resource},
};
use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, PolicySet, Request, Schema, ValidationMode,
    Validator,
};
use std::str::FromStr;

use super::entities::{cedar_uid, entities};

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

        let principal = cedar_uid("Nessa::Actor", context.principal_id().as_str())?;
        let action_uid = cedar_uid("Nessa::Action", action.as_str())?;
        let resource_uid = cedar_uid("Nessa::Resource", resource.id().as_str())?;
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
        let entities = entities(&self.schema, context, resource, snapshot)?;
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
#[path = "evaluator_tests.rs"]
mod tests;
