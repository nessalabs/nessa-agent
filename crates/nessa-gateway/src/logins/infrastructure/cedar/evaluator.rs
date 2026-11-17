//! The saved-login policy: a machine's kind and switch, as checked Cedar
//! data, decided by `policies.cedar`.
use crate::logins::application::{LoginFacts, LoginPolicyEvaluator};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicyId,
    PolicySet, Request, Schema, ValidationMode, Validator,
};
use nessa_auth::application::ports::AccessError;
use nessa_sdk::domain::agent_execution::leases::EnvironmentRef;
use std::str::FromStr;

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

pub struct CedarLoginPolicyEvaluator {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

impl CedarLoginPolicyEvaluator {
    /// Strictly validate the checked-in bundle; every policy carries an `@id`.
    ///
    /// # Errors
    /// [`AccessError::Unavailable`] for any error or warning.
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

impl LoginPolicyEvaluator for CedarLoginPolicyEvaluator {
    fn may_receive(&self, facts: &LoginFacts<'_>) -> Result<bool, AccessError> {
        let reach = match facts.environment {
            EnvironmentRef::Here => "here",
            EnvironmentRef::Boat => "boat",
            EnvironmentRef::Ssh(_) => "ssh",
            EnvironmentRef::Peer(_) => "peer",
        };
        let machine = facts.environment.name();
        let agent = facts.agent.name();
        let entities = Entities::from_json_value(
            serde_json::json!([
                {"uid": {"type": "NessaLogins::Machine", "id": machine},
                 "attrs": {"reach": reach, "switchedOn": facts.switched_on}, "parents": []},
                {"uid": {"type": "NessaLogins::Login", "id": agent},
                 "attrs": {"agent": agent}, "parents": []}
            ]),
            Some(&self.schema),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let action = uid("NessaLogins::Action", "login.receive")?;
        let context =
            Context::from_json_value(serde_json::json!({}), Some((&self.schema, &action)))
                .map_err(|_| AccessError::Unavailable)?;
        let request = Request::new(
            uid("NessaLogins::Machine", &machine)?,
            action,
            uid("NessaLogins::Login", agent)?,
            context,
            Some(&self.schema),
        )
        .map_err(|_| AccessError::Unavailable)?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if response.diagnostics().errors().next().is_some() {
            return Err(AccessError::Unavailable);
        }
        let rules: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        let allowed = response.decision() == Decision::Allow;
        if !allowed {
            tracing::info!(
                machine,
                agent,
                ?rules,
                "a saved login is not handed to this machine"
            );
        }
        Ok(allowed)
    }
}
