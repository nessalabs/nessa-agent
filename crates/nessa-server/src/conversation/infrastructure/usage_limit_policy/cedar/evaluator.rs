//! Current caller and gateway facts, checked against the preference bundle.
use crate::conversation::application::{
    UsageLimitPreferenceDecision, UsageLimitPreferenceFacts, UsageLimitPreferencePolicyEvaluator,
};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicyId,
    PolicySet, Request, Schema, ValidationMode, Validator,
};
use nessa_auth::application::ports::AccessError;
use std::str::FromStr;

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

/// The gateway usage-limit preference. A member is forbidden even with the grant.
pub struct CedarUsageLimitPreferencePolicy {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

impl CedarUsageLimitPreferencePolicy {
    /// Strictly validate the checked-in bundle before composing any route.
    pub fn new() -> Result<Self, AccessError> {
        let schema = Schema::from_json_str(SCHEMA).map_err(|_| AccessError::Unavailable)?;
        let parsed = PolicySet::from_str(POLICIES).map_err(|_| AccessError::Unavailable)?;
        let mut policies = PolicySet::new();
        for policy in parsed.policies() {
            let id = policy
                .annotation("id")
                .filter(|id| !id.is_empty())
                .ok_or(AccessError::Unavailable)?;
            policies
                .add(policy.new_id(PolicyId::new(id)))
                .map_err(|_| AccessError::Unavailable)?;
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            tracing::warn!(
                ?validation,
                "the usage-limit preference policy did not validate"
            );
            return Err(AccessError::Unavailable);
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }
}

fn uid(kind: &str, id: &str) -> Result<EntityUid, AccessError> {
    let kind = EntityTypeName::from_str(kind).map_err(|_| AccessError::Unavailable)?;
    Ok(EntityUid::from_type_name_and_id(kind, EntityId::new(id)))
}

impl UsageLimitPreferencePolicyEvaluator for CedarUsageLimitPreferencePolicy {
    fn evaluate(
        &self,
        facts: &UsageLimitPreferenceFacts,
    ) -> Result<UsageLimitPreferenceDecision, AccessError> {
        let grants: Vec<_> = facts
            .grants
            .iter()
            .map(|grant| {
                serde_json::json!({
                    "action": grant.action,
                    "resourceId": grant.resource_id,
                    "organizationId": grant.organization_id,
                })
            })
            .collect();
        let entities = Entities::from_json_value(
            serde_json::json!([
                {
                    "uid": {"type": "NessaUsageLimit::Caller", "id": facts.principal_id},
                    "attrs": {
                        "organizationId": facts.organization_id,
                        "role": facts.role,
                        "active": facts.active,
                        "grants": grants,
                    },
                    "parents": []
                },
                {
                    "uid": {"type": "NessaUsageLimit::Gateway", "id": facts.gateway_id},
                    "attrs": {
                        "id": facts.gateway_id,
                        "organizationId": facts.gateway_organization,
                    },
                    "parents": []
                }
            ]),
            Some(&self.schema),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let action = uid("NessaUsageLimit::Action", "preference.set")?;
        let context =
            Context::from_json_value(serde_json::json!({}), Some((&self.schema, &action)))
                .map_err(|_| AccessError::Unavailable)?;
        let principal = uid("NessaUsageLimit::Caller", &facts.principal_id)?;
        let resource = uid("NessaUsageLimit::Gateway", &facts.gateway_id)?;
        let request = Request::new(principal, action, resource, context, Some(&self.schema))
            .map_err(|_| AccessError::Unavailable)?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if response.diagnostics().errors().next().is_some() {
            return Err(AccessError::Unavailable);
        }
        let mut policies: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        policies.sort();
        Ok(match response.decision() {
            Decision::Allow => UsageLimitPreferenceDecision::Allow { policies },
            Decision::Deny => UsageLimitPreferenceDecision::Deny { policies },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::application::UsageLimitPreferenceGrant;

    fn facts(
        role: &str,
        active: bool,
        grant: bool,
        organization: &str,
    ) -> UsageLimitPreferenceFacts {
        UsageLimitPreferenceFacts {
            principal_id: "person".into(),
            organization_id: organization.into(),
            role: role.into(),
            active,
            grants: grant
                .then(|| UsageLimitPreferenceGrant {
                    action: "credential.manage".into(),
                    resource_id: "gateway".into(),
                    organization_id: "org".into(),
                })
                .into_iter()
                .collect(),
            gateway_id: "gateway".into(),
            gateway_organization: "org".into(),
        }
    }

    fn policy() -> CedarUsageLimitPreferencePolicy {
        CedarUsageLimitPreferencePolicy::new().expect("preference policy validates")
    }

    #[test]
    fn an_active_member_with_the_grant_is_forbidden() {
        let decision = policy()
            .evaluate(&facts("member", true, true, "org"))
            .unwrap();
        let UsageLimitPreferenceDecision::Deny { policies } = decision else {
            panic!("a member was allowed");
        };
        assert!(policies
            .iter()
            .any(|id| id == "usage-limit-preference.member"));
    }

    #[test]
    fn an_active_admin_with_the_grant_is_allowed() {
        let decision = policy()
            .evaluate(&facts("admin", true, true, "org"))
            .unwrap();
        let UsageLimitPreferenceDecision::Allow { policies } = decision else {
            panic!("an admin with the grant was refused");
        };
        assert!(policies
            .iter()
            .any(|id| id == "usage-limit-preference.admin-grant"));
    }

    #[test]
    fn an_admin_without_the_grant_is_refused() {
        let decision = policy()
            .evaluate(&facts("admin", true, false, "org"))
            .unwrap();
        assert!(matches!(
            decision,
            UsageLimitPreferenceDecision::Deny { .. }
        ));
    }

    #[test]
    fn an_admin_of_another_organization_is_refused() {
        let decision = policy()
            .evaluate(&facts("admin", true, true, "other"))
            .unwrap();
        let UsageLimitPreferenceDecision::Deny { policies } = decision else {
            panic!("a foreign admin was allowed");
        };
        assert!(policies
            .iter()
            .any(|id| id == "usage-limit-preference.foreign-organization"));
    }

    #[test]
    fn an_inactive_admin_with_the_grant_is_refused() {
        let decision = policy()
            .evaluate(&facts("admin", false, true, "org"))
            .unwrap();
        let UsageLimitPreferenceDecision::Deny { policies } = decision else {
            panic!("an inactive admin was allowed");
        };
        assert!(policies
            .iter()
            .any(|id| id == "usage-limit-preference.inactive"));
    }
}
