use super::super::fields::{identifier, string};
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::permissions::{
    PermissionDecision, PermissionEffect, PermissionOfferPolicy, PermissionOption,
    PermissionOptionId, PermissionOptions, PermissionScope,
};
use crate::infrastructure::json_rpc::{protocol, success, RpcId};
use serde_json::{json, Value};
pub(crate) fn permission_cancel(id: &RpcId) -> Value {
    success(id, json!({"outcome":{"outcome":"cancelled"}}))
}
pub(crate) fn selected(id: &RpcId, option: &str) -> Value {
    success(
        id,
        json!({"outcome":{"outcome":"selected","optionId":option}}),
    )
}
pub(crate) fn permission_options(
    params: &Value,
    config: &PermissionOfferPolicy,
) -> Result<PermissionOptions, AgentError> {
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing permission options"))?;
    let mut seen = std::collections::HashSet::new();
    let mut admitted = Vec::new();
    for option in options {
        let id = PermissionOptionId::new(identifier(option, "optionId")?)
            .map_err(|error| protocol(&error.to_string()))?;
        if !seen.insert(id.clone()) {
            return Err(protocol("duplicate permission option"));
        }
        let label = string(option, "name")?;
        if label.trim().is_empty() {
            return Err(protocol("empty permission option label"));
        }
        // ACP's persistent kinds carry no structured boundary here. Do not
        // fabricate a session/application scope from labels or option IDs.
        let effect = match string(option, "kind")? {
            "allow_once" => PermissionEffect::Allow,
            "reject_once" => PermissionEffect::Deny,
            "allow_always" | "reject_always" => continue,
            _ => return Err(protocol("unknown permission option kind")),
        };
        admitted.push(
            PermissionOption::new(
                id,
                label,
                PermissionDecision::new(effect, PermissionScope::request()),
            )
            .map_err(|error| protocol(&error.to_string()))?,
        );
    }
    let options = admitted;
    let options =
        PermissionOptions::new(options, config).map_err(|error| protocol(&error.to_string()))?;
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_mapping_filters_through_domain_configuration_without_downgrading_scope() {
        let options = json!({"options":[
            {"optionId":"allow", "kind":"allow_once", "name":"Allow"},
            {"optionId":"deny", "kind":"reject_once", "name":"Deny"},
            {"optionId":"persistent", "kind":"allow_always", "name":"Always allow"}
        ]});
        let deny = PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request());
        let config = PermissionOfferPolicy::new(vec![deny.clone()]).unwrap();
        let admitted = permission_options(&options, &config).unwrap();
        assert_eq!(admitted.choices().len(), 1);
        assert_eq!(admitted.choices()[0].id().as_str(), "deny");
        assert_eq!(admitted.choices()[0].decision(), &deny);
        // Transport validates identities even for persistent choices it cannot expose.
        for rejected in [
            json!({"options":[{"optionId":"same","kind":"allow_always","name":"Always"},{"optionId":"same","kind":"reject_once","name":"Deny"}]}),
            json!({"options":[{"optionId":"persistent","kind":"allow_always","name":"Always"}]}),
            json!({"options":[{"optionId":"deny","kind":"reject_once","name":" "}]}),
        ] {
            assert!(permission_options(&rejected, &config).is_err());
        }
    }
}
