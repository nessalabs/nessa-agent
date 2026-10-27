//! Catalogue changes acknowledged by the same physical store that publishes notices.
use super::{uuid, Setup};
use crate::conversation::application::{
    ConversationCaller, ConversationRepository, ConversationSummaries, ShareGrantChange,
    ShareGrantTransition, ShareGrants, MAX_LISTED_CONVERSATIONS,
};
use crate::conversation::domain::{Conversation, ShareAccess};
use crate::conversation::infrastructure::LocalConversationStore;
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId, ConversationSummary,
};
use std::io::{self, Write};
use std::sync::Arc;
use tokio::io::{stdin, AsyncBufReadExt, BufReader};

pub(super) async fn serve(setup: &Setup, metadata: Arc<LocalConversationStore>) {
    let main = ConversationId::new(&setup.conversation).unwrap();
    let empty = ConversationId::new(&setup.empty).unwrap();
    let hidden = ConversationId::new(&uuid()).unwrap();
    for index in 0..=MAX_LISTED_CONVERSATIONS {
        let id = if index == 0 {
            hidden.clone()
        } else {
            ConversationId::new(&uuid()).unwrap()
        };
        metadata
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new(&setup.organization).unwrap(),
                    PrincipalId::new(&setup.owner).unwrap(),
                    "catalogue".into(),
                    uuid(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("model").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        summary(&metadata, &id, 1000 + index as u64, false).await;
    }
    summary(&metadata, &main, 2, false).await;
    summary(&metadata, &empty, 1, false).await;
    println!("ONLINE_READY");
    io::stdout().flush().unwrap();
    let mut lines = BufReader::new(stdin()).lines();
    let mut revision = 10_000;
    while let Some(line) = lines.next_line().await.unwrap() {
        revision += 1;
        match line.as_str() {
            "hidden" => summary(&metadata, &hidden, revision, false).await,
            "archive" => summary(&metadata, &main, 2, true).await,
            "unarchive" => summary(&metadata, &main, 2, false).await,
            "replace two" | "replace three" | "restore" | "revoke" => {
                let receiver = match line.as_str() {
                    "replace two" => "catalogue-two",
                    "replace three" => "catalogue-three",
                    _ => &setup.receiver,
                };
                let credential = if receiver == setup.receiver {
                    &setup.credential
                } else {
                    receiver
                };
                assert!(
                    metadata
                        .change(ShareGrantChange {
                            transition: if line == "revoke" {
                                ShareGrantTransition::Revoke
                            } else {
                                ShareGrantTransition::Grant(ShareAccess::read())
                            },
                            conversation_id: main.clone(),
                            principal_id: Some(
                                PrincipalId::new(format!("grantee-{}", setup.receiver)).unwrap()
                            ),
                            receiver_id: Some(receiver.into()),
                            credential_id: CredentialId::new(credential).unwrap(),
                            expected_policy_revision: None,
                            initiator: ConversationCaller {
                                organization_id: OrganizationId::new(&setup.organization).unwrap(),
                                principal_id: PrincipalId::new(&setup.owner).unwrap(),
                                surface_id: "catalogue".into(),
                                action_id: uuid(),
                            },
                            at_ms: revision,
                        })
                        .await
                        .unwrap()
                        .applied
                );
            }
            _ => panic!("unknown catalogue control {line}"),
        }
        println!("DONE {line}");
        io::stdout().flush().unwrap();
    }
    std::future::pending::<()>().await;
}

async fn summary(metadata: &LocalConversationStore, id: &ConversationId, at: u64, archived: bool) {
    metadata
        .record(
            id,
            ConversationSummary::new(None, None, at, archived).unwrap(),
        )
        .await
        .unwrap();
}
