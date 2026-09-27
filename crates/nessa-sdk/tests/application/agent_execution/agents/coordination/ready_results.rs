//! Control acknowledgements retain their meaning when shutdown becomes ready too.
use super::*;
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};

async fn poll_once<T>(mut future: Pin<&mut impl Future<Output = T>>) -> Poll<T> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn started_control_retains_ready_result_when_stop_is_also_ready() {
    for explicit in [false, true] {
        for success in [false, true] {
            let agent = agent().await;
            let admission = agent.accept_control().unwrap();
            let (release, waiting) = oneshot::channel();
            let polls = AtomicUsize::new(0);
            let operation = async {
                polls.fetch_add(1, Ordering::SeqCst);
                waiting.await.unwrap();
                if success {
                    Ok(())
                } else {
                    Err(ProviderOperationFailure::new(
                        AgentError::StalePermission,
                        ProviderSessionState::Usable,
                    ))
                }
            };
            let control = agent.run_control(admission, operation);
            tokio::pin!(control);
            assert!(poll_once(control.as_mut()).await.is_pending());
            let _stop = agent.start_shutdown(if explicit {
                SessionCloseRequest::Explicit(actor())
            } else {
                SessionCloseRequest::ExecutionFailed
            });
            // Neither future can be repolled between publishing these two results.
            release.send(()).unwrap();
            assert_eq!(
                poll_once(control.as_mut()).await,
                Poll::Ready(if success {
                    Ok(())
                } else {
                    Err(AgentError::StalePermission)
                })
            );
            assert_eq!(polls.load(Ordering::SeqCst), 1);
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn stopped_control_does_not_wait_for_a_pending_acknowledgement() {
    for explicit in [false, true] {
        let agent = agent().await;
        let admission = agent.accept_control().unwrap();
        let (_release, waiting) = oneshot::channel::<()>();
        let control = agent.run_control(admission, async {
            waiting.await.unwrap();
            Ok(())
        });
        tokio::pin!(control);
        assert!(poll_once(control.as_mut()).await.is_pending());
        let _stop = agent.start_shutdown(if explicit {
            SessionCloseRequest::Explicit(actor())
        } else {
            SessionCloseRequest::ExecutionFailed
        });
        assert_eq!(
            poll_once(control.as_mut()).await,
            Poll::Ready(Err(AgentError::Closed))
        );
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn stop_before_first_control_poll_prevents_provider_effects() {
    for explicit in [false, true] {
        let agent = agent().await;
        let admission = agent.accept_control().unwrap();
        let polls = AtomicUsize::new(0);
        let control = agent.run_control(admission, async {
            polls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        let _stop = agent.start_shutdown(if explicit {
            SessionCloseRequest::Explicit(actor())
        } else {
            SessionCloseRequest::ExecutionFailed
        });
        assert_eq!(control.await, Err(AgentError::Closed));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        agent.close(actor()).await.unwrap();
    }
}

#[tokio::test]
async fn stopped_started_control_reads_ready_acknowledgement_with_no_task_budget() {
    let agent = agent().await;
    let admission = agent.accept_control().unwrap();
    let (release, mut receiver) = tokio::sync::mpsc::channel::<()>(1);
    let control = agent.run_control(admission, async {
        receiver.recv().await.unwrap();
        Ok(())
    });
    tokio::pin!(control);
    assert!(poll_once(control.as_mut()).await.is_pending());
    let _stop = agent.start_shutdown(SessionCloseRequest::ExecutionFailed);
    release.try_send(()).unwrap();
    // Spend the task budget without yielding; mpsc has a ready value but its
    // ordinary poll would report Pending solely for runtime fairness.
    poll_fn(|cx| {
        for _ in 0..256 {
            let budget = tokio::task::consume_budget();
            tokio::pin!(budget);
            if budget.poll(cx).is_pending() {
                return Poll::Ready(());
            }
        }
        panic!("expected finite cooperative budget");
    })
    .await;
    assert_eq!(poll_once(control.as_mut()).await, Poll::Ready(Ok(())));
    agent.close(actor()).await.unwrap();
}
