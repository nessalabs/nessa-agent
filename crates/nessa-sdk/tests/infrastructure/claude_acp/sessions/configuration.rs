//! Ambiguous configuration identities fail before any selected value is trusted.
use super::verify_config;
use crate::application::agent_execution::agents::AgentError;
use serde_json::{json, Value};

#[test]
fn duplicate_policy_selectors_fail_in_every_order_before_value_checks() {
    for require_mode in [false, true] {
        for id in ["model", "mode"] {
            for duplicate in [
                json!("default"),
                json!("exact"),
                json!("bypassPermissions"),
                json!("other-model"),
                Value::Null,
            ] {
                for reversed in [false, true] {
                    let mut options = vec![
                        json!({"id":"model", "currentValue":"exact"}),
                        json!({"id":"mode", "currentValue":"default"}),
                        json!({"id":id, "currentValue":duplicate}),
                    ];
                    if reversed {
                        options.reverse();
                    }
                    assert_eq!(
                        verify_config(&json!({"configOptions":options}), "exact", require_mode),
                        Err(AgentError::Protocol(
                            "duplicate model or mode config option".into()
                        )),
                        "{id}, require_mode={require_mode}, reversed={reversed}"
                    );
                }
            }
        }
    }
}

#[test]
fn unique_configuration_keeps_initial_and_configured_mode_requirements() {
    let model = json!({"id":"model", "currentValue":"exact"});
    assert_eq!(
        verify_config(&json!({"configOptions":[model.clone()]}), "exact", false),
        Ok(())
    );
    assert!(verify_config(&json!({"configOptions":[model.clone()]}), "exact", true).is_err());
    assert_eq!(
        verify_config(
            &json!({"configOptions":[model, {"id":"mode", "currentValue":"default"}]}),
            "exact",
            true
        ),
        Ok(())
    );
}
