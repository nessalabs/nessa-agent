//! Public adapter admission and answer evidence keep one question correlation.
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{AdmittedQuestion, ExecutionController, ExecutionUpdate},
    permissions::{ActionContext, PermissionAnswerDelivery, QuestionAnswerRecord},
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    questions::{
        AgentQuestion, AnswerOption, AnswerShape, Question, QuestionCancellation, QuestionChoice,
        QuestionId, QuestionResponse,
    },
    sessions::ExecutionSessionId,
    ExecutionError,
};

fn question(value: &str) -> AgentQuestion {
    AgentQuestion::new(
        format!("Choose {value}"),
        vec![Question::new(
            "environment",
            "Environment?",
            None,
            AnswerShape::One,
            vec![AnswerOption::new(value, value, None).unwrap()],
            None,
            true,
        )
        .unwrap()],
    )
    .unwrap()
}
fn choices(value: &str) -> Option<Vec<QuestionChoice>> {
    Some(vec![QuestionChoice::new(
        "environment",
        vec![value.into()],
        None,
    )
    .unwrap()])
}
fn actor() -> ActionContext {
    ActionContext::new("person", "panel", "answer").unwrap()
}
fn assert_correlation(record: &QuestionAnswerRecord, admitted: &AdmittedQuestion) {
    assert_eq!(record.session_id(), admitted.session_id());
    assert_eq!(record.execution_id(), admitted.event().execution_id());
    let ExecutionUpdate::QuestionAsked { id, question } = admitted.event().into_update() else {
        panic!("ask event")
    };
    assert_eq!(record.question_id(), &id);
    assert_eq!(record.question(), &question);
}

#[test]
fn admitted_question_cannot_be_answered_with_another_asks_options() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let first = controller
        .ask_question(
            &execution,
            QuestionId::new("A").unwrap(),
            question("staging"),
        )
        .unwrap();
    let second = controller
        .ask_question(
            &execution,
            QuestionId::new("B").unwrap(),
            question("production"),
        )
        .unwrap();
    assert_eq!(
        QuestionAnswerRecord::chosen(
            first.clone(),
            choices("production"),
            actor(),
            PermissionAnswerDelivery::Selected
        )
        .unwrap_err(),
        ExecutionError::UnofferedAnswer
    );
    let other = QuestionAnswerRecord::chosen(
        second.clone(),
        choices("production"),
        actor(),
        PermissionAnswerDelivery::Selected,
    )
    .unwrap();
    assert_correlation(&other, &second);
    for answer in [choices("staging"), None] {
        let selected = QuestionAnswerRecord::chosen(
            first.clone(),
            answer.clone(),
            actor(),
            PermissionAnswerDelivery::Selected,
        )
        .unwrap();
        assert_eq!(
            matches!(selected.response(), QuestionResponse::Declined),
            answer.is_none()
        );
        for delivery in [
            PermissionAnswerDelivery::Selected,
            PermissionAnswerDelivery::Written,
            PermissionAnswerDelivery::Failed(AgentError::Closed),
        ] {
            let record = selected.clone().with_delivery(delivery.clone());
            assert_correlation(&record, &first);
            assert_eq!(record.response(), selected.response());
            assert_eq!(record.actor(), Some(&actor()));
            assert_eq!(record.delivery(), &delivery);
        }
    }
}

#[test]
fn admitted_question_closure_retains_its_origin_after_the_execution_ends() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("original").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let admitted = controller
        .ask_question(
            &execution,
            QuestionId::new("ask").unwrap(),
            question("staging"),
        )
        .unwrap();
    let settlement = controller
        .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert_eq!(settlement.len(), 1);
    controller
        .begin_execution(ExecutionId::new("later").unwrap())
        .unwrap();
    let closed = admitted.closed_event();
    assert_eq!(closed.execution_id(), &execution);
    assert_eq!(
        closed.update(),
        &ExecutionUpdate::QuestionClosed {
            id: admitted.id().clone()
        }
    );
    for cause in [
        QuestionCancellation::ProviderWithdrawal,
        QuestionCancellation::ExecutionFinished,
        QuestionCancellation::SessionEnded,
    ] {
        for delivery in [
            PermissionAnswerDelivery::Written,
            PermissionAnswerDelivery::Failed(AgentError::Closed),
        ] {
            let record = QuestionAnswerRecord::ended(admitted.clone(), cause, delivery.clone());
            assert_correlation(&record, &admitted);
            assert_eq!(record.response(), &QuestionResponse::Cancelled(cause));
            assert_eq!(record.actor(), None);
            assert_eq!(record.delivery(), &delivery);
        }
    }
}

#[test]
fn admitted_question_requires_the_controllers_active_execution() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let active = ExecutionId::new("active").unwrap();
    let inactive = ExecutionId::new("other").unwrap();
    assert!(matches!(
        controller.ask_question(
            &active,
            QuestionId::new("idle").unwrap(),
            question("staging")
        ),
        Err(AgentError::InvalidInput(_))
    ));
    controller.begin_execution(active.clone()).unwrap();
    assert!(matches!(
        controller.ask_question(
            &inactive,
            QuestionId::new("foreign").unwrap(),
            question("staging")
        ),
        Err(AgentError::InvalidInput(_))
    ));
    let admitted = controller
        .ask_question(
            &active,
            QuestionId::new("accepted").unwrap(),
            question("staging"),
        )
        .unwrap();
    assert_eq!(admitted.execution_id(), &active);
    assert_eq!(admitted.session_id(), controller.id());
}
