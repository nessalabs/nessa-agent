//! The first stop belongs to each admitted input, across invocation delivery modes.
use super::*;
use nessa_sdk::application::agent_execution::sessions::InvocationSchedulingEvent;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Immediate,
    Queued,
    BoundarySteering,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn active_input_keeps_automatic_cause_when_later_close_overtakes_dispatch() {
    for delivery in [
        Delivery::Immediate,
        Delivery::Queued,
        Delivery::BoundarySteering,
    ] {
        for source in [ProviderControl::Answer, ProviderControl::CancelPermission] {
            for audit_failed in [false, true] {
                for reject_hook in [false, true] {
                    let (agent, backend, storage) = probe(false).await;
                    let (entered, waiting) = oneshot::channel();
                    let (release, released) = std::sync::mpsc::channel();
                    agent.add_invocation_hook(Arc::new(PausedBeforeDispatch {
                        entered: Mutex::new(Some(entered)),
                        release: Mutex::new(released),
                        reject: reject_hook,
                    }));
                    let invocation = tokio::spawn({
                        let agent = agent.clone();
                        async move {
                            match delivery {
                                Delivery::Immediate => {
                                    agent
                                        .invoke(
                                            input("stopped-before-dispatch"),
                                            Submitter::owner(actor()),
                                        )
                                        .await
                                }
                                Delivery::Queued => {
                                    agent
                                        .enqueue(
                                            input("stopped-before-dispatch"),
                                            Submitter::owner(actor()),
                                        )
                                        .await
                                        .unwrap()
                                        .wait()
                                        .await
                                }
                                Delivery::BoundarySteering => {
                                    agent
                                        .enqueue_steering(
                                            input("stopped-before-dispatch"),
                                            Submitter::owner(actor()),
                                        )
                                        .await
                                        .unwrap()
                                        .wait()
                                        .await
                                }
                            }
                        }
                    });
                    waiting.await.unwrap();
                    let waiting_input = agent
                        .enqueue(input("still-waiting"), Submitter::owner(actor()))
                        .await
                        .unwrap();
                    *backend.control_attachment.lock().unwrap() =
                        ProviderSessionState::CleanupReported(if audit_failed {
                            cleaned_with_error(AgentError::AuditFailure)
                        } else {
                            CleanupReport::confirmed(CloseOutcome { forced: false })
                        });
                    assert_eq!(
                        invoke_control(&agent, source).await,
                        Err(AgentError::StalePermission)
                    );
                    let closer = ActionContext::new("owner", "phone", "later-close").unwrap();
                    let closing = tokio::spawn({
                        let agent = agent.clone();
                        let closer = closer.clone();
                        async move { agent.close(closer).await }
                    });
                    // Close settles waiting receipts while the active hook still holds;
                    // failed audit may already have assigned their automatic cause.
                    assert_eq!(
                        timeout(Duration::from_secs(2), waiting_input.wait())
                            .await
                            .unwrap(),
                        Err(AgentError::Closed)
                    );
                    release.send(()).unwrap();
                    let result = timeout(Duration::from_secs(2), invocation)
                        .await
                        .unwrap()
                        .unwrap();
                    let closed = timeout(Duration::from_secs(2), closing)
                        .await
                        .unwrap()
                        .unwrap();
                    if reject_hook {
                        assert!(
                            matches!(result, Err(AgentError::BeforeInvocationHook(_))),
                            "{delivery:?}: {result:?}"
                        );
                    } else {
                        assert_eq!(result, Err(AgentError::Closed), "{delivery:?}");
                    }
                    assert_eq!(
                        closed.map(|_| ()),
                        if audit_failed {
                            Err(AgentError::AuditFailure)
                        } else {
                            Ok(())
                        }
                    );
                    assert_eq!(backend.executions.load(Ordering::SeqCst), 0);
                    let saved = storage.snapshot();
                    let active = &saved.invocations[0];
                    assert_eq!(
                        active.request.execution_id.as_str(),
                        "stopped-before-dispatch"
                    );
                    assert_eq!(active.result, Some(result));
                    assert!(active.events.is_empty());
                    assert!(active.provider_report.is_none());
                    if matches!(delivery, Delivery::Immediate) {
                        assert!(active.scheduling.is_empty());
                        let cancellation = active.cancellation.as_ref().unwrap();
                        assert_eq!(cancellation.cause, SchedulingCause::RunnerStopped);
                        assert!(cancellation.actor.is_none());
                    } else {
                        assert!(active.cancellation.is_none());
                        assert_original_stop_and_finality(
                            &active.scheduling,
                            SchedulingCause::RunnerStopped,
                            None,
                            &closer,
                        );
                    }
                    assert_original_stop_and_finality(
                        &saved.invocations[1].scheduling,
                        if audit_failed {
                            SchedulingCause::RunnerStopped
                        } else {
                            SchedulingCause::SessionClosed
                        },
                        if audit_failed {
                            None
                        } else {
                            Some(closer.clone())
                        },
                        &closer,
                    );
                    if audit_failed {
                        // The earlier close stage failed: no finality decision
                        // may turn that error into a cleanup result.
                        assert!(saved.invocations.iter().all(|record| record
                            .scheduling
                            .iter()
                            .all(|edge| edge.cause != SchedulingCause::NotResumed)));
                    } else {
                        assert!(agent.owed_turns().await.is_empty());
                    }
                    assert!(agent.session_manager().snapshot().await.is_some());
                }
            }
        }
    }
}

/// Final Close can append its decision; it must preserve the earlier stop.
fn assert_original_stop_and_finality(
    history: &[InvocationSchedulingEvent],
    cause: SchedulingCause,
    actor: Option<ActionContext>,
    closer: &ActionContext,
) {
    assert_eq!(history.last().unwrap().stage, InvocationStage::Cancelled);
    let stops: Vec<_> = history
        .iter()
        .filter(|edge| edge.stage == InvocationStage::Cancelled)
        .collect();
    let original = stops.first().expect("original stop remains saved");
    assert_eq!(original.cause, cause);
    assert_eq!(original.actor, actor);
    assert!(stops.len() <= 2, "at most one later finality decision");
    if let Some(finality) = stops.get(1) {
        assert_eq!(finality.cause, SchedulingCause::NotResumed);
        assert_eq!(finality.before, Some(InvocationStage::Cancelled));
        assert_eq!(finality.actor.as_ref(), Some(closer));
        assert_eq!(finality.kind, original.kind);
        assert_eq!(finality.target, original.target);
    }
}
