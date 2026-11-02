//! Real uploaded-command provenance at the existing service owner.
use super::*;
use crate::conversation::application::input_files::tests::{caller, Fixture};
use crate::conversation::application::{PendingReport, SubmittedUpload, ThreadReport};
use std::sync::Arc;

#[tokio::test]
async fn report_authorship_matches_exact_agent_origin_and_preserves_legacy_person_records() {
    use nessa_sdk::application::agent_execution::sessions::SessionStorage;
    use nessa_sdk::domain::agent_execution::executions::{AgentSource, Initiation};
    use nessa_sdk::infrastructure::session_storage::RecordStorage;

    let fixture = Fixture::new().await;
    let directory = tempfile::tempdir().unwrap();
    let commands = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
    fixture.service.bind_commands(commands.clone());
    let pending = PendingReport {
        child: ConversationId::new("8b547a4b-6974-49a4-b878-d13a9c031f16").unwrap(),
        execution: "watched-child-turn".into(),
        report: ThreadReport {
            parent: fixture.id.clone(),
            caller: caller("report-author"),
            execution: "report-delivery".into(),
        },
    };
    fixture
        .service
        .submit_command(
            commands.clone(),
            fixture.id.clone(),
            pending.report.caller.clone(),
            Initiation::owner(),
            false,
            pending.report.execution.clone(),
            SubmittedMessage {
                text: format!(
                    "[Thread report] Thread {} finished the turn you sent it (execution {}): completed",
                    pending.child, pending.execution
                ),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    fixture.ended(&pending.report.execution).await;
    let committed = fixture
        .service
        .inner
        .storage
        .read_committed(conversation_session(&fixture.id))
        .await
        .unwrap()
        .unwrap();
    let history = &committed.snapshot().unwrap().invocations;
    let legacy = history
        .iter()
        .find(|record| record.request.execution_id.as_str() == pending.report.execution)
        .unwrap();
    assert!(is_report_record(&pending, legacy));
    let people = fixture
        .service
        .person_turns(&fixture.id, &caller("inspect-provenance"), history)
        .await
        .unwrap();
    assert!(
        people
            .iter()
            .any(|(_, turn)| turn == &pending.report.execution),
        "a genuine person-command collision remains excluded from report Stop capture"
    );
    let deliveries = report_deliveries(&pending.report.execution, history, |record| {
        is_report_record(&pending, record)
    });
    assert_eq!(deliveries.len(), 1);
    assert!(
        deliveries[0].contains_person_command(&people),
        "the same receipt also excludes legacy Person acknowledgement during delivery"
    );
    for (conversation, execution, expected) in [
        (pending.child.to_string(), pending.execution.clone(), true),
        (
            "8b547a4b-6974-49a4-b878-d13a9c031f17".into(),
            pending.execution.clone(),
            false,
        ),
        (
            pending.child.to_string(),
            "another-child-turn".into(),
            false,
        ),
    ] {
        let author = AgentSource::new(conversation, ExecutionId::new(execution).unwrap()).unwrap();
        let mut record = legacy.clone();
        record.initiation = Initiation::agent(author.clone());
        record.request.user_message = record
            .request
            .user_message
            .sent_by(MessageSender::Agent(author));
        assert_eq!(is_report_record(&pending, &record), expected);
    }
    fixture.service.shutdown().await.unwrap();
    commands.shutdown().await.unwrap();
}

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
