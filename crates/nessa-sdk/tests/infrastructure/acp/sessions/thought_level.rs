//! The reasoning effort option is found by its ACP category whatever the agent
//! calls it, read to a bounded list, and narrows the catalogue's levels.
use super::{offered, thought_level, ThoughtLevel, MAX_CHOICES};
use crate::domain::common::value_objects::{Date, TokenLimits, Url};
use crate::domain::effective_capabilities::value_objects::{
    BindingRestrictions, EffectiveCapabilities,
};
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::domain::model_metadata::value_objects::{
    EffortLevel, EffortLevels, Modalities, ModelDescription, ModelFeatures, ModelKey, ModelProvider,
};
use serde_json::{json, Value};

fn select(id: &str, category: &str, current: &str, values: &[&str]) -> Value {
    json!({
        "id": id, "name": id, "category": category, "type": "select",
        "currentValue": current,
        "options": values.iter().map(|value| json!({"value": value, "name": value})).collect::<Vec<_>>(),
    })
}

/// claude-agent-acp on Opus 5, as probed on 2026-09-29.
fn claude() -> Value {
    json!({"configOptions": [
        select("mode", "mode", "default", &["default", "plan"]),
        select("model", "model", "opus", &["opus", "sonnet"]),
        select("effort", "thought_level", "default",
            &["default", "low", "medium", "high", "xhigh", "max"]),
    ]})
}

/// codex-acp 1.12 on gpt-5.6-sol, as probed on 2026-09-29.
fn codex() -> Value {
    json!({"configOptions": [
        select("mode", "mode", "agent", &["read-only", "agent"]),
        select("model", "model", "gpt-5.6-sol", &["gpt-5.6-sol"]),
        select("reasoning_effort", "thought_level", "medium",
            &["low", "medium", "high", "xhigh", "max", "ultra"]),
    ]})
}

fn capabilities(levels: Option<&[&str]>, binding_reasons: bool) -> EffectiveCapabilities {
    let text = Modalities::new(true, false, false).unwrap();
    let limits = TokenLimits::new(1000, 100).unwrap();
    let model = ModelMetadata::new(
        ModelKey::new(ModelProvider::OpenAi, "gpt-5.6-sol".into()).unwrap(),
        ModelDescription::new(
            "Test".into(),
            Date::new("2026-01".into()).unwrap(),
            Url::new("https://example.com").unwrap(),
        )
        .unwrap(),
        ModelFeatures::new(text, text, true, true, false),
        limits,
    );
    let model = match levels {
        Some(names) => model
            .with_effort_levels(
                EffortLevels::new(
                    names
                        .iter()
                        .map(|name| EffortLevel::new((*name).into()).unwrap())
                        .collect(),
                )
                .unwrap(),
            )
            .unwrap(),
        None => model,
    };
    let binding = BindingRestrictions::new(
        ModelFeatures::new(text, text, true, binding_reasons, false),
        limits,
    );
    EffectiveCapabilities::new(&model, binding, limits).unwrap()
}

fn narrowed(result: &Value, capabilities: &EffectiveCapabilities) -> Option<Vec<String>> {
    let offered = offered(result, capabilities).unwrap();
    capabilities
        .effort_levels()
        .and_then(|levels| levels.restricted_to(offered))
        .map(|levels| {
            levels
                .levels()
                .iter()
                .map(|level| level.as_str().to_owned())
                .collect()
        })
}

#[test]
fn finds_the_option_by_category_whatever_the_agent_calls_it() {
    let claude = claude();
    let found = thought_level(&claude).unwrap().unwrap();
    assert_eq!(
        found,
        ThoughtLevel {
            id: "effort",
            current: Some("default"),
            choices: vec!["default", "low", "medium", "high", "xhigh", "max"],
        }
    );
    let codex = codex();
    let found = thought_level(&codex).unwrap().unwrap();
    assert_eq!(
        (found.id, found.current),
        ("reasoning_effort", Some("medium"))
    );
}

#[test]
fn narrows_to_the_catalogue_levels_the_agent_also_offers() {
    let gpt = capabilities(
        Some(&["none", "low", "medium", "high", "xhigh", "max"]),
        true,
    );
    // No `none` from the agent, and its `ultra` is not a catalogue level.
    assert_eq!(
        narrowed(&codex(), &gpt).unwrap(),
        ["low", "medium", "high", "xhigh", "max"]
    );
    let opus = capabilities(Some(&["low", "medium", "high", "xhigh", "max"]), true);
    // Claude's own `default` is not a level either.
    assert_eq!(
        narrowed(&claude(), &opus).unwrap(),
        ["low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn offers_nothing_without_an_option_catalogue_levels_or_binding_reasoning() {
    // claude-agent-acp on Haiku 4.5 advertises no effort option.
    let haiku = json!({"configOptions": [select("model", "model", "haiku", &["haiku"])]});
    assert_eq!(thought_level(&haiku).unwrap(), None);
    let levels: &[&str] = &["low", "high"];
    assert_eq!(narrowed(&haiku, &capabilities(Some(levels), true)), None);
    // The catalogue records no levels for the model.
    assert!(offered(&claude(), &capabilities(None, true))
        .unwrap()
        .is_empty());
    // The binding cannot run the model's reasoning: no catalogue levels survive.
    assert!(offered(&claude(), &capabilities(Some(levels), false))
        .unwrap()
        .is_empty());
    // Nothing the agent advertises is a catalogue level.
    let other = json!({"configOptions": [select("effort", "thought_level", "a", &["a", "b"])]});
    assert!(offered(&other, &capabilities(Some(levels), true))
        .unwrap()
        .is_empty());
}

#[test]
fn reads_choices_grouped_as_acp_allows() {
    let grouped = json!({"configOptions": [{
        "id": "effort", "category": "thought_level", "currentValue": "low",
        "options": [
            {"group": "fast", "name": "Fast", "options": [{"value": "low"}]},
            {"group": "deep", "name": "Deep", "options": [{"value": "high"}, {"value": "max"}]},
        ],
    }]});
    assert_eq!(
        thought_level(&grouped).unwrap().unwrap().choices,
        ["low", "high", "max"]
    );
}

#[test]
fn refuses_an_ambiguous_or_malformed_option() {
    let twice = json!({"configOptions": [
        select("effort", "thought_level", "low", &["low"]),
        select("reasoning_effort", "thought_level", "low", &["low"]),
    ]});
    let without_id = json!({"configOptions": [{"category": "thought_level", "options": []}]});
    let without_choices = json!({"configOptions": [{"id": "effort", "category": "thought_level"}]});
    let numeric = json!({"configOptions": [{"id": "effort", "category": "thought_level", "options": [{"value": 1}]}]});
    let too_many: Vec<String> = (0..=MAX_CHOICES)
        .map(|index| format!("level-{index}"))
        .collect();
    let too_many = json!({"configOptions": [select(
        "effort",
        "thought_level",
        "level-0",
        &too_many.iter().map(String::as_str).collect::<Vec<_>>(),
    )]});
    for (label, result) in [
        ("missing options", json!({})),
        ("twice", twice),
        ("without id", without_id),
        ("without choices", without_choices),
        ("numeric", numeric),
        ("too many", too_many),
    ] {
        assert!(
            matches!(
                thought_level(&result),
                Err(crate::application::agent_execution::agents::AgentError::Protocol(_))
            ),
            "{label}"
        );
    }
    // Exactly the bound is read.
    let most: Vec<String> = (0..MAX_CHOICES)
        .map(|index| format!("level-{index}"))
        .collect();
    let most = json!({"configOptions": [select(
        "effort",
        "thought_level",
        "level-0",
        &most.iter().map(String::as_str).collect::<Vec<_>>(),
    )]});
    assert_eq!(
        thought_level(&most).unwrap().unwrap().choices.len(),
        MAX_CHOICES
    );
}
