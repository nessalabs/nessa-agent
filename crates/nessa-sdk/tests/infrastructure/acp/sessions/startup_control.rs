//! Startup stop selection stays deterministic when provider readiness races it.
use super::*;
use crate::application::agent_execution::permissions::ActionContext;

#[tokio::test]
async fn an_already_published_stop_wins_over_an_already_ready_generation() {
    let (startup_sender, mut startup) = oneshot::channel();
    startup_sender
        .send(Ok(ExecutionSessionId::new("simultaneous-ready").unwrap()))
        .unwrap();
    let request = SessionCloseRequest::Explicit(
        ActionContext::new("owner", "surface", "simultaneous-stop").unwrap(),
    );
    let (control_sender, control_receiver) = watch::channel(Some(request.clone()));
    let mut control = ProviderOpenControl::new(control_receiver);
    let (close_sender, close_receiver) = watch::channel(None);

    let (result, stop_selected) =
        await_startup_or_stop(&mut startup, &close_sender, &mut control).await;

    assert_eq!(result, Err(AgentError::Closed));
    assert!(stop_selected);
    assert_eq!(close_receiver.borrow().as_ref(), Some(&request));
    drop(control_sender);
}
