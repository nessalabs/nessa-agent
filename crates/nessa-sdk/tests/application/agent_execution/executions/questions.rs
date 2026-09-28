//! Public adapter admission and answer evidence keep one question correlation.
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{AdmittedQuestion, ExecutionController, ExecutionUpdate},
    permissions::{
        ActionContext, CancellationOrigin, PermissionAnswerDelivery, QuestionAnswerRecord,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome},
    permissions::PermissionCancellationReason,
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
            QuestionId::new("accepted").unwrap(),
            question("staging")
        ),
        Err(AgentError::InvalidInput(_))
    ));
    controller.begin_execution(active.clone()).unwrap();
    assert!(matches!(
        controller.ask_question(
            &inactive,
            QuestionId::new("accepted").unwrap(),
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

#[test]
fn question_identity_cannot_be_readmitted_with_replacement_contents() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let id = QuestionId::new("A").unwrap();
    let admitted = controller
        .ask_question(&execution, id.clone(), question("staging"))
        .unwrap();
    for stage in 0..5 {
        let evidence = match stage {
            0 => None,
            1 => Some(
                QuestionAnswerRecord::chosen(
                    admitted.clone(),
                    choices("staging"),
                    actor(),
                    PermissionAnswerDelivery::Written,
                )
                .unwrap(),
            ),
            2 => Some(
                QuestionAnswerRecord::chosen(
                    admitted.clone(),
                    None,
                    actor(),
                    PermissionAnswerDelivery::Written,
                )
                .unwrap(),
            ),
            3 => Some(QuestionAnswerRecord::ended(
                admitted.clone(),
                QuestionCancellation::ProviderWithdrawal,
                PermissionAnswerDelivery::Written,
            )),
            _ => {
                assert!(matches!(
                    controller.finish_execution(
                        &execution,
                        Err(PermissionCancellationReason::session_closed()),
                    ),
                    Err(AgentError::InvalidInput(_))
                ));
                assert!(matches!(
                    controller.finish_execution(
                        &ExecutionId::new("stale").unwrap(),
                        Ok(ExecutionOutcome::Completed),
                    ),
                    Err(AgentError::Protocol(_))
                ));
                None
            }
        };
        for value in ["staging", "production"] {
            assert!(matches!(
                controller.ask_question(&execution, id.clone(), question(value)),
                Err(AgentError::InvalidInput(_))
            ));
        }
        assert_eq!(controller.active_execution_id(), Some(&execution));
        if let Some(record) = evidence {
            assert_correlation(&record, &admitted);
            assert_eq!(record.question(), &question("staging"));
        }
    }
    let _settled = controller
        .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert!(matches!(
        controller.begin_execution(execution),
        Err(AgentError::InvalidInput(_))
    ));
    let next = ExecutionId::new("next").unwrap();
    controller.begin_execution(next.clone()).unwrap();
    let later = controller
        .ask_question(&next, id, question("production"))
        .unwrap();
    assert_ne!(later.execution_id(), admitted.execution_id());
    assert_eq!(later.question(), &question("production"));
    let original = QuestionAnswerRecord::chosen(
        admitted.clone(),
        choices("staging"),
        actor(),
        PermissionAnswerDelivery::Written,
    )
    .unwrap();
    assert_correlation(&original, &admitted);
}

#[test]
fn question_identity_history_is_bounded_and_released_only_at_settlement() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let original = question("staging");
    for index in 0..ExecutionController::MAX_QUESTIONS_PER_EXECUTION {
        let id = QuestionId::new(index.to_string()).unwrap();
        let admitted = controller
            .ask_question(&execution, id.clone(), original.clone())
            .unwrap();
        let record = QuestionAnswerRecord::ended(
            admitted,
            QuestionCancellation::ProviderWithdrawal,
            PermissionAnswerDelivery::Written,
        );
        assert_eq!(record.question(), &original);
        assert!(matches!(
            controller.ask_question(&execution, id, question("production")),
            Err(AgentError::InvalidInput(_))
        ));
    }
    let extra = QuestionId::new("over-limit").unwrap();
    assert!(matches!(
        controller.ask_question(&execution, extra.clone(), original.clone()),
        Err(AgentError::Protocol(_))
    ));
    let _settled = controller
        .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
        .unwrap();
    let next = ExecutionId::new("next").unwrap();
    controller.begin_execution(next.clone()).unwrap();
    let admitted = controller
        .ask_question(&next, extra, original.clone())
        .unwrap();
    assert_eq!(admitted.question(), &original);
}

#[test]
fn closed_controller_cannot_admit_questions_while_execution_awaits_settlement() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("provider").unwrap());
    let execution = ExecutionId::new("run").unwrap();
    controller.begin_execution(execution.clone()).unwrap();
    let admitted = controller
        .ask_question(
            &execution,
            QuestionId::new("A").unwrap(),
            question("staging"),
        )
        .unwrap();
    let _closure = controller
        .close(
            PermissionCancellationReason::session_closed(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert_eq!(controller.active_execution_id(), Some(&execution));
    assert_eq!(
        controller.ask_question(
            &execution,
            QuestionId::new("B").unwrap(),
            question("production")
        ),
        Err(AgentError::Closed)
    );
    let ended = QuestionAnswerRecord::ended(
        admitted.clone(),
        QuestionCancellation::SessionEnded,
        PermissionAnswerDelivery::Written,
    );
    assert_correlation(&ended, &admitted);
}
