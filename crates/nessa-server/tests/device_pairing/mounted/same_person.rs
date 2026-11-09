//! Production composition keeps attribution independent of listener/data presence.
use super::*;
use nessa_auth::domain::pairing::ConsentClass;
use nessa_local_database::rusqlite::Connection;

fn namespace(gateway: &Gateway) -> PathBuf {
    gateway.root.path().join("data/ci")
}
fn grants(gateway: &Gateway) -> PathBuf {
    namespace(gateway).join("access-grants/access-grants.sqlite3")
}
fn count(gateway: &Gateway, table: &str) -> i64 {
    assert!(matches!(
        table,
        "person_links" | "person_link_changes" | "share_grants"
    ));
    Connection::open(grants(gateway))
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agentless_gateway_without_native_linking_opens_independent_grants_without_metadata() {
    let mut gateway = Gateway::new(false);
    let server = tokio::task::block_in_place(|| gateway.start());
    assert!(grants(&gateway).is_file());
    // Existing environment-audit composition creates conversations/audit.
    // Attribution must create neither metadata nor a colocated grant file.
    assert!(!namespace(&gateway)
        .join("conversations/metadata.sqlite3")
        .exists());
    assert!(!namespace(&gateway)
        .join("conversations/metadata.grants.sqlite3")
        .exists());
    assert!(!gateway.private_directory().exists());
    assert_eq!(count(&gateway, "person_links"), 0);
    assert_eq!(count(&gateway, "share_grants"), 0);
    assert!(tokio::task::block_in_place(|| server.stop()).success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agentless_gateway_refuses_foreign_grants_before_creating_conversation_data() {
    let mut gateway = Gateway::new(false);
    let path = grants(&gateway);
    nessa_local_storage::create_directory(path.parent().unwrap()).unwrap();
    let schema = nessa_local_database::Schema::new(
        "CREATE TABLE preserved (value TEXT);\nPRAGMA user_version = 99;",
    )
    .unwrap();
    let connection = nessa_local_database::open(&path, &schema).unwrap();
    connection
        .execute("INSERT INTO preserved VALUES ('keep')", [])
        .unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    let status = tokio::task::block_in_place(|| gateway.start_until_exit());
    assert!(!status.success());
    let log = std::fs::read_to_string(gateway.root.path().join("server.log")).unwrap();
    assert!(log.contains("access grants"), "{log}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!namespace(&gateway).join("conversations").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_native_linking_keeps_startup_repair_and_live_credential_unlink() {
    let mut gateway = Gateway::new(true);
    let server = tokio::task::block_in_place(|| gateway.start());
    let connection = Connection::open(grants(&gateway)).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_link BEFORE INSERT ON person_links BEGIN SELECT RAISE(ABORT,'unavailable'); END;").unwrap();
    let (created, person) = tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        let person = owner.ok("auth.session", json!({}))["principalId"]
            .as_str()
            .unwrap()
            .to_owned();
        (
            owner.ok(
                "pairing.create",
                json!({"enrollee":"gateway", "samePerson":true}),
            ),
            person,
        )
    });
    let id = invitation(&created["status"]);
    let client_root = tempfile::tempdir().unwrap();
    let (_, store) = pending(client_root.path(), "peer");
    let client = NativeEnrollmentClient::enrolling(
        ConsentClass::PeerRead,
        store.clone(),
        RuntimeDependencies::default().clock,
    );
    let code = ManualCode::parse(created["code"].as_str().unwrap().as_bytes()).unwrap();
    let claimed = tokio::time::timeout(
        WAIT,
        client.enroll(
            TcpStream::connect(gateway.native.unwrap()).unwrap(),
            code,
            OsEntropy,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(claimed, NativePairingStatus::Claimed(_)));
    assert!(claimed.public().same_person());
    let (credential, principal) = tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        let status = owner.ok("pairing.status", json!({"invitationId":id}));
        let approved = owner.ok(
            "pairing.approve",
            json!({"invitationId":id,"deviceKey":status["claimedDeviceKey"]}),
        );
        assert_eq!(approved["status"]["phase"], "active", "{approved}");
        assert!(approved.get("activationStopped").is_some(), "{approved}");
        let credential = approved["status"]["credentialId"]
            .as_str()
            .unwrap()
            .to_owned();
        let credentials = owner.ok("credential.list", json!({}));
        let principal = credentials["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == credential)
            .unwrap()["principalId"]
            .as_str()
            .unwrap()
            .to_owned();
        (credential, principal)
    });
    assert_ne!(person, principal);
    assert_eq!(count(&gateway, "person_links"), 0);
    assert_eq!(count(&gateway, "person_link_changes"), 0);
    assert!(tokio::task::block_in_place(|| server.stop()).success());
    connection.execute_batch("DROP TRIGGER fail_link").unwrap();
    std::fs::write(namespace(&gateway).join("config.json"), b"{}").unwrap();
    gateway.native_enabled = false;
    gateway.native = None;
    let server = tokio::task::block_in_place(|| gateway.start());
    assert!(!namespace(&gateway)
        .join("conversations/metadata.sqlite3")
        .exists());
    assert!(!namespace(&gateway)
        .join("conversations/metadata.grants.sqlite3")
        .exists());
    let linked: (String, String, String) = connection
        .query_row(
            "SELECT person_principal,gateway_principal,credential_id FROM person_links",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(linked, (person.clone(), principal, credential.clone()));
    assert_eq!(count(&gateway, "person_link_changes"), 1);
    tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        assert_eq!(
            owner.refused("pairing.create", json!({})),
            "pairing_not_configured"
        );
        owner.ok(
            "credential.revoke",
            json!({"requestId":"revoke-while-native-off","credentialId":credential}),
        );
    });
    assert_eq!(count(&gateway, "person_links"), 0);
    assert_eq!(count(&gateway, "share_grants"), 0);
    let removed: (String, String) = connection
        .query_row(
            "SELECT cause,initiator_id FROM person_link_changes ORDER BY sequence DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(removed, ("credential_revoked".into(), person));
    assert_eq!(count(&gateway, "person_link_changes"), 2);
    assert!(tokio::task::block_in_place(|| server.stop()).success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_composed_gateways_retain_the_choice_through_peer_activation_and_restart() {
    let mut owner_gateway = Gateway::new(true);
    let mut peer = Gateway::new(true);
    let server = tokio::task::block_in_place(|| owner_gateway.start());
    let peer_server = tokio::task::block_in_place(|| peer.start());
    let peer_directory = namespace(&peer).join("peer-gateways");
    let (credential, person, principal, id) = tokio::task::block_in_place(|| {
        let mut owner = owner_gateway.owner();
        let mut peer_owner = peer.owner();
        let person = owner.ok("auth.session", json!({}))["principalId"]
            .as_str()
            .unwrap()
            .to_owned();
        let created = owner.ok(
            "pairing.create",
            json!({"enrollee":"gateway","samePerson":true}),
        );
        assert_eq!(created["status"]["samePerson"], true);
        let id = invitation(&created["status"]);
        peer_owner.ok(
            "peer.enroll",
            json!({"address":owner_gateway.native.unwrap().to_string(),"code":created["code"]}),
        );
        let status = owner.ok(
            "pairing.status",
            json!({"invitationId":invitation(&created["status"])}),
        );
        let approved = owner.ok("pairing.approve", json!({"invitationId":invitation(&created["status"]),"deviceKey":status["claimedDeviceKey"]}));
        assert_eq!(approved["status"]["phase"], "active", "{approved}");
        assert_eq!(approved["status"]["samePerson"], true);
        assert!(approved.get("activationStopped").is_none(), "{approved}");
        let credential = approved["status"]["credentialId"]
            .as_str()
            .unwrap()
            .to_owned();
        let credentials = owner.ok("credential.list", json!({}));
        let principal = credentials["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == credential)
            .unwrap()["principalId"]
            .as_str()
            .unwrap()
            .to_owned();
        let deadline = Instant::now() + WAIT;
        loop {
            let listed = peer_owner.ok("peer.list", json!({}));
            if listed["items"][0]["phase"] == "active" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "peer activation keeps its exact choice: {listed}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        (credential, person, principal, id)
    });
    let record_path = std::fs::read_dir(&peer_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let record: Value = serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["samePerson"], true);
    assert_eq!(record["credential"]["credentialId"], credential);
    let linked: (String, String, String) = Connection::open(grants(&owner_gateway))
        .unwrap()
        .query_row(
            "SELECT person_principal,gateway_principal,credential_id FROM person_links",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(linked, (person, principal, credential.clone()));
    assert_eq!(count(&owner_gateway, "share_grants"), 0);
    assert!(tokio::task::block_in_place(|| peer_server.stop()).success());
    let peer_server = tokio::task::block_in_place(|| peer.start());
    tokio::task::block_in_place(|| {
        let listed = peer.owner().ok("peer.list", json!({}));
        assert_eq!(listed["items"][0]["phase"], "active", "{listed}");
    });
    let restored: Value = serde_json::from_slice(&std::fs::read(record_path).unwrap()).unwrap();
    assert_eq!(restored["samePerson"], true);
    assert_eq!(restored["credential"]["credentialId"], credential);
    assert!(tokio::task::block_in_place(|| peer_server.stop()).success());
    assert!(tokio::task::block_in_place(|| server.stop()).success());
    let server = tokio::task::block_in_place(|| owner_gateway.start());
    tokio::task::block_in_place(|| {
        let status = owner_gateway
            .owner()
            .ok("pairing.status", json!({"invitationId":id}));
        assert_eq!(status["phase"], "active");
        assert_eq!(status["samePerson"], true);
        assert_eq!(status["credentialId"], credential);
    });
    assert!(tokio::task::block_in_place(|| server.stop()).success());
}
