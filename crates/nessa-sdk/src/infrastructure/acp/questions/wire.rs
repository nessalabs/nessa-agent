//! Reading an agent's question off the wire, and writing back the answer.
//!
//! ACP carries a question as a *form elicitation*: a JSON Schema whose
//! properties are the fields to fill in. The agent-side tool that produces it
//! (`AskUserQuestion` and its equivalents) is not part of this: what arrives is
//! a schema, and what goes back is the content for it.
use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::questions::{
    AgentQuestion, AnswerOption, AnswerShape, Question,
};
use crate::infrastructure::acp::fields::string;
use crate::infrastructure::json_rpc::protocol;
use serde_json::{json, Map, Value};

/// The `_meta` key marking a free-text field as one question's "other" box.
///
/// Deliberately un-namespaced by the agents that emit it, so the same marker
/// means the same thing whichever harness bridged the question. Reading it is
/// how a companion field is told apart from a question of its own.
const CUSTOM_ANSWER_META: &str = "_askUserQuestionCustomAnswer";

/// Read the question an agent is asking out of its form elicitation.
///
/// Only `mode: "form"` is a question. A URL-mode elicitation asks somebody to
/// visit a page, which is a different thing this binding does not offer, and it
/// is refused here rather than flattened into a question with no answers.
pub(crate) fn question(params: &Value) -> Result<AgentQuestion, AgentError> {
    if string(params, "mode")? != "form" {
        return Err(AgentError::Unsupported(
            "only a form question can be answered here".into(),
        ));
    }
    let message = string(params, "message")?;
    let properties = params
        .pointer("/requestedSchema/properties")
        .and_then(Value::as_object)
        .ok_or_else(|| protocol("question has no fields"))?;

    // A free-text companion belongs to the question it names, so the companions
    // are collected first and then attached; field order in an object is not
    // something to depend on.
    let mut free_text = Vec::new();
    for (key, field) in properties {
        if let Some(owner) = custom_answer_for(field) {
            free_text.push(owner.unwrap_or(key.as_str()).to_owned());
        }
    }

    let mut questions = Vec::new();
    for (key, field) in properties {
        if custom_answer_for(field).is_some() {
            continue;
        }
        let (shape, options) = shape_and_options(field)?;
        // With one question the prompt is the ask's own message; with several,
        // each field carries its own text. Falling back to the message keeps a
        // question that describes itself either way.
        let prompt = field
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(message);
        let header = field
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned);
        questions.push(
            Question::new(
                key.as_str(),
                prompt,
                header,
                shape,
                options,
                free_text.iter().any(|owner| owner == key),
            )
            .map_err(|error| protocol(&error.to_string()))?,
        );
    }
    AgentQuestion::new(message, questions).map_err(|error| protocol(&error.to_string()))
}

/// The question a free-text field answers for, when the field is one.
///
/// Returns the named owner where the marker gives one, and `None` inside the
/// `Some` when it does not — a companion that does not say what it belongs to
/// is still a companion, and is attached to its own key rather than promoted to
/// a question with no options.
fn custom_answer_for(field: &Value) -> Option<Option<&str>> {
    let marker = field.pointer(&format!("/_meta/{CUSTOM_ANSWER_META}"))?;
    marker
        .get("isCustomAnswer")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        .then(|| marker.get("questionId").and_then(Value::as_str))
}

/// How many answers a field takes, and what it offers.
fn shape_and_options(field: &Value) -> Result<(AnswerShape, Vec<AnswerOption>), AgentError> {
    if let Some(options) = field.get("oneOf").and_then(Value::as_array) {
        return Ok((AnswerShape::One, answer_options(options)?));
    }
    if let Some(options) = field.pointer("/items/anyOf").and_then(Value::as_array) {
        return Ok((AnswerShape::Many, answer_options(options)?));
    }
    Err(protocol("question field offers no answers"))
}

/// Read the offered answers, keeping what is recorded apart from what is read.
fn answer_options(options: &[Value]) -> Result<Vec<AnswerOption>, AgentError> {
    options
        .iter()
        .map(|option| {
            let value = option
                .get("const")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol("answer option has no value"))?;
            let label = option.get("title").and_then(Value::as_str).unwrap_or(value);
            let description = option
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned);
            AnswerOption::new(value, label, description)
                .map_err(|error| protocol(&error.to_string()))
        })
        .collect()
}

/// The answer to send back, as the content the agent's own schema asked for.
///
/// `chosen` pairs a question's key with what was chosen for it: the option
/// values, and the answerer's own words where they typed any. A question left
/// alone contributes nothing, which is how skipping is expressed — the schema
/// requires no field.
pub(crate) fn accepted(
    id: &crate::infrastructure::json_rpc::RpcId,
    chosen: &[(String, Vec<String>, Option<String>)],
) -> Value {
    let mut content = Map::new();
    for (key, values, own_words) in chosen {
        match values.as_slice() {
            [] => {}
            [single] => {
                content.insert(key.clone(), json!(single));
            }
            many => {
                content.insert(key.clone(), json!(many));
            }
        }
        if let Some(words) = own_words {
            content.insert(format!("{key}_custom"), json!(words));
        }
    }
    crate::infrastructure::json_rpc::success(id, json!({"action":"accept","content":content}))
}

/// The answer that declines to answer: asked, and answered with nothing.
pub(crate) fn declined(id: &crate::infrastructure::json_rpc::RpcId) -> Value {
    crate::infrastructure::json_rpc::success(id, json!({"action":"decline"}))
}

/// The answer that no longer applies, because the question was withdrawn or the
/// work it belonged to is over.
pub(crate) fn cancelled(id: &crate::infrastructure::json_rpc::RpcId) -> Value {
    crate::infrastructure::json_rpc::success(id, json!({"action":"cancel"}))
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/questions/wire.rs"]
mod tests;
