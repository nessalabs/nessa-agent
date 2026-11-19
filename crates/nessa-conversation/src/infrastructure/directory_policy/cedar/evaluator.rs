//! Cedar answers a directory question. This module only builds the question.

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid, PolicyId,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use std::str::FromStr;

use super::command_rules;
use crate::application::{
    DirectoryCommandQuestion, DirectoryPolicyEvaluator, ExecQuestion, PolicyAnswer,
    PolicyUnavailable, UseQuestion,
};
use crate::domain::program_name;

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("directory.cedar");

/// The directory policy bundle.
pub struct CedarDirectoryPolicyEvaluator {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

impl CedarDirectoryPolicyEvaluator {
    /// The checked-in directory policies, re-identified by their `@id`.
    pub fn new() -> Result<Self, PolicyUnavailable> {
        let schema = Schema::from_json_str(SCHEMA).map_err(|_| PolicyUnavailable)?;
        let parsed = PolicySet::from_str(POLICIES).map_err(|_| PolicyUnavailable)?;
        let mut policies = PolicySet::new();
        for policy in parsed.policies() {
            let Some(id) = policy.annotation("id").filter(|id| !id.is_empty()) else {
                return Err(PolicyUnavailable);
            };
            if command_rules::reserved(id) {
                return Err(PolicyUnavailable);
            }
            policies
                .add(policy.new_id(PolicyId::new(id)))
                .map_err(|_| PolicyUnavailable)?;
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            return Err(PolicyUnavailable);
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }

    fn decide(
        &self,
        capabilities: &[String],
        resource: Entity,
        action_name: &str,
        context: Context,
        policies: &PolicySet,
    ) -> Result<PolicyAnswer, PolicyUnavailable> {
        let principal = grant(capabilities)?;
        let action = uid("NessaDirectory::Action", action_name)?;
        let request = Request::new(
            principal.uid(),
            action,
            resource.uid(),
            context,
            Some(&self.schema),
        )
        .map_err(|_| PolicyUnavailable)?;
        let entities = Entities::from_entities([principal, resource], Some(&self.schema))
            .map_err(|_| PolicyUnavailable)?;
        let response = self.authorizer.is_authorized(&request, policies, &entities);
        if response.diagnostics().errors().next().is_some() {
            return Err(PolicyUnavailable);
        }
        let rules: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        Ok(match response.decision() {
            Decision::Allow => PolicyAnswer::Allow,
            Decision::Deny => PolicyAnswer::Deny { rules },
        })
    }
}

impl DirectoryPolicyEvaluator for CedarDirectoryPolicyEvaluator {
    fn use_directory(&self, question: &UseQuestion) -> Result<PolicyAnswer, PolicyUnavailable> {
        let context = Context::from_json_value(
            serde_json::json!({
                "leave": question.leave.as_str(),
                "marker": question.marker,
            }),
            Some((
                &self.schema,
                &uid("NessaDirectory::Action", &question.action)?,
            )),
        )
        .map_err(|_| PolicyUnavailable)?;
        self.decide(
            &question.capabilities,
            directory(&question.resource)?,
            &question.action,
            context,
            &self.policies,
        )
    }

    fn exec(&self, question: &ExecQuestion) -> Result<PolicyAnswer, PolicyUnavailable> {
        let context = Context::from_json_value(
            serde_json::json!({
                "leave": "inside",
                "marker": false,
                "profile": question.profile.as_str(),
                "hidesMarker": question.hides_marker,
                "rootLeave": question.root_leave.as_str(),
            }),
            Some((
                &self.schema,
                &uid("NessaDirectory::Action", "directory.exec")?,
            )),
        )
        .map_err(|_| PolicyUnavailable)?;
        self.decide(
            &question.capabilities,
            directory(&question.resource)?,
            "directory.exec",
            context,
            &self.policies,
        )
    }

    fn command(
        &self,
        question: &DirectoryCommandQuestion,
    ) -> Result<PolicyAnswer, PolicyUnavailable> {
        if !question.rules.constrains_commands() {
            return Ok(PolicyAnswer::Allow);
        }
        let policies = command_rules::policies(question.rules)?;
        let context = Context::from_json_value(
            serde_json::json!({ "argv": question.argv }),
            Some((
                &self.schema,
                &uid("NessaDirectory::Action", "directory.command")?,
            )),
        )
        .map_err(|_| PolicyUnavailable)?;
        self.decide(
            &question.capabilities,
            command_entity(question.program)?,
            "directory.command",
            context,
            &policies,
        )
    }
}

fn grant(capabilities: &[String]) -> Result<Entity, PolicyUnavailable> {
    Entity::new(
        uid("NessaDirectory::Grant", "grant")?,
        [(
            "capabilities".to_owned(),
            RestrictedExpression::new_set(
                capabilities
                    .iter()
                    .cloned()
                    .map(RestrictedExpression::new_string),
            ),
        )]
        .into_iter()
        .collect(),
        Default::default(),
    )
    .map_err(|_| PolicyUnavailable)
}

fn directory(resource: &str) -> Result<Entity, PolicyUnavailable> {
    Entity::new(
        uid("NessaDirectory::Directory", resource)?,
        [(
            "id".to_owned(),
            RestrictedExpression::new_string(resource.to_owned()),
        )]
        .into_iter()
        .collect(),
        Default::default(),
    )
    .map_err(|_| PolicyUnavailable)
}

fn command_entity(program: &str) -> Result<Entity, PolicyUnavailable> {
    Entity::new(
        uid("NessaDirectory::Command", "command")?,
        [(
            "programName".to_owned(),
            RestrictedExpression::new_string(program_name(program).to_owned()),
        )]
        .into_iter()
        .collect(),
        Default::default(),
    )
    .map_err(|_| PolicyUnavailable)
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, PolicyUnavailable> {
    let entity_type = EntityTypeName::from_str(entity_type).map_err(|_| PolicyUnavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}
