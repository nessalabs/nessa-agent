//! Codex's selections are checked for the right thing at the right moment, and
//! an ambiguous configuration identity fails before any value is trusted.
use super::verify_config;
use crate::application::agent_execution::agents::AgentError;
use serde_json::{json, Value};

const MODE: &str = "read-only";

fn options(model: Value, mode: Value) -> Value {
    json!({"configOptions":[model, mode]})
}

fn model_option(current: &str, offered: &[&str]) -> Value {
    json!({
        "id": "model",
        "currentValue": current,
        "options": offered.iter().map(|value| json!({"value": value})).collect::<Vec<_>>(),
    })
}

#[test]
fn duplicate_policy_selectors_fail_in_every_order_before_value_checks() {
    for mode in [None, Some(MODE)] {
        for id in ["model", "mode"] {
            for duplicate in [
                json!(MODE),
                json!("agent-full-access"),
                json!("exact"),
                json!("other-model"),
                Value::Null,
            ] {
                for reversed in [false, true] {
                    let mut options = vec![
                        model_option("exact", &["exact"]),
                        json!({"id":"mode", "currentValue":MODE}),
                        json!({"id":id, "currentValue":duplicate}),
                    ];
                    if reversed {
                        options.reverse();
                    }
                    assert_eq!(
                        verify_config(&json!({"configOptions":options}), "exact", mode),
                        Err(AgentError::Protocol(
                            "duplicate model or mode config option".into()
                        )),
                        "{id}, mode={mode:?}, reversed={reversed}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_new_session_is_checked_for_a_model_codex_would_accept_not_one_it_has_selected() {
    // Codex opens the session on its own model and takes this binding's model
    // afterwards. Requiring the selection here would fail every healthy start.
    let offered = options(
        model_option("codex-default", &["codex-default", "exact"]),
        json!({}),
    );
    assert_eq!(verify_config(&offered, "exact", None), Ok(()));
    assert_eq!(
        verify_config(&offered, "absent", None),
        Err(AgentError::Protocol(
            "provider does not offer the configured model".into()
        ))
    );
    // A model served by a custom provider is not in the enumerated catalog. The
    // session already being open on it is the provider accepting it.
    assert_eq!(
        verify_config(
            &options(model_option("exact", &["codex-default"]), json!({})),
            "exact",
            None
        ),
        Ok(())
    );
}

#[test]
fn a_configured_session_must_read_back_both_selections_exactly() {
    assert_eq!(
        verify_config(
            &options(
                model_option("exact", &["exact"]),
                json!({"id":"mode","currentValue":MODE})
            ),
            "exact",
            Some(MODE)
        ),
        Ok(())
    );
    for (result, expected) in [
        (
            options(
                model_option("codex-default", &["codex-default", "exact"]),
                json!({"id":"mode","currentValue":MODE}),
            ),
            "provider did not select the exact configured model",
        ),
        (
            options(
                model_option("exact", &["exact"]),
                json!({"id":"mode","currentValue":"agent-full-access"}),
            ),
            "provider is not in the configured approval mode",
        ),
        (
            json!({"configOptions":[model_option("exact", &["exact"])]}),
            "missing mode config option",
        ),
    ] {
        assert_eq!(
            verify_config(&result, "exact", Some(MODE)),
            Err(AgentError::Protocol(expected.into()))
        );
    }
}

#[test]
fn a_response_without_readable_options_is_never_read_as_agreement() {
    for result in [
        json!({}),
        json!({"configOptions":"none"}),
        json!({"configOptions":[]}),
        json!({"configOptions":[{"id":"mode","currentValue":MODE}]}),
        // Present, but with nothing that says which models it would take.
        json!({"configOptions":[{"id":"model","currentValue":"codex-default"}]}),
    ] {
        assert!(verify_config(&result, "exact", None).is_err(), "{result}");
        assert!(
            verify_config(&result, "exact", Some(MODE)).is_err(),
            "{result}"
        );
    }
}
