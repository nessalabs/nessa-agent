//! Handoffs through a real agent: a switch carries a conversation on under
//! another provider in a new native session, a fork starts one from another's
//! turns, and the first turn after either is given what came before.
use super::*;
use nessa_sdk::application::agent_execution::sessions::{HandoffIntent, HandoffKind};
use nessa_sdk::domain::agent_execution::prompts::{CarriedHistory, CarriedTurn};

fn asked(request: &str) -> ActionContext {
    ActionContext::new("person", "desktop", request).unwrap()
}
fn other_provider() -> Arc<TestProvider> {
    let mut other = TestProvider::new();
    let identity = &mut Arc::get_mut(&mut other).unwrap().identity;
    *identity =
        ProviderIdentity::new("another-provider", identity.model_id(), identity.context()).unwrap();
    other
}
fn last_history(provider: &TestProvider) -> Option<CarriedHistory> {
    provider
        .calls
        .inputs
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .user_message
        .history()
        .cloned()
}
fn turns(history: &CarriedHistory) -> Vec<(Option<&str>, Option<&str>)> {
    history
        .turns()
        .iter()
        .map(|turn| (turn.said(), turn.reply()))
        .collect()
}

/// Rows H1, H6, H7: a switch opens a fresh native session of the new
/// provider, records one handoff with the identity it left, and gives the
/// next turn, and only it, the turns before. Reopened under the new provider
/// it resumes its own session; under the old one it is refused.
#[tokio::test]
async fn a_switch_opens_a_fresh_session_and_carries_the_turns_before_it() {
    let storage = MemoryStorage::default();
    let first = TestProvider::new();
    let agent = attached_agent(first.clone(), storage.manager().await)
        .await
        .unwrap();
    invoke(&agent, "first").await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);

    // Without the intent, another provider is still refused.
    let other = other_provider();
    assert!(matches!(
        attached_agent(other.clone(), storage.manager().await).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
    let mut manager = storage.manager().await;
    manager.expect_handoff(HandoffIntent::Switch {
        actor: asked("switch"),
        at_ms: 5,
    });
    let agent = attached_agent(other.clone(), manager).await.unwrap();
    assert_eq!(other.calls.opens.lock().unwrap().as_slice(), &[None]);
    let saved = storage.snapshot();
    assert_eq!(saved.provider, other.identity);
    let handoff = saved.handoff("person", "switch").expect("the switch");
    assert_eq!(
        handoff.kind,
        HandoffKind::Switch {
            before: first.identity.clone(),
            after: other.identity.clone(),
        }
    );
    assert_eq!(
        handoff.after_execution,
        Some(ExecutionId::new("first").unwrap())
    );
    let id = handoff.id.clone();

    invoke(&agent, "second").await.unwrap();
    let carried = last_history(&other).expect("the turns before the switch");
    assert_eq!(
        turns(&carried),
        vec![(Some("message first"), Some("message first"))]
    );
    assert_eq!(storage.snapshot().invocations[1].handoff, Some(id));
    invoke(&agent, "third").await.unwrap();
    assert_eq!(last_history(&other), None);
    agent.close(close_action()).await.unwrap();
    drop(agent);

    let agent = attached_agent(other.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(
        other.calls.opens.lock().unwrap().last().unwrap(),
        &Some(ExecutionSessionId::new("provider-context").unwrap())
    );
    assert_eq!(storage.snapshot().handoffs.len(), 1);
    agent.close(close_action()).await.unwrap();
    drop(agent);
    assert!(matches!(
        attached_agent(first.clone(), storage.manager().await).await,
        Err(AgentError::Storage(StorageError::IdentityMismatch))
    ));
}

/// A switch needs a history to leave; a switch to the same provider records
/// nothing and resumes as before.
#[tokio::test]
async fn a_switch_without_history_is_refused_and_one_to_the_same_provider_records_nothing() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let mut manager = storage.manager().await;
    manager.expect_handoff(HandoffIntent::Switch {
        actor: asked("switch"),
        at_ms: 5,
    });
    assert!(matches!(
        attached_agent(provider.clone(), manager).await,
        Err(AgentError::InvalidInput(_))
    ));
    assert!(provider.calls.opens.lock().unwrap().is_empty());

    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    invoke(&agent, "first").await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let mut manager = storage.manager().await;
    manager.expect_handoff(HandoffIntent::Switch {
        actor: asked("switch"),
        at_ms: 5,
    });
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    assert!(storage.snapshot().handoffs.is_empty());
    invoke(&agent, "second").await.unwrap();
    assert_eq!(last_history(&provider), None);
    agent.close(close_action()).await.unwrap();
}

/// Row H5: a fork records the copied turns with the new conversation and
/// gives them to its first turn; the same request reopening it records
/// nothing again, and a fork into a conversation that exists is refused.
#[tokio::test]
async fn a_fork_starts_a_conversation_from_the_turns_it_copied() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let copied = CarriedHistory::new(
        vec![CarriedTurn::new(Some("plot May"), Some("done: may.png")).unwrap()],
        2,
    )
    .unwrap();
    let intent = HandoffIntent::Fork {
        source: SessionId::new("source").unwrap(),
        history: copied.clone(),
        actor: asked("fork"),
        at_ms: 9,
    };
    let mut manager = storage.manager().await;
    manager.expect_handoff(intent.clone());
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    let saved = storage.snapshot();
    assert_eq!(saved.handoffs.len(), 1);
    assert_eq!(saved.carried_history(), copied);
    invoke(&agent, "first").await.unwrap();
    let carried = last_history(&provider).expect("the copied turns");
    assert_eq!(carried.omitted(), 2);
    assert_eq!(
        turns(&carried),
        vec![(Some("plot May"), Some("done: may.png"))]
    );
    agent.close(close_action()).await.unwrap();
    drop(agent);

    // The same fork again finds it recorded.
    let mut manager = storage.manager().await;
    manager.expect_handoff(intent);
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    assert_eq!(storage.snapshot().handoffs.len(), 1);
    agent.close(close_action()).await.unwrap();
    drop(agent);
    // Another fork into it is refused.
    let mut manager = storage.manager().await;
    manager.expect_handoff(HandoffIntent::Fork {
        source: SessionId::new("source").unwrap(),
        history: copied,
        actor: asked("again"),
        at_ms: 9,
    });
    assert!(matches!(
        attached_agent(provider.clone(), manager).await,
        Err(AgentError::InvalidInput(_))
    ));
}

/// A fork whose first opening ended before the fork was saved is saved by the
/// next opening that asks for it, while the conversation holds no turn yet.
#[tokio::test]
async fn a_fork_is_saved_by_a_later_opening_while_nothing_came_before_it() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let agent = attached_agent(provider.clone(), storage.manager().await)
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let mut manager = storage.manager().await;
    manager.expect_handoff(HandoffIntent::Fork {
        source: SessionId::new("source").unwrap(),
        history: CarriedHistory::new(
            vec![CarriedTurn::new(Some("hello"), Some("hi")).unwrap()],
            0,
        )
        .unwrap(),
        actor: asked("fork"),
        at_ms: 9,
    });
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    assert!(storage.snapshot().handoff("person", "fork").is_some());
    invoke(&agent, "first").await.unwrap();
    assert_eq!(
        turns(&last_history(&provider).unwrap()),
        vec![(Some("hello"), Some("hi"))]
    );
    agent.close(close_action()).await.unwrap();
}
