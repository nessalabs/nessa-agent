//! The deletion-inventory policy bundle, loaded and validated once.
use crate::backup::application::{
    InventoryCapturePolicyEvaluator, InventoryDecision, InventoryDenial, InventoryFacts,
    RestoreFacts, RestorePolicyEvaluator,
};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

/// The checked-in inventory policy, strictly validated.
pub(crate) struct CedarInventoryPolicy {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

/// A step of building a request or of evaluating it that failed. The write
/// is denied, and this is logged.
struct Unavailable(String);

impl Unavailable {
    fn at(step: &str, error: impl std::fmt::Display) -> Self {
        Self(format!("{step}: {error}"))
    }
}

impl CedarInventoryPolicy {
    /// Load and strictly validate the checked-in bundle.
    ///
    /// # Errors
    /// The bundle does not parse, does not validate without warnings, or
    /// holds a policy without an `@id`. The message says which.
    pub(crate) fn new() -> Result<Self, String> {
        Self::from_sources(SCHEMA, POLICIES)
    }

    pub(crate) fn from_sources(schema: &str, policies: &str) -> Result<Self, String> {
        let schema = Schema::from_json_str(schema)
            .map_err(|error| format!("the inventory policy schema does not parse: {error}"))?;
        let policies = PolicySet::from_str(policies)
            .map_err(|error| format!("the inventory policies do not parse: {error}"))?;
        if let Some(unnamed) = policies
            .policies()
            .find(|policy| policy.annotation("id").is_none_or(str::is_empty))
        {
            return Err(format!("an inventory policy has no @id: {}", unnamed.id()));
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            let found: Vec<String> = validation
                .validation_errors()
                .map(ToString::to_string)
                .chain(validation.validation_warnings().map(ToString::to_string))
                .collect();
            return Err(format!(
                "the inventory policies do not validate: {}",
                found.join("; ")
            ));
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }

    fn decide(
        &self,
        kind: &str,
        cause: &str,
        action: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Result<InventoryDecision, Unavailable> {
        let principal = Entity::new(
            uid("NessaBackup::Principal", kind)?,
            HashMap::from([(
                "kind".to_owned(),
                RestrictedExpression::new_string(kind.to_owned()),
            )]),
            HashSet::new(),
        )
        .map_err(|error| Unavailable::at("the principal", error))?;
        let resource = Entity::new_no_attrs(uid(resource_type, resource_id)?, HashSet::new());
        let context = Context::from_pairs([(
            "cause".to_owned(),
            RestrictedExpression::new_string(cause.to_owned()),
        )])
        .map_err(|error| Unavailable::at("the context", error))?;
        let request = Request::new(
            principal.uid(),
            uid("NessaBackup::Action", action)?,
            resource.uid(),
            context,
            Some(&self.schema),
        )
        .map_err(|error| Unavailable::at("the request", error))?;
        let entities = Entities::from_entities([principal, resource], Some(&self.schema))
            .map_err(|error| Unavailable::at("the entities", error))?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if let Some(error) = response.diagnostics().errors().next() {
            return Err(Unavailable::at("evaluation", error));
        }
        let policies: Vec<String> = response
            .diagnostics()
            .reason()
            .filter_map(|id| self.policies.annotation(id, "id"))
            .map(str::to_owned)
            .collect();
        Ok(match response.decision() {
            Decision::Allow => InventoryDecision::Allow { policies },
            Decision::Deny if policies.is_empty() => {
                InventoryDecision::Deny(InventoryDenial::NotPermitted)
            }
            Decision::Deny => InventoryDecision::Deny(InventoryDenial::Forbidden { policies }),
        })
    }
}

impl InventoryCapturePolicyEvaluator for CedarInventoryPolicy {
    fn evaluate(&self, facts: &InventoryFacts<'_>) -> InventoryDecision {
        self.decide(
            facts.kind,
            facts.cause.as_str(),
            facts.action.as_str(),
            "NessaBackup::Inventory",
            "inventory",
        )
        .unwrap_or_else(|Unavailable(error)| {
            tracing::error!(
                %error,
                kind = facts.kind,
                cause = facts.cause.as_str(),
                action = facts.action.as_str(),
                "the inventory policy could not decide; the write is refused"
            );
            InventoryDecision::Deny(InventoryDenial::Unavailable)
        })
    }
}

impl RestorePolicyEvaluator for CedarInventoryPolicy {
    fn evaluate(&self, facts: &RestoreFacts<'_>) -> InventoryDecision {
        self.decide(
            facts.kind,
            facts.cause.as_str(),
            facts.action.as_str(),
            "NessaBackup::Restore",
            "restore",
        )
        .unwrap_or_else(|Unavailable(error)| {
            tracing::error!(
                %error,
                kind = facts.kind,
                cause = facts.cause.as_str(),
                action = facts.action.as_str(),
                "the restore policy could not decide; the restore is refused"
            );
            InventoryDecision::Deny(InventoryDenial::Unavailable)
        })
    }
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, Unavailable> {
    let entity_type = EntityTypeName::from_str(entity_type)
        .map_err(|error| Unavailable::at("an entity type", error))?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unnamed_policy_is_refused() {
        let policies = r#"
            permit (
                principal is NessaBackup::Principal,
                action == NessaBackup::Action::"deletion.inventory.capture",
                resource is NessaBackup::Inventory
            );
        "#;
        match CedarInventoryPolicy::from_sources(SCHEMA, policies) {
            Err(error) => assert!(error.contains("@id"), "{error}"),
            Ok(_) => panic!("an unnamed policy was accepted"),
        }
    }
}
