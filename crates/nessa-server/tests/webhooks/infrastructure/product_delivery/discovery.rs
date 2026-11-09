//! Bounded discovery, shared command availability and corruption refusal.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_overflow_keeps_shared_commands_and_healthy_sibling_available() {
    let configured = ["hook", "healthy"].map(|principal| ConfiguredWebhook {
        principal: PrincipalId::new(principal).unwrap(),
        registration: RegistrationConfig {
            destination: Destination::parse("https://recipient.test/synthetic").unwrap(),
            key: Arc::new(MacKey::new([7; 32])),
        },
    });
    let f = Harness::with_configured(None, None, configured.to_vec()).await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let first = issue_share(&f, &mut client).await;
    // Actual conversation metadata and public, authorized shares: no synthetic
    // duplicate SQL rows and no altered runtime discovery cap.
    let mut extra = Vec::new();
    for n in 0..crate::webhooks::application::MAX_CURSORS {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        f.grants
            .create(
                Conversation::new(
                    id.clone(),
                    OrganizationId::new("org").unwrap(),
                    PrincipalId::new("owner").unwrap(),
                    "synthetic".into(),
                    format!("overflow-create-{n}"),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("synthetic").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let shared = client
            .call(
                "conversation.share",
                json!({
                    "requestId":format!("overflow-share-{n}"), "conversationId":id.to_string(),
                    "credentialId":first, "capabilities":["read"]
                }),
            )
            .await;
        assert_eq!(shared["ok"], true, "share {n}: {shared}");
        extra.push(id);
    }
    let mut params = issue_params();
    params["requestId"] = json!("overflow-healthy-issue");
    params["principal"]["id"] = json!("healthy");
    params["membership"]["principalId"] = json!("healthy");
    params["membership"]["id"] = json!("healthy-member");
    let issued = client.call("credential.issue", params).await;
    assert_eq!(issued["ok"], true, "{issued}");
    let second = issued["payload"]["credential"]["id"].as_str().unwrap();
    let shared = client
        .call(
            "conversation.share",
            json!({"requestId":"overflow-healthy-share",
        "conversationId":f.b.to_string(), "credentialId":second, "capabilities":["read"]}),
        )
        .await;
    assert_eq!(shared["ok"], true, "{shared}");
    let mut recipient = Recipient::new(false);
    mount(&f, recipient.transport.clone()).await;
    let healthy = tokio::time::timeout(WAIT, recipient.requests.recv()).await;
    let status = f.runtime.status();
    if healthy.is_err() {
        let removed = client
            .call(
                "conversation.unshare",
                json!({"requestId":"overflow-remove",
            "conversationId":extra.last().unwrap().to_string(), "credentialId":first}),
            )
            .await;
        let revoked = client
            .call(
                "credential.revoke",
                json!({"requestId":"overflow-revoke",
            "credentialId":first}),
            )
            .await;
        drop(client);
        let closed = f.close().await;
        panic!("healthy sibling blocked; status={status:?}; unshare={removed}; revoke={revoked}; cleanup={closed:?}");
    }
    checked_request(&healthy.unwrap().unwrap(), &f.b);
    checked_request(&recipient.next().await, &f.b);
    two_fresh_cycles(&f).await;
    assert_eq!(
        f.runtime.status().last_refusal,
        Some(nessa_protocol::protocol::WebhookRefusal::SourceUnavailable)
    );
    let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let registration = f
        .store
        .registration(CredentialId::new(&first).unwrap())
        .await
        .unwrap()
        .unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM deliveries WHERE registration=?1",
            [registration.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "overflow refuses capture before queueing or sending"
    );
    let cursors: i64 = connection
        .query_row(
            "SELECT count(*) FROM cursors WHERE registration=?1",
            [registration.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cursors, 0, "overflow cannot advance a capture checkpoint");
    assert!(recipient.requests.try_recv().is_err());
    drop(connection);
    assert!(matches!(
        f.grants
            .principal_conversations(
                PrincipalId::new("hook").unwrap(),
                CredentialId::new(&first).unwrap(),
                crate::webhooks::application::MAX_CURSORS
            )
            .await,
        Err(ConversationError::Capacity)
    ));
    // Removing one legitimate excess grant restores exactly the original cap.
    // The same live runtime must freshly discover and deliver the original chat.
    let removed = client
        .call(
            "conversation.unshare",
            json!({"requestId":"overflow-remove",
        "conversationId":extra.last().unwrap().to_string(), "credentialId":first}),
        )
        .await;
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(
        f.grants
            .principal_conversations(
                PrincipalId::new("hook").unwrap(),
                CredentialId::new(&first).unwrap(),
                crate::webhooks::application::MAX_CURSORS
            )
            .await
            .unwrap()
            .len(),
        crate::webhooks::application::MAX_CURSORS
    );
    f.runtime.wake_delivery();
    checked_request(&recipient.next().await, &f.a);
    checked_request(&recipient.next().await, &f.a);
    two_fresh_cycles(&f).await;
    let revoked = client
        .call(
            "credential.revoke",
            json!({"requestId":"overflow-revoke",
        "credentialId":first}),
        )
        .await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    f.clock.0.store(4000, Ordering::SeqCst);
    two_fresh_cycles(&f).await;
    // Only the healthy sibling's retry can cross this fresh due boundary.
    checked_request(&recipient.next().await, &f.b);
    drop(client);
    let closed = f.close().await;
    assert!(closed.is_ok(), "{closed:?}");
    assert!(
        recipient.requests.try_recv().is_err(),
        "no revoked or shutdown POST"
    );
}

#[tokio::test]
async fn malformed_discovery_metadata_still_seals_the_supervisor_without_post() {
    let f = Harness::new().await;
    let (mut client, _) = Client::open(f.address, &f.owner).await;
    let first = issue_share(&f, &mut client).await;
    let connection = Connection::open(f._root.path().join("private/conversations.db")).unwrap();
    connection
        .execute(
            "UPDATE share_grants SET resource_id=?1 WHERE credential_id=?2",
            ["invalid-conversation", &first],
        )
        .unwrap();
    drop(connection);
    let mut recipient = Recipient::new(false);
    mount(&f, recipient.transport.clone()).await;
    tokio::time::timeout(WAIT, async {
        while f.runtime.status().native_mutations
            != nessa_protocol::protocol::WebhookNativeMutations::Stopped
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(recipient.requests.try_recv().is_err());
    let connection = Connection::open(f._root.path().join("private/outbox.db")).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM deliveries", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    drop(client);
    assert!(
        f.close().await.is_err(),
        "corruption is not confirmed cleanup"
    );
}
