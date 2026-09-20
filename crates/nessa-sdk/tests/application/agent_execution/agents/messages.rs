//! Actual message bytes are validated independently of caller token estimates.
use super::*;

async fn submit(
    agent: &Agent,
    input: ExecutionRequest,
    operation: usize,
) -> Result<ExecutionOutcome, AgentError> {
    match operation {
        0 => agent.invoke(input, actor()).await,
        1 => agent.enqueue(input, actor()).await?.wait().await,
        2 => agent.enqueue_steering(input, actor()).await?.wait().await,
        _ => match agent.steer(input, actor()).await? {
            SteeringDelivery::Queued(receipt) => receipt.wait().await,
            SteeringDelivery::Injected { .. } => panic!("idle fixture cannot inject"),
        },
    }
}

#[tokio::test]
async fn message_byte_limit_precedes_every_admission_save() {
    for operation in 0..4 {
        for oversized in [false, true] {
            let storage = MemoryStorage::default();
            let provider = TestProvider::new();
            let agent = Agent::new(provider.clone(), storage.manager().await)
                .await
                .unwrap();
            let writes = storage.0.lock().unwrap().writes;
            let mut text = "é".repeat(ExecutionRequest::MAX_MESSAGE_BYTES / 2);
            if oversized {
                text.push('x');
            }
            let input = ExecutionRequest {
                user_message: UserMessage::text_only(PromptText::new(text).unwrap()),
                ..request("bytes")
            };
            let result = submit(&agent, input, operation).await;
            if oversized {
                assert!(matches!(result, Err(AgentError::InvalidInput(_))));
                assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
                assert_eq!(storage.0.lock().unwrap().writes, writes);
                assert!(storage.snapshot().invocations.is_empty());
            } else {
                assert_eq!(result, Ok(ExecutionOutcome::Completed));
                assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
                assert_eq!(
                    storage.snapshot().invocations[0]
                        .request
                        .user_message
                        .text_str()
                        .len(),
                    ExecutionRequest::MAX_MESSAGE_BYTES
                );
            }
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn custom_storage_cannot_restore_oversized_input_before_provider_open() {
    let storage = MemoryStorage::default();
    let agent = Agent::new(TestProvider::new(), storage.manager().await)
        .await
        .unwrap();
    agent.invoke(request("saved"), actor()).await.unwrap();
    agent.close(actor()).await.unwrap();
    drop(agent);
    storage
        .0
        .lock()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .invocations[0]
        .request
        .user_message = UserMessage::text_only(
        PromptText::new("x".repeat(ExecutionRequest::MAX_MESSAGE_BYTES + 1)).unwrap(),
    );
    let writes = storage.0.lock().unwrap().writes;
    let provider = TestProvider::new();
    assert!(
        matches!(Agent::new(provider.clone(), storage.manager().await).await,
        Err(error) if matches!(error.cause(), AgentError::Storage(StorageError::Corrupt(_))))
    );
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert_eq!(storage.0.lock().unwrap().writes, writes);
    assert_eq!(
        storage.snapshot().invocations[0]
            .request
            .user_message
            .text_str()
            .len(),
        ExecutionRequest::MAX_MESSAGE_BYTES + 1
    );
}

#[tokio::test]
async fn an_image_for_a_text_only_binding_is_refused_before_every_admission_save() {
    use nessa_sdk::domain::{
        agent_execution::prompts::{ImageMediaType, ImageReference},
        common::value_objects::Sha256Digest,
    };
    let image =
        ImageReference::new(Sha256Digest::from_bytes([3; 32]), ImageMediaType::Png, 64).unwrap();
    for operation in 0..4 {
        for text in [Some("look at this"), None] {
            let storage = MemoryStorage::default();
            let provider = TestProvider::new();
            let agent = Agent::new(provider.clone(), storage.manager().await)
                .await
                .unwrap();
            let writes = storage.0.lock().unwrap().writes;
            let input = ExecutionRequest {
                user_message: UserMessage::new(
                    text.map(|text| PromptText::new(text).unwrap()),
                    vec![image],
                )
                .unwrap(),
                ..request("image")
            };
            // Refused, never sent as its text alone with the image dropped.
            let result = submit(&agent, input, operation).await;
            assert!(
                matches!(result, Err(AgentError::InvalidInput(_))),
                "{result:?}"
            );
            assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
            assert_eq!(storage.0.lock().unwrap().writes, writes);
            assert!(storage.snapshot().invocations.is_empty());
            agent.close(actor()).await.unwrap();
        }
    }
}
