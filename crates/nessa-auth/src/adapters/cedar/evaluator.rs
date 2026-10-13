//! Embedded bundle validation and application policy-port implementation.

use crate::{
    application::ports::{
        AccessError, AccessSnapshot, Decision, GrantPolicyEvaluator, PolicyEvaluator,
    },
    domain::{Action, AuthContext, Grant, PrincipalKind, Resource},
};
use cedar_policy::{
    Authorizer, Context, Decision as CedarDecision, Entities, EntityTypeName, PolicySet, Request,
    Response, Schema, ValidationMode, Validator,
};
use sha2::{Digest, Sha256};
use std::str::FromStr;

use super::entities::{cedar_uid, entities, holding};

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

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
    /// Identity of the embedded read policy and schema for receiver epochs.
    pub fn profile_digest() -> String {
        let mut digest = Sha256::new();
        digest.update(SCHEMA.as_bytes());
        digest.update(POLICIES.as_bytes());
        format!("{:x}", digest.finalize())
    }
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

    /// Whether `action` is one the schema lets `principal` take on a
    /// `Nessa::Resource`. The schema is the one list of actions: anything else
    /// is denied, not an unavailable authority.
    fn applies(
        &self,
        principal: &str,
        action: &cedar_policy::EntityUid,
    ) -> Result<bool, AccessError> {
        let principal =
            EntityTypeName::from_str(principal).map_err(|_| AccessError::Unavailable)?;
        let resource =
            EntityTypeName::from_str("Nessa::Resource").map_err(|_| AccessError::Unavailable)?;
        let applies = self
            .schema
            .actions_for_principal_and_resource(&principal, &resource)
            .any(|known| known == action);
        Ok(applies)
    }

    /// Cedar's answer as Nessa's: any evaluation error denies, even when
    /// another policy permits.
    fn decide(&self, request: &Request, entities: &Entities) -> Decision {
        let response: Response = self
            .authorizer
            .is_authorized(request, &self.policies, entities);
        if response.diagnostics().errors().next().is_some() {
            return Decision::Deny;
        }
        match response.decision() {
            CedarDecision::Allow => Decision::Allow,
            CedarDecision::Deny => Decision::Deny,
        }
    }

    /// Whether the snapshot is the session's own identity. This is
    /// authentication, not policy, so it stays here rather than in Cedar: a
    /// request for one identity is never evaluated against another's facts.
    fn identity_matches(context: &AuthContext, snapshot: &AccessSnapshot) -> bool {
        snapshot.credential.id() == context.credential_id()
            && snapshot.credential.principal_id() == context.principal_id()
            && snapshot.credential.organization_id() == context.organization_id()
            && snapshot.credential.audience_id() == context.audience_id()
            && snapshot.membership.id() == context.membership_id()
            && snapshot.membership.principal_id() == context.principal_id()
            && snapshot.membership.organization_id() == context.organization_id()
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
        if !Self::identity_matches(context, snapshot) {
            return Ok(Decision::Deny);
        }
        let principal = cedar_uid("Nessa::Actor", context.principal_id().as_str())?;
        let action_uid = cedar_uid("Nessa::Action", action.as_str())?;
        if !self.applies("Nessa::Actor", &action_uid)? {
            return Ok(Decision::Deny);
        }
        let resource_uid = cedar_uid("Nessa::Resource", resource.id().as_str())?;
        let cedar_context =
            Context::from_json_value(serde_json::json!({}), Some((&self.schema, &action_uid)))
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
        Ok(self.decide(&request, &entities))
    }
}

impl GrantPolicyEvaluator for CedarPolicyEvaluator {
    fn may_hold(
        &self,
        kind: PrincipalKind,
        grant: &Grant,
        administrative: bool,
    ) -> Result<Decision, AccessError> {
        let hold = cedar_uid("Nessa::Action", "credential.hold")?;
        let (principal, resource, entities) = holding(&self.schema, kind, grant.resource())?;
        let context = Context::from_json_value(
            serde_json::json!({ "action": grant.action().as_str(), "administrative": administrative }),
            Some((&self.schema, &hold)),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let request = Request::new(principal, hold, resource, context, Some(&self.schema))
            .map_err(|_| AccessError::Unavailable)?;
        Ok(self.decide(&request, &entities))
    }
}

#[cfg(test)]
#[path = "evaluator_tests.rs"]
mod tests;
