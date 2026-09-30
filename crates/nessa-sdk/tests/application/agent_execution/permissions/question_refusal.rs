//! Public refusal evidence derives accounting and retains its original comparison.
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::permissions::{
    PermissionAnswerDelivery, QuestionRefusalRecord, RefusedAsk,
};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    questions::{
        AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId, QuestionRefusalReason,
        MAX_OPEN_QUESTIONS,
    },
    sessions::ExecutionSessionId,
    ExecutionError,
};

fn minimal_ask() -> AgentQuestion {
    AgentQuestion::new(
        "m",
        vec![Question::new(
            "k",
            "p",
            None,
            AnswerShape::One,
            vec![AnswerOption::new("v", "l", None).unwrap()],
            None,
            false,
        )
        .unwrap()],
    )
    .unwrap()
}

#[test]
fn public_refusal_uses_actual_minimal_open_asks_and_preserves_delivery_correlation() {
    let ask = minimal_ask();
    let open = vec![ask.clone(); MAX_OPEN_QUESTIONS];
    let refused = RefusedAsk::new(ask.clone(), &open).unwrap();
    assert_eq!(refused.open_asks(), open.len());
    assert_eq!(
        refused.open_cost(),
        open.iter().map(AgentQuestion::carrying_cost).sum::<usize>()
    );
    assert_eq!(refused.ask(), &ask);
    assert_eq!(refused.reason(), Some(QuestionRefusalReason::TooManyOpen));
    let selected = QuestionRefusalRecord::for_room(
        ExecutionSessionId::new("session").unwrap(),
        ExecutionId::new("run").unwrap(),
        QuestionId::new("refused").unwrap(),
        refused,
        PermissionAnswerDelivery::Selected,
    )
    .unwrap();
    for delivery in [
        PermissionAnswerDelivery::Written,
        PermissionAnswerDelivery::Failed(AgentError::Transport("failed".into())),
    ] {
        let later = selected.clone().with_delivery(delivery.clone());
        assert_eq!(later.session_id(), selected.session_id());
        assert_eq!(later.execution_id(), selected.execution_id());
        assert_eq!(later.id(), selected.id());
        assert_eq!(later.reason(), selected.reason());
        assert_eq!(later.refused(), selected.refused());
        assert_eq!(later.delivery(), &delivery);
    }
    assert_eq!(selected.delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(
        RefusedAsk::new(ask.clone(), open.iter().chain([&ask])).unwrap_err(),
        ExecutionError::InvalidQuestionRefusal
    );
    let candidate = RefusedAsk::new(ask.clone(), []).unwrap();
    assert_eq!(candidate.open_asks(), 0);
    assert_eq!(candidate.open_cost(), 0);
    assert_eq!(candidate.reason(), None);
    assert_eq!(candidate.clone().into_ask(), ask);
    assert_eq!(
        QuestionRefusalRecord::for_room(
            selected.session_id().clone(),
            selected.execution_id().clone(),
            selected.id().clone(),
            candidate,
            PermissionAnswerDelivery::Selected,
        )
        .unwrap_err(),
        ExecutionError::InvalidQuestionRefusal
    );
}
