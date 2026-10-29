//! Real uploaded-command provenance at the existing service owner.
use super::*;
use crate::conversation::application::input_files::tests::{caller, Fixture};
use crate::conversation::application::SubmittedUpload;
use std::sync::Arc;

#[cfg(unix)]
#[tokio::test]
async fn uploaded_person_turn_retains_its_exact_command_provenance() {
    use nessa_sdk::application::agent_execution::sessions::SessionStorage;
    use nessa_sdk::domain::agent_execution::executions::Initiation;
    use nessa_sdk::infrastructure::session_storage::RecordStorage;

    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let commands = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
    fixture.service.bind_commands(commands.clone());
    let file = fixture.keep(b"person receipt", "text/plain", "receipt.txt");
    fixture
        .service
        .submit_command(
            commands.clone(),
            fixture.id.clone(),
            caller("uploaded-person"),
            Initiation::owner(),
            false,
            "uploaded-person".into(),
            SubmittedMessage {
                text: String::new(),
                uploads: vec![SubmittedUpload {
                    digest: file.digest().to_string(),
                    media_type: file.media_type().as_str().into(),
                    size: file.size(),
                    name: file.name().into(),
                }],
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    fixture.ended("uploaded-person").await;
    let committed = fixture
        .service
        .inner
        .storage
        .read_committed(conversation_session(&fixture.id))
        .await
        .unwrap()
        .unwrap();
    let history = &committed.snapshot().unwrap().invocations;
    let people = fixture
        .service
        .person_turns(&fixture.id, &caller("inspect-provenance"), history)
        .await
        .unwrap();
    fixture.service.shutdown().await.unwrap();
    commands.shutdown().await.unwrap();
    assert_eq!(
        people.len(),
        1,
        "the real uploaded command retains its person admission"
    );
    assert_eq!(people[0].1, "uploaded-person");
}
