use super::super::fields::{identifier, string};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::permissions::{
    remember_effect, REMEMBER_ALLOW, REMEMBER_DENY,
};
use crate::domain::agent_execution::permissions::{
    PermissionDecision, PermissionEffect, PermissionOfferPolicy, PermissionOption,
    PermissionOptionId, PermissionOptions, PermissionScope, MAX_REVIEW_OPTIONS,
    MAX_REVIEW_OPTION_LABEL_BYTES,
};
use crate::infrastructure::json_rpc::{protocol, success, RpcId};
use serde_json::{json, Value};
use std::collections::HashSet;

/// Whether a harness's persistent kinds are offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PersistentKinds {
    /// OpenCode: `allow_always` and `reject_always` carry no boundary here.
    Drop,
    /// Claude and Codex: keep them, as a request-scoped choice with the
    /// harness's own option id. Nessa does not replace that choice.
    Keep,
}

/// Options admitted from one permission request.
pub(crate) struct AdmittedPermissions {
    /// Choices the host may offer, after the offer policy.
    pub options: PermissionOptions,
    /// Harness persistent option ids among `options`. Selecting one is the
    /// harness's own memory, not a Nessa rule.
    pub persistent: Vec<PermissionOptionId>,
}
pub(crate) fn permission_cancel(id: &RpcId) -> Value {
    success(id, json!({"outcome":{"outcome":"cancelled"}}))
}
pub(crate) fn selected(id: &RpcId, option: &str) -> Value {
    success(
        id,
        json!({"outcome":{"outcome":"selected","optionId":option}}),
    )
}
/// A selected option plus the reason the deadline recorded.
///
/// The reason is `_meta`, which a harness may show its model and must not be
/// required to. A person's answer does not use this: their selection carries
/// no deadline reason.
pub(crate) fn selected_because(id: &RpcId, option: &str, reason: &str) -> Value {
    success(
        id,
        json!({
            "outcome": {"outcome": "selected", "optionId": option},
            "_meta": {"deadlineReason": reason}
        }),
    )
}
#[cfg(test)]
pub(crate) fn permission_options(
    params: &Value,
    config: &PermissionOfferPolicy,
) -> Result<PermissionOptions, AgentError> {
    Ok(admit(params, config, PersistentKinds::Drop, false)?.options)
}

/// Parse `params` the way [`permission_options`] does.
///
/// `persistent` keeps Claude and Codex `allow_always` / `reject_always` as
/// request-scoped choices with their own ids. They are not given a session
/// or application scope: the harness remembers them, and the id is how that
/// memory is selected. `remember` adds Nessa's project rules beside them,
/// only when a once choice of the same effect survived, so the current
/// review can still be answered with that once choice.
pub(crate) fn admit(
    params: &Value,
    config: &PermissionOfferPolicy,
    persistent: PersistentKinds,
    remember: bool,
) -> Result<AdmittedPermissions, AgentError> {
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("missing permission options"))?;
    let mut seen = HashSet::new();
    let mut admitted = Vec::new();
    let mut persistent_ids = Vec::new();
    for option in options {
        let id = PermissionOptionId::new(identifier(option, "optionId")?)
            .map_err(|error| protocol(&error.to_string()))?;
        // These ids are Nessa's project choices. A provider that sends them
        // is not offering that choice, and selecting it must not store a rule.
        if remember_effect(id.as_str()).is_some() {
            return Err(protocol("permission option id is reserved"));
        }
        if !seen.insert(id.clone()) {
            return Err(protocol("duplicate permission option"));
        }
        let label = string(option, "name")?;
        if label.trim().is_empty() {
            return Err(protocol("empty permission option label"));
        }
        // A choice no surface could show is refused with the request, so the
        // review is declined at once rather than left waiting unseen.
        if label.len() > MAX_REVIEW_OPTION_LABEL_BYTES {
            return Err(protocol("permission option label too long"));
        }
        // A persistent kind is the harness's own memory. Keeping it does not
        // invent a session or application scope; the option stays one request,
        // and its id is what the harness is sent.
        let (effect, harness_persistent) = match string(option, "kind")? {
            "allow_once" => (PermissionEffect::Allow, false),
            "reject_once" => (PermissionEffect::Deny, false),
            "allow_always" if persistent == PersistentKinds::Keep => {
                (PermissionEffect::Allow, true)
            }
            "reject_always" if persistent == PersistentKinds::Keep => {
                (PermissionEffect::Deny, true)
            }
            "allow_always" | "reject_always" => continue,
            _ => return Err(protocol("unknown permission option kind")),
        };
        if harness_persistent {
            persistent_ids.push(id.clone());
        }
        admitted.push(
            PermissionOption::new(
                id,
                label,
                PermissionDecision::new(effect, PermissionScope::request()),
            )
            .map_err(|error| protocol(&error.to_string()))?,
        );
    }
    if remember {
        let allow_once = has_once(&admitted, &persistent_ids, PermissionEffect::Allow);
        let deny_once = has_once(&admitted, &persistent_ids, PermissionEffect::Deny);
        append_remember_options(&mut admitted, allow_once, deny_once)?;
    }
    let persistent = persistent_ids
        .into_iter()
        .filter(|id| admitted.iter().any(|option| option.id() == id))
        .collect();
    if admitted.len() > MAX_REVIEW_OPTIONS {
        return Err(protocol("too many permission options"));
    }
    let options =
        PermissionOptions::new(admitted, config).map_err(|error| protocol(&error.to_string()))?;
    Ok(AdmittedPermissions {
        options,
        persistent,
    })
}

fn has_once(
    admitted: &[PermissionOption],
    persistent: &[PermissionOptionId],
    effect: PermissionEffect,
) -> bool {
    admitted.iter().any(|option| {
        option.decision().effect() == effect
            && !persistent.iter().any(|id| id == option.id())
            && remember_effect(option.id().as_str()).is_none()
    })
}

fn append_remember_options(
    admitted: &mut Vec<PermissionOption>,
    allow_once: bool,
    deny_once: bool,
) -> Result<(), AgentError> {
    let push = |admitted: &mut Vec<PermissionOption>, id: &str, label: &str, effect| {
        if admitted.iter().any(|option| option.id().as_str() == id) {
            return Ok(());
        }
        admitted.push(
            PermissionOption::new(
                PermissionOptionId::new(id).map_err(|error| protocol(&error.to_string()))?,
                label.to_owned(),
                PermissionDecision::new(effect, PermissionScope::request()),
            )
            .map_err(|error| protocol(&error.to_string()))?,
        );
        Ok(())
    };
    if allow_once {
        push(
            admitted,
            REMEMBER_ALLOW,
            "Always allow in this project",
            PermissionEffect::Allow,
        )?;
    }
    if deny_once {
        push(
            admitted,
            REMEMBER_DENY,
            "Always deny in this project",
            PermissionEffect::Deny,
        )?;
    }
    Ok(())
}

/// The once option of `effect`: not a harness persistent id, and not a
/// Nessa remember id. The first one, when several match.
pub(crate) fn once_option(
    options: &PermissionOptions,
    effect: PermissionEffect,
    persistent: &[PermissionOptionId],
) -> Option<PermissionOptionId> {
    options
        .choices()
        .iter()
        .find(|option| {
            option.decision().effect() == effect
                && remember_effect(option.id().as_str()).is_none()
                && !persistent.iter().any(|id| id == option.id())
        })
        .map(|option| option.id().clone())
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

    #[test]
    fn claude_and_codex_keep_harness_persistent_choices_and_can_add_project_rules() {
        let options = json!({"options":[
            {"optionId":"allow", "kind":"allow_once", "name":"Allow"},
            {"optionId":"session", "kind":"allow_always", "name":"Always allow"},
            {"optionId":"deny", "kind":"reject_once", "name":"Deny"},
            {"optionId":"never", "kind":"reject_always", "name":"Always deny"}
        ]});
        let once = PermissionOfferPolicy::once_only();
        let kept = admit(&options, &once, PersistentKinds::Keep, false).unwrap();
        let ids: Vec<_> = kept
            .options
            .choices()
            .iter()
            .map(|option| option.id().as_str())
            .collect();
        assert_eq!(ids, ["allow", "session", "deny", "never"]);
        assert_eq!(
            kept.persistent
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
            ["session", "never"]
        );
        assert!(kept.options.choices().iter().all(|option| {
            *option.decision().scope()
                == crate::domain::agent_execution::permissions::PermissionScope::request()
        }));
        let remembered = admit(&options, &once, PersistentKinds::Keep, true).unwrap();
        let ids: Vec<_> = remembered
            .options
            .choices()
            .iter()
            .map(|option| option.id().as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "allow",
                "session",
                "deny",
                "never",
                "nessa.remember.allow",
                "nessa.remember.deny"
            ]
        );
        assert_eq!(
            once_option(
                &remembered.options,
                PermissionEffect::Allow,
                &remembered.persistent
            )
            .unwrap()
            .as_str(),
            "allow"
        );
        // A review that offers only the harness's persistent choice is that
        // choice, not a decline, and it does not grow a project rule: there
        // is no once option to answer the current review with.
        let only_persistent = json!({"options":[
            {"optionId":"always", "kind":"allow_always", "name":"Always"}
        ]});
        let passed = admit(&only_persistent, &once, PersistentKinds::Keep, true).unwrap();
        assert_eq!(passed.options.choices().len(), 1);
        assert_eq!(passed.options.choices()[0].id().as_str(), "always");
        assert!(
            once_option(&passed.options, PermissionEffect::Allow, &passed.persistent).is_none()
        );
    }

    #[test]
    fn a_provider_option_cannot_use_a_remember_id() {
        let reserved = json!({"options":[
            {"optionId":"allow", "kind":"allow_once", "name":"Allow"},
            {"optionId":"nessa.remember.allow", "kind":"allow_once", "name":"Always allow in this project"}
        ]});
        let once = PermissionOfferPolicy::once_only();
        assert!(admit(&reserved, &once, PersistentKinds::Keep, true).is_err());
        assert!(admit(&reserved, &once, PersistentKinds::Drop, false).is_err());
        let deny = json!({"options":[
            {"optionId":"nessa.remember.deny", "kind":"reject_once", "name":"Always deny"}
        ]});
        assert!(admit(&deny, &once, PersistentKinds::Keep, false).is_err());
    }
    // A choice past what a review may carry is refused with the request, which
    // the worker declines at once; a surface is never left holding a review
    // it cannot show while its deadline runs.
    #[test]
    fn choices_a_review_cannot_carry_are_refused_with_the_request() {
        let once = PermissionOfferPolicy::once_only();
        let fits = json!({"options":[
            {"optionId":"allow", "kind":"allow_once", "name":"x".repeat(MAX_REVIEW_OPTION_LABEL_BYTES)}
        ]});
        assert!(admit(&fits, &once, PersistentKinds::Drop, false).is_ok());
        let long = json!({"options":[
            {"optionId":"allow", "kind":"allow_once", "name":"x".repeat(MAX_REVIEW_OPTION_LABEL_BYTES + 1)}
        ]});
        assert!(admit(&long, &once, PersistentKinds::Drop, false).is_err());
        let many = |count: usize| {
            json!({"options": (0..count)
                .map(|index| json!({"optionId": format!("allow-{index}"), "kind": "allow_once", "name": "Allow"}))
                .collect::<Vec<_>>()})
        };
        assert!(admit(
            &many(MAX_REVIEW_OPTIONS),
            &once,
            PersistentKinds::Drop,
            false
        )
        .is_ok());
        assert!(admit(
            &many(MAX_REVIEW_OPTIONS + 1),
            &once,
            PersistentKinds::Drop,
            false
        )
        .is_err());
    }
}
