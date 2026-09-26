//! The schema an agent sends is the only description of what it wants, so what
//! this reads out of it is checked against the shape the pinned harnesses emit.
use super::*;
use crate::domain::agent_execution::questions::{AcceptedAnswer, QuestionChoice};
use crate::domain::agent_execution::ExecutionError;
use crate::infrastructure::json_rpc::RpcId;
use serde_json::json;

fn single_question() -> Value {
    json!({
        "mode":"form",
        "sessionId":"session",
        "message":"Which environment should I deploy to?",
        "requestedSchema":{"type":"object","properties":{
            "question_0":{"type":"string","title":"Environment","oneOf":[
                {"const":"staging","title":"Staging","description":"Safe to break"},
                {"const":"production","title":"Production"}
            ]},
            "question_0_custom":{"type":"string","title":"Other","description":"Type your own",
                "_meta":{"_askUserQuestionCustomAnswer":{"questionId":"question_0","isCustomAnswer":true}}}
        }}
    })
}

#[test]
fn a_single_question_takes_its_prompt_from_the_ask_and_keeps_its_options() {
    let ask = question(&single_question()).unwrap();
    assert_eq!(ask.message(), "Which environment should I deploy to?");
    assert_eq!(ask.questions().len(), 1, "the companion is not a question");
    let asked = &ask.questions()[0];
    assert_eq!(asked.key(), "question_0");
    // With one question the field carries no description, so the ask's own
    // message is the prompt rather than the field being left unexplained.
    assert_eq!(asked.prompt(), "Which environment should I deploy to?");
    assert_eq!(asked.header(), Some("Environment"));
    assert_eq!(asked.shape(), AnswerShape::One);
    assert!(asked.free_text(), "its companion offers words of their own");
    assert_eq!(asked.options()[0].value(), "staging");
    assert_eq!(asked.options()[0].label(), "Staging");
    assert_eq!(asked.options()[0].description(), Some("Safe to break"));
    // An option with no title reads as its own value rather than as nothing.
    assert_eq!(asked.options()[1].label(), "Production");
    assert_eq!(asked.options()[1].description(), None);
}

#[test]
fn several_questions_each_carry_their_own_text_and_shape() {
    let ask = question(&json!({
        "mode":"form","sessionId":"session","message":"Please answer the following questions.",
        "requestedSchema":{"type":"object","properties":{
            "question_0":{"type":"string","description":"Which environment?","oneOf":[{"const":"staging"}]},
            "question_1":{"type":"array","description":"Which checks?","items":{"anyOf":[
                {"const":"tests"},{"const":"lint"}
            ]}}
        }}
    }))
    .unwrap();
    let by_key = |key: &str| {
        ask.questions()
            .iter()
            .find(|question| question.key() == key)
            .expect("question present")
            .clone()
    };
    assert_eq!(by_key("question_0").prompt(), "Which environment?");
    assert_eq!(by_key("question_0").shape(), AnswerShape::One);
    assert!(
        !by_key("question_0").free_text(),
        "no companion was offered"
    );
    assert_eq!(by_key("question_1").prompt(), "Which checks?");
    assert_eq!(by_key("question_1").shape(), AnswerShape::Many);
    assert_eq!(by_key("question_1").options().len(), 2);
}

#[test]
fn what_cannot_be_answered_here_is_refused_rather_than_flattened() {
    // A page to visit is not a question with answers.
    assert!(matches!(
        question(
            &json!({"mode":"url","sessionId":"s","message":"Sign in","url":"https://example.com"})
        ),
        Err(AgentError::Unsupported(_))
    ));
    // A field offering nothing to choose, and a schema with no fields at all.
    assert!(question(&json!({
        "mode":"form","sessionId":"s","message":"m",
        "requestedSchema":{"type":"object","properties":{"question_0":{"type":"string"}}}
    }))
    .is_err());
    assert!(question(&json!({"mode":"form","sessionId":"s","message":"m"})).is_err());
    // An option with no value cannot be sent back as an answer.
    assert!(question(&json!({
        "mode":"form","sessionId":"s","message":"m",
        "requestedSchema":{"type":"object","properties":{
            "question_0":{"type":"string","oneOf":[{"title":"No value"}]}}}
    }))
    .is_err());
}

/// Three questions whose answers are each shaped by what was asked, not by
/// what was chosen: a list, a single choice, and prose under a name the asker
/// picked rather than one this binding would have guessed.
fn shaped_ask() -> AgentQuestion {
    question(&json!({
        "mode":"form","sessionId":"session","message":"Please answer the following questions.",
        "requestedSchema":{"type":"object","required":["environment"],"properties":{
            "checks":{"type":"array","description":"Which checks?","items":{"anyOf":[
                {"const":"tests"},{"const":"lint"}
            ]}},
            "environment":{"type":"string","description":"Which environment?","oneOf":[
                {"const":"staging"},{"const":"production"}
            ]},
            "explanation":{"type":"string","title":"Other",
                "_meta":{"_askUserQuestionCustomAnswer":{"questionId":"environment","isCustomAnswer":true}}}
        }}
    }))
    .unwrap()
}

#[test]
fn an_answer_is_shaped_by_the_question_it_answers_not_by_how_much_was_chosen() {
    // The regression the review reproduced: a list answered with one option was
    // written as a string, and prose went to `<key>_custom` — a field this
    // schema does not have.
    let asked = shaped_ask();
    let environment = asked
        .questions()
        .iter()
        .find(|question| question.key() == "environment")
        .unwrap();
    assert_eq!(environment.free_text_key(), Some("explanation"));
    assert!(environment.required());
    let checks = asked
        .questions()
        .iter()
        .find(|question| question.key() == "checks")
        .unwrap();
    assert!(!checks.required());
    assert_eq!(checks.free_text_key(), None);

    let answer = AcceptedAnswer::new(
        &asked,
        vec![
            QuestionChoice::new("checks", vec!["tests".into()], None).unwrap(),
            QuestionChoice::new(
                "environment",
                vec!["staging".into()],
                Some("only the eu region".into()),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let id = RpcId::Text("ask".into());
    let value = accepted(&id, &asked, &answer);
    let content = &value["result"]["content"];
    assert_eq!(value["result"]["action"], "accept");
    assert_eq!(
        content["checks"],
        json!(["tests"]),
        "one choice of a list is still a list"
    );
    assert_eq!(content["environment"], json!("staging"));
    assert_eq!(content["explanation"], json!("only the eu region"));
    assert!(
        content.get("environment_custom").is_none(),
        "no invented field names"
    );

    assert_eq!(declined(&id)["result"]["action"], "decline");
    assert_eq!(cancelled(&id)["result"]["action"], "cancel");
}

#[test]
fn a_required_question_cannot_be_skipped_and_an_optional_one_can() {
    let asked = shaped_ask();
    // Skipping the required one is refused before anything is written, so the
    // agent never receives content its own schema rejects.
    assert_eq!(
        AcceptedAnswer::new(
            &asked,
            vec![QuestionChoice::new("checks", vec!["lint".into()], None).unwrap()],
        )
        .unwrap_err(),
        ExecutionError::UnansweredQuestion
    );
    // Words of the answerer's own are an answer to it, and the optional
    // question may be left alone.
    assert!(AcceptedAnswer::new(
        &asked,
        vec![QuestionChoice::new("environment", vec![], Some("dev box".into())).unwrap()],
    )
    .is_ok());
}
