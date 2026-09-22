//! An ask is held for as long as a turn waits for somebody to answer it, so
//! what it may contain is decided here rather than by whoever sent it.
use super::*;

fn option(value: &str) -> AnswerOption {
    AnswerOption::new(value, value, None).unwrap()
}

fn question(key: &str) -> Question {
    Question::new(
        key,
        "Which environment?",
        Some("Environment".into()),
        AnswerShape::One,
        vec![option("staging"), option("production")],
        false,
    )
    .unwrap()
}

#[test]
fn an_ask_keeps_its_framing_its_questions_and_their_order() {
    let ask = AgentQuestion::new(
        "I need one thing before I continue.",
        vec![question("question_0"), question("question_1")],
    )
    .unwrap();
    assert_eq!(ask.message(), "I need one thing before I continue.");
    assert_eq!(
        ask.questions()
            .iter()
            .map(Question::key)
            .collect::<Vec<_>>(),
        ["question_0", "question_1"]
    );
    let first = &ask.questions()[0];
    assert_eq!(first.prompt(), "Which environment?");
    assert_eq!(first.header(), Some("Environment"));
    assert_eq!(first.shape(), AnswerShape::One);
    assert!(!first.free_text());
    assert_eq!(
        first
            .options()
            .iter()
            .map(AnswerOption::value)
            .collect::<Vec<_>>(),
        ["staging", "production"]
    );
}

#[test]
fn an_option_keeps_what_is_recorded_apart_from_what_is_read() {
    let chosen = AnswerOption::new("prod-eu", "Production (EU)", Some("Frankfurt".into())).unwrap();
    assert_eq!(chosen.value(), "prod-eu");
    assert_eq!(chosen.label(), "Production (EU)");
    assert_eq!(chosen.description(), Some("Frankfurt"));
    // A description is the only part an agent may leave out.
    assert_eq!(option("staging").description(), None);
}

#[test]
fn an_unanswerable_ask_is_refused_rather_than_left_waiting() {
    // Nothing to put to anybody, in each of the ways an agent can send nothing.
    assert!(AgentQuestion::new("ask", vec![]).is_err());
    assert!(AgentQuestion::new("   ", vec![question("question_0")]).is_err());
    assert!(Question::new(
        "question_0",
        "Which environment?",
        None,
        AnswerShape::One,
        vec![],
        false
    )
    .is_err());
    assert!(Question::new(
        "",
        "prompt",
        None,
        AnswerShape::One,
        vec![option("a")],
        false
    )
    .is_err());
    assert!(Question::new("key", " ", None, AnswerShape::One, vec![option("a")], false).is_err());
    assert!(AnswerOption::new("", "label", None).is_err());
    assert!(AnswerOption::new("value", "\t", None).is_err());
}

#[test]
fn two_questions_cannot_share_a_key() {
    // An answer to either could not be correlated back to the one that wanted it.
    let error = AgentQuestion::new("ask", vec![question("question_0"), question("question_0")])
        .unwrap_err();
    assert_eq!(error, ExecutionError::DuplicateQuestionKey);
}

#[test]
fn every_retained_text_and_collection_is_bounded() {
    let at_limit = "q".repeat(MAX_TEXT_BYTES);
    let over_limit = "q".repeat(MAX_TEXT_BYTES + 1);
    assert!(AnswerOption::new(&at_limit, "label", None).is_ok());
    assert!(AnswerOption::new(&over_limit, "label", None).is_err());
    assert!(AnswerOption::new("value", &at_limit, None).is_ok());
    assert!(AnswerOption::new("value", &over_limit, None).is_err());
    assert!(AnswerOption::new("value", "label", Some(over_limit.clone())).is_err());
    assert!(AgentQuestion::new(&over_limit, vec![question("question_0")]).is_err());

    let options = (0..=MAX_OPTIONS)
        .map(|index| option(&format!("option-{index}")))
        .collect::<Vec<_>>();
    assert_eq!(
        Question::new("key", "prompt", None, AnswerShape::Many, options, true).unwrap_err(),
        ExecutionError::TooManyValues {
            field: "question options",
            max: MAX_OPTIONS
        }
    );
    let questions = (0..=MAX_QUESTIONS)
        .map(|index| question(&format!("question_{index}")))
        .collect::<Vec<_>>();
    assert_eq!(
        AgentQuestion::new("ask", questions).unwrap_err(),
        ExecutionError::TooManyValues {
            field: "questions",
            max: MAX_QUESTIONS
        }
    );
}
