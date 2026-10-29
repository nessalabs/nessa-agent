//! Public save boundaries refuse contradictory provenance before it can be encoded.
use super::{
    fixtures::{input_record, rows},
    opened,
};
use nessa_sdk::{
    application::agent_execution::sessions::{
        InvocationRecord, SessionChange, SessionSaveUnit, SessionStorage, StorageError,
    },
    domain::agent_execution::{
        executions::{AgentSource, ExecutionId, Initiation},
        prompts::MessageSender,
        sessions::SessionId,
    },
    infrastructure::session_storage::RecordStorage,
};

fn source(conversation: &str, execution: &str) -> AgentSource {
    AgentSource::new(conversation, ExecutionId::new(execution).unwrap()).unwrap()
}

fn origins() -> Vec<(Initiation, MessageSender)> {
    let original = source("8b547a4b-6974-49a4-b878-d13a9c031f16", "source-turn");
    vec![
        (Initiation::owner(), MessageSender::Agent(original.clone())),
        (Initiation::agent(original.clone()), MessageSender::Person),
        (Initiation::agent(original.clone()), MessageSender::Internal),
        (
            Initiation::agent(original.clone()),
            MessageSender::Agent(source(
                "8b547a4b-6974-49a4-b878-d13a9c031f17",
                "source-turn",
            )),
        ),
        (
            Initiation::agent(original),
            MessageSender::Agent(source(
                "8b547a4b-6974-49a4-b878-d13a9c031f16",
                "another-turn",
            )),
        ),
    ]
}

fn input(initiation: Initiation, sender: MessageSender) -> InvocationRecord {
    let mut record = input_record("agent-input", 10);
    record.initiation = initiation;
    record.request.user_message = record.request.user_message.sent_by(sender);
    record
}

#[test]
fn public_fold_refuses_disagreeing_agent_origin_before_retaining_input() {
    let id = SessionId::new("origin-fold").unwrap();
    for (initiation, sender) in origins() {
        let (opening, _) = opened(&id);
        let unit = SessionSaveUnit::new(vec![
            opening,
            SessionChange::InputAccepted(Box::new(input(initiation, sender))),
        ])
        .unwrap();
        assert!(matches!(
            SessionSaveUnit::fold(None, &[unit]),
            Err(StorageError::Corrupt(_))
        ));
    }
    let origin = source("8b547a4b-6974-49a4-b878-d13a9c031f16", "source-turn");
    let valid = input(
        Initiation::agent(origin.clone()),
        MessageSender::Agent(origin),
    );
    let (opening, _) = opened(&id);
    let unit = SessionSaveUnit::new(vec![
        opening,
        SessionChange::InputAccepted(Box::new(valid.clone())),
    ])
    .unwrap();
    assert_eq!(
        SessionSaveUnit::fold(None, &[unit]).unwrap().invocations,
        vec![valid]
    );
}

#[tokio::test]
async fn record_save_refuses_disagreeing_agent_origin_before_append_and_preserves_valid_origin() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("records");
    let storage = RecordStorage::new(&root).unwrap();
    storage.initialize().await.unwrap();
    let id = SessionId::new("origin-storage").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (opening, _) = opened(&id);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            vec![SessionSaveUnit::new(vec![opening]).unwrap()],
        )
        .await
        .unwrap();
    let binding = lease.load().await.unwrap().binding().clone();
    let prior_rows = rows(&root);
    for (initiation, sender) in origins() {
        let unit = SessionSaveUnit::new(vec![SessionChange::InputAccepted(Box::new(input(
            initiation, sender,
        )))])
        .unwrap();
        assert!(matches!(
            lease.save_changes(binding.clone(), vec![unit]).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(
            rows(&root),
            prior_rows,
            "refused input must not append records"
        );
        let loaded = lease.load().await.unwrap();
        assert_eq!(loaded.binding(), &binding);
        assert!(loaded.snapshot().unwrap().invocations.is_empty());
    }
    let origin = source("8b547a4b-6974-49a4-b878-d13a9c031f16", "source-turn");
    let valid = input(
        Initiation::agent(origin.clone()),
        MessageSender::Agent(origin),
    );
    lease
        .save_changes(
            binding,
            vec![
                SessionSaveUnit::new(vec![SessionChange::InputAccepted(Box::new(valid.clone()))])
                    .unwrap(),
            ],
        )
        .await
        .unwrap();
    drop(lease);
    let lease = storage.open_existing(id).await.unwrap().unwrap();
    let loaded = lease.load().await.unwrap();
    assert_eq!(loaded.snapshot().unwrap().invocations, vec![valid]);
    drop(lease);
    storage.shutdown().await.unwrap();
}
