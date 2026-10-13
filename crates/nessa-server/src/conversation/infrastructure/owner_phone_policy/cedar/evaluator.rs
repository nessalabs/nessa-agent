//! Canonical purpose and repository facts are translated into checked Cedar data.
use crate::conversation::application::{
    OwnerPhoneDecision, OwnerPhoneOperation, OwnerPhonePolicyEvaluator, OwnerPhonePolicyFacts,
};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, EntityId, EntityTypeName, EntityUid, PolicyId,
    PolicySet, Request, Schema, ValidationMode, Validator,
};
use nessa_auth::application::ports::AccessError;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

/// Checked first-party phone scope, additional to Auth's coarse action policy.
pub struct CedarOwnerPhonePolicyEvaluator {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}
impl CedarOwnerPhonePolicyEvaluator {
    /// Complete read-policy identity. A change to either authority must advance
    /// the receiver journal's policy revision before old progress is reused.
    pub fn receiver_profile_digest(auth_profile: &str, conversation_sources: &[&str]) -> String {
        let mut digest = Sha256::new();
        digest.update(b"nessa.receiver-policy.owner-phone.v1\0");
        for source in [auth_profile, SCHEMA, POLICIES]
            .into_iter()
            .chain(conversation_sources.iter().copied())
        {
            digest.update((source.len() as u64).to_be_bytes());
            digest.update(source.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }
    /// Strictly validate the checked-in bundle before composing any route.
    pub fn new() -> Result<Self, AccessError> {
        Self::from_sources(SCHEMA, POLICIES)
    }
    fn from_sources(schema: &str, policies: &str) -> Result<Self, AccessError> {
        let schema = Schema::from_json_str(schema).map_err(|_| AccessError::Unavailable)?;
        let parsed = PolicySet::from_str(policies).map_err(|_| AccessError::Unavailable)?;
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
impl OwnerPhonePolicyEvaluator for CedarOwnerPhonePolicyEvaluator {
    fn evaluate(
        &self,
        facts: OwnerPhonePolicyFacts<'_>,
    ) -> Result<OwnerPhoneDecision, AccessError> {
        let context = facts.context;
        let purpose = facts.purpose;
        let receiver = facts.receiver;
        let principal = uid("NessaPhone::Phone", context.credential_id().as_str())?;
        let scope = uid("NessaPhone::Scope", facts.gateway.id().as_str())?;
        let (action_name, request_context) = match facts.operation {
            OwnerPhoneOperation::Linkage => ("phone.context", serde_json::json!({})),
            OwnerPhoneOperation::ProductMethod {
                method,
                published_action,
            } => {
                let mut context = serde_json::json!({"method": method});
                if let Some(action) = published_action {
                    context["publishedAction"] = action.into();
                }
                ("product.request", context)
            }
            OwnerPhoneOperation::Creation {
                requested_mode,
                default_mode,
            } => {
                let mut context = serde_json::json!({"defaultMode": default_mode});
                if let Some(mode) = requested_mode {
                    context["requestedMode"] = mode.into();
                }
                ("conversation.create", context)
            }
            OwnerPhoneOperation::SubmissionInput { linked_files } => {
                let count = i64::try_from(linked_files).map_err(|_| AccessError::Unavailable)?;
                (
                    "conversation.input",
                    serde_json::json!({"linkedFiles": count}),
                )
            }
        };
        let resource = serde_json::json!({
            "gatewayId": facts.gateway.id().as_str(),
            "organization": facts.gateway.organization_id().as_str()
        });
        let entities = Entities::from_json_value(serde_json::json!([
            {"uid": {"type":"NessaPhone::Phone", "id":context.credential_id().as_str()}, "attrs": {
                "owner":context.principal_id().as_str(), "organization":context.organization_id().as_str(),
                "membership":context.membership_id().as_str(), "audience":context.audience_id().as_str(),
                "credential":context.credential_id().as_str(),
                "purposeOwner":purpose.owner().as_str(), "purposeOrganization":purpose.organization().as_str(),
                "purposeMembership":purpose.membership().as_str(), "purposeAudience":purpose.audience().as_str(),
                "purposeCredential":purpose.credential().as_str(), "purposeReceiver":purpose.receiver().as_str(),
                // Full u64 progress is retained as data. Current holding and epoch
                // correlation belong to the receiver join before this policy port.
                "pairedEpoch":purpose.paired_epoch().to_string(),
                "receiverId":receiver.receiver_id, "receiverOwner":receiver.owner_id.as_str(),
                "receiverOrganization":receiver.organization_id.as_str(), "receiverCredential":receiver.credential_id.as_str(),
                "receiverActive":receiver.active, "receiverEpoch":receiver.access_epoch.to_string()
            }, "parents":[]},
            {"uid":{"type":"NessaPhone::Scope", "id":facts.gateway.id().as_str()}, "attrs":resource, "parents":[]}
        ]), Some(&self.schema)).map_err(|_| AccessError::Unavailable)?;
        let action = uid("NessaPhone::Action", action_name)?;
        let context = Context::from_json_value(request_context, Some((&self.schema, &action)))
            .map_err(|_| AccessError::Unavailable)?;
        let request = Request::new(principal, action, scope, context, Some(&self.schema))
            .map_err(|_| AccessError::Unavailable)?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if response.diagnostics().errors().next().is_some() {
            return Err(AccessError::Unavailable);
        }
        let mut rules: Vec<String> = response
            .diagnostics()
            .reason()
            .map(ToString::to_string)
            .collect();
        rules.sort();
        Ok(match response.decision() {
            Decision::Allow => OwnerPhoneDecision::Allow { rules },
            Decision::Deny => OwnerPhoneDecision::Deny { rules },
        })
    }
}

#[cfg(test)]
#[path = "../../../../../tests/conversation/owner_phone_policy.rs"]
mod tests;
