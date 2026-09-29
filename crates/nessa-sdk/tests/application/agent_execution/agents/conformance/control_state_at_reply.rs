//! Provider session state closes control admission at reply receipt, even while evidence saves.
use super::*;
use nessa_sdk::application::agent_execution::providers::ResourceCleanup;
use nessa_sdk::domain::agent_execution::questions::QuestionId;

#[derive(Clone, Copy, Debug)]
enum SaveTrigger {
    Timer,
    Threshold,
    Consequential,
}

#[tokio::test]
async fn provider_reply_updates_control_admission_before_stalled_save_finishes() {
    let confirmed = CleanupReport::confirmed(CloseOutcome { forced: false });
    let unconfirmed = CleanupReport::new(
        ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain),
        Ok(()),
    );
    let confirmed_audit_failed = CleanupReport::new(
        ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
        Err(AgentError::AuditFailure),
    );
    let statuses = [
        (ProviderSessionState::CleanupRequired, false),
        (ProviderSessionState::CleanupReported(confirmed), false),
        (ProviderSessionState::CleanupReported(unconfirmed), false),
        (
            ProviderSessionState::CleanupReported(confirmed_audit_failed),
            false,
        ),
        (ProviderSessionState::Usable, true),
    ];
    for mode in Mode::ALL {
        for trigger in [
            SaveTrigger::Timer,
            SaveTrigger::Threshold,
            SaveTrigger::Consequential,
        ] {
            for (session_state, control_allowed) in &statuses {
                let (agent, backend, storage) = workflow().await;
                *backend.execution_report.lock().unwrap() = Some(ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Completed)),
                    None,
                    session_state.clone(),
                ));
                let (release_report, report_gate) = oneshot::channel();
                *backend.execution_gate.lock().unwrap() = Some(report_gate);
                let execution_id = ExecutionId::new("control-during-save").unwrap();
                let mut updates = agent.subscribe();
                let running = mode.start(agent.clone(), request(execution_id.as_str()));
                bounded(backend.dispatched.notified()).await;
                let (saving, release_save) = storage.pause_next_save();
                let update = match trigger {
                    SaveTrigger::Timer => {
                        ExecutionUpdate::Message(MessageChunk::text("small pending message"))
                    }
                    SaveTrigger::Threshold => {
                        ExecutionUpdate::Message(MessageChunk::text("x".repeat(16 * 1024)))
                    }
                    SaveTrigger::Consequential => {
                        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
                    }
                };
                backend
                    .output
                    .lock()
                    .unwrap()
                    .send(Some(ExecutionEvent::new(execution_id.clone(), update)))
                    .unwrap();
                if matches!(trigger, SaveTrigger::Timer) {
                    bounded(updates.next()).await.unwrap().unwrap();
                }
                bounded(saving).await.unwrap();
                release_report.send(()).unwrap();
                let mut report_polled = backend.report_polled.subscribe();
                bounded(report_polled.wait_for(|polled| *polled))
                    .await
                    .unwrap();
                let control = bounded(agent.answer_question(QuestionAnswer {
                    actor: actor(),
                    execution_id,
                    id: QuestionId::new("ask").unwrap(),
                    choices: None,
                }))
                .await;
                assert!(control.is_err(), "fixture refuses answered questions");
                assert_eq!(
                    backend.question_calls.load(Ordering::SeqCst),
                    usize::from(*control_allowed),
                    "{mode:?} {trigger:?} {session_state:?}: {control:?}"
                );
                release_save.send(()).unwrap();
                let result = bounded(running).await.unwrap();
                let saved = storage.snapshot();
                assert_eq!(saved.invocations[0].result, Some(result));
                assert_eq!(
                    saved.invocations[0]
                        .provider_report
                        .as_ref()
                        .unwrap()
                        .session_state(),
                    session_state
                );
            }
        }
    }
}
