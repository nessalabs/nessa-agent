//! Restored asks keep what an answer depends on, refuse histories that could
//! not have happened, and are bounded while decoding by the same limits the
//! domain enforces live.
use super::*;
use nessa_sdk::domain::agent_execution::questions::{
    AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId, MAX_KEY_BYTES, MAX_OPTIONS,
    MAX_QUESTIONS, MAX_TEXT_BYTES,
};
use serde_json::Value;

fn asked(question: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new("execution").unwrap(),
        ExecutionUpdate::QuestionAsked {
            id: QuestionId::new(question).unwrap(),
            question: AgentQuestion::new(
                "Which environment?",
                vec![Question::new(
                    "question_0",
                    "Which environment?",
                    None,
                    AnswerShape::Many,
                    vec![AnswerOption::new("staging", "Staging", None).unwrap()],
                    Some("question_0_custom".into()),
                    true,
                )
                .unwrap()],
            )
            .unwrap(),
        },
    )
}
fn question_closed(question: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new("execution").unwrap(),
        ExecutionUpdate::QuestionClosed {
            id: QuestionId::new(question).unwrap(),
        },
    )
}

#[tokio::test]
async fn a_saved_ask_keeps_everything_an_answer_depends_on() {
    // A restored ask that lost its companion field or its `required` flag would
    // send prose to a field the schema does not have, or quietly let a required
    // question be skipped.
    let storage = InMemoryStorage::new();
    let store = storage.open(id("asked")).await.unwrap();
    let mut saved = snapshot("asked");
    saved.invocations[0].events.push(asked("1"));
    saved.invocations[0].events.push(question_closed("1"));
    store.save(saved.clone()).await.unwrap();
    assert_same(&store.load().await.unwrap().unwrap(), &saved);
    let ExecutionUpdate::QuestionAsked { question, .. } =
        store.load().await.unwrap().unwrap().invocations[0].events[1]
            .update()
            .clone()
    else {
        panic!("expected the saved ask");
    };
    let restored = &question.questions()[0];
    assert_eq!(restored.free_text_key(), Some("question_0_custom"));
    assert!(restored.required());
    assert_eq!(restored.shape(), AnswerShape::Many);
}

#[tokio::test]
async fn an_ask_history_that_could_not_have_happened_is_refused() {
    // Fields that are each valid do not make a sequence that could have
    // happened. The review found these all restoring.
    for (case, events) in [
        ("a closure with no ask", vec![question_closed("1")]),
        (
            "a closure twice",
            vec![asked("1"), question_closed("1"), question_closed("1")],
        ),
        ("one identity asked twice", vec![asked("1"), asked("1")]),
    ] {
        let storage = InMemoryStorage::new();
        let store = storage.open(id("history")).await.unwrap();
        let original = snapshot("history");
        store.save(original.clone()).await.unwrap();
        let mut invalid = original.clone();
        invalid.invocations[0].events.extend(events);
        assert!(
            matches!(store.save(invalid).await, Err(StorageError::Corrupt(_))),
            "{case} was accepted"
        );
        assert_same(&store.load().await.unwrap().unwrap(), &original);
    }
}

#[tokio::test]
async fn a_saved_ask_beyond_the_published_bounds_is_refused_while_decoding() {
    // Each bound is the one the domain publishes, so an ask the agent could
    // not have made live cannot come back from disk either — and the decoder
    // refuses it before building the oversized value.
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    private::create_directory(&directory).unwrap();
    let storage = LocalFileStorage::new(directory.clone()).unwrap();
    let mut value = snapshot("bounded-ask");
    value.invocations[0].events.push(asked("1"));
    let at = value.invocations[0].events.len() - 1;
    let lease = storage.open(value.id.clone()).await.unwrap();
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&directory, "bounded-ask");
    let original: Value = snapshot_json(&std::fs::read(&path).unwrap()).unwrap();
    let ask = format!("/invocations/0/events/{at}/update/QuestionAsked");
    let string_limit = "journal field exceeds decoded string limit";
    let collection_limit = "journal collection exceeds decoding limit";
    let question = original.pointer(&format!("{ask}/questions/0")).unwrap();
    let cases: [(&str, &str, Value, &str); 5] = [
        ("key", "/questions/0/key", "k".repeat(MAX_KEY_BYTES + 1).into(), string_limit),
        (
            "companion key",
            "/questions/0/free_text_key",
            "k".repeat(MAX_KEY_BYTES + 1).into(),
            string_limit,
        ),
        (
            "option label",
            "/questions/0/options/0/label",
            "l".repeat(MAX_TEXT_BYTES + 1).into(),
            string_limit,
        ),
        (
            "options",
            "/questions/0/options",
            Value::Array(
                (0..=MAX_OPTIONS)
                    .map(|index| {
                        serde_json::json!({"value": format!("v{index}"), "label": "l", "description": null})
                    })
                    .collect(),
            ),
            collection_limit,
        ),
        (
            "questions",
            "/questions",
            Value::Array(vec![question.clone(); MAX_QUESTIONS + 1]),
            collection_limit,
        ),
    ];
    for (case, field, replacement, expected) in cases {
        let mut invalid = original.clone();
        *invalid
            .pointer_mut(&format!("{ask}{field}"))
            .unwrap_or_else(|| panic!("saved ask has no {case}")) = replacement;
        let bytes = journal_bytes(&invalid).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        match lease.load().await {
            Err(StorageError::Corrupt(message)) => {
                assert!(message.contains(expected), "{case}: {message}")
            }
            other => panic!("{case} restored: {other:?}"),
        }
    }
}
