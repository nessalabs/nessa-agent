//! The first stop belongs to each admitted input, across invocation delivery modes.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Immediate,
    Queued,
    BoundarySteering,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
                                        .invoke(input("stopped-before-dispatch"), actor())
                                        .await
                                }
                                Delivery::Queued => {
                                    agent
                                        .enqueue(input("stopped-before-dispatch"), actor())
                                        .await
                                        .unwrap()
                                        .wait()
                                        .await
                                }
                                Delivery::BoundarySteering => {
                                    agent
                                        .enqueue_steering(input("stopped-before-dispatch"), actor())
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
                        .enqueue(input("still-waiting"), actor())
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
                        let cancellation = active.scheduling.last().unwrap();
                        assert_eq!(cancellation.stage, InvocationStage::Cancelled);
                        assert_eq!(
                            cancellation.cause,
                            SchedulingCause::RunnerStopped,
                            "{delivery:?}"
                        );
                        assert!(cancellation.actor.is_none());
                    }
                    let waiting = saved.invocations[1].scheduling.last().unwrap();
                    assert_eq!(waiting.stage, InvocationStage::Cancelled);
                    assert_eq!(
                        waiting.cause,
                        if audit_failed {
                            SchedulingCause::RunnerStopped
                        } else {
                            SchedulingCause::SessionClosed
                        }
                    );
                    assert_eq!(
                        waiting.actor,
                        if audit_failed { None } else { Some(closer) }
                    );
                    assert!(agent.session_manager().snapshot().await.is_some());
                }
            }
        }
    }
}
