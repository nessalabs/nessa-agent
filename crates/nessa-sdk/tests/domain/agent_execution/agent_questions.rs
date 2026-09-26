//! An ask is held for as long as a turn waits for somebody to answer it, so
//! what it may contain is decided here rather than by whoever sent it.
use super::*;
use crate::domain::agent_execution::questions::{AcceptedAnswer, QuestionChoice};

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
        None,
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
        None,
        false
    )
    .is_err());
    assert!(Question::new(
        "",
        "prompt",
        None,
        AnswerShape::One,
        vec![option("a")],
        None,
        false
    )
    .is_err());
    assert!(Question::new(
        "key",
        " ",
        None,
        AnswerShape::One,
        vec![option("a")],
        None,
        false
    )
    .is_err());
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
        Question::new(
            "key",
            "prompt",
            None,
            AnswerShape::Many,
            options,
            Some("key_custom".into()),
            false
        )
        .unwrap_err(),
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

#[test]
fn a_key_is_bounded_where_every_layer_bounds_it() {
    // The review found keys the domain accepted and the panel then refused,
    // which invalidated the whole conversation view. One bound, everywhere.
    let at_limit = "k".repeat(MAX_KEY_BYTES);
    let over_limit = "k".repeat(MAX_KEY_BYTES + 1);
    let ask = |key: &str| {
        Question::new(
            key,
            "prompt",
            None,
            AnswerShape::One,
            vec![option("a")],
            None,
            false,
        )
    };
    assert!(ask(&at_limit).is_ok());
    assert!(ask(&over_limit).is_err());
    assert!(Question::new(
        "key",
        "prompt",
        None,
        AnswerShape::One,
        vec![option("a")],
        Some(over_limit.clone()),
        false
    )
    .is_err());
    assert!(QuestionChoice::new(over_limit, vec![], None).is_err());
}

#[test]
fn two_options_cannot_record_the_same_value() {
    // A host choosing between them could not say which it meant.
    assert_eq!(
        Question::new(
            "key",
            "prompt",
            None,
            AnswerShape::One,
            vec![
                option("a"),
                AnswerOption::new("a", "Another label", None).unwrap()
            ],
            None,
            false
        )
        .unwrap_err(),
        ExecutionError::DuplicateAnswerOption
    );
}

#[test]
fn a_selection_is_one_the_question_could_have_offered() {
    let many = || {
        AgentQuestion::new(
            "ask",
            vec![Question::new(
                "checks",
                "Which checks?",
                None,
                AnswerShape::Many,
                vec![option("tests"), option("lint")],
                None,
                false,
            )
            .unwrap()],
        )
        .unwrap()
    };
    // Choosing the same option twice, or more options than exist at all, is
    // not a selection this question could have produced. The review
    // reproduced 100,000 copies of one value being accepted.
    assert_eq!(
        QuestionChoice::new("checks", vec!["tests".into(), "tests".into()], None).unwrap_err(),
        ExecutionError::DuplicateAnswerOption
    );
    let too_many = (0..=MAX_OPTIONS)
        .map(|index| format!("value-{index}"))
        .collect();
    assert!(QuestionChoice::new("checks", too_many, None).is_err());
    // A valid multi-select passes, and one choice from a list is still a list.
    assert!(AcceptedAnswer::new(
        &many(),
        vec![QuestionChoice::new("checks", vec!["tests".into(), "lint".into()], None).unwrap()],
    )
    .is_ok());
    // A value the question never offered is refused.
    assert_eq!(
        AcceptedAnswer::new(
            &many(),
            vec![QuestionChoice::new("checks", vec!["deploy".into()], None).unwrap()],
        )
        .unwrap_err(),
        ExecutionError::UnofferedAnswer
    );
}

#[test]
fn a_required_question_is_answered_by_a_choice_and_not_by_words_alone() {
    let required = Question::new(
        "question_0",
        "Which environment?",
        None,
        AnswerShape::One,
        vec![option("staging"), option("production")],
        Some("question_0_custom".into()),
        true,
    )
    .unwrap();
    let ask =
        AgentQuestion::new("Which environment?", vec![required, question("question_1")]).unwrap();
    let answer = |values: Vec<String>, words: Option<&str>| {
        AcceptedAnswer::new(
            &ask,
            vec![QuestionChoice::new("question_0", values, words.map(str::to_owned)).unwrap()],
        )
    };
    // The asker required the choice field; words go to the companion field, so
    // on their own they leave the required one empty.
    assert_eq!(
        answer(vec![], Some("dev box")).unwrap_err(),
        ExecutionError::UnansweredQuestion
    );
    assert_eq!(
        AcceptedAnswer::new(
            &ask,
            vec![QuestionChoice::new("question_1", vec!["staging".into()], None).unwrap()],
        )
        .unwrap_err(),
        ExecutionError::UnansweredQuestion
    );
    assert!(answer(vec!["staging".into()], None).is_ok());
    assert!(answer(vec!["staging".into()], Some("the eu region")).is_ok());
}
