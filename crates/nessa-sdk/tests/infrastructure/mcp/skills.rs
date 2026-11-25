//! End-to-end tests for Skills over MCP (SEP-2640): client discovery,
//! manifest-bound verified reads, directory reading, capability gating, and
//! stand-in passthrough.
use super::super::{handle_skill_method, insert_server_skills_capabilities, McpError};
use super::{session, Behaviour, Harness};
use crate::domain::mcp_skills::{
    SkillCatalog, SkillDefinition, SkillFile, SkillFileUri, SkillUri, SkillsCapability, EXTENSION,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

const REVIEW_SKILL: &str = "---\n\
name: code-review\n\
description: Review code using the team checklist.\n\
---\n\n\
# Code review\n\n\
Read `references/checklist.md`.\n";

const REVIEW_CHECKLIST: &str = "# Review checklist\nCheck invariants and tests.\n";

fn test_catalog() -> Arc<SkillCatalog> {
    Arc::new(
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: "code-review".to_owned(),
            files: vec![
                SkillFile::markdown("SKILL.md", REVIEW_SKILL),
                SkillFile::markdown("references/checklist.md", REVIEW_CHECKLIST),
            ],
        }])
        .unwrap(),
    )
}

fn skills_behaviour(catalog: Arc<SkillCatalog>, directory_read: bool) -> Behaviour {
    Behaviour {
        version: Some("2026-07-28"),
        on_request: Some(Arc::new(move |request| {
            let method = request["method"].as_str().unwrap_or_default();
            let id = request["id"].clone();
            if method == "initialize" {
                let mut caps = Map::new();
                caps.insert("tools".to_owned(), json!({}));
                if directory_read {
                    insert_server_skills_capabilities(&mut caps);
                } else {
                    caps.insert("resources".to_owned(), json!({}));
                    caps.insert("extensions".to_owned(), json!({ (EXTENSION): {} }));
                }
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2026-07-28",
                        "capabilities": Value::Object(caps),
                        "serverInfo": { "name": "skills-fixture", "version": "1" },
                    }
                }));
            }
            let params = request.get("params").unwrap_or(&Value::Null);
            handle_skill_method(&catalog, method, params).map(|handled| match handled {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
                }
            })
        })),
        ..Behaviour::default()
    }
}

#[tokio::test]
async fn client_discovers_verifies_and_navigates_skills_over_mcp() {
    let catalog = test_catalog();
    let (session, servers, launcher, _) = session(skills_behaviour(catalog, true)).await;

    assert_eq!(
        session.skills_capability(),
        Some(SkillsCapability {
            directory_read: true
        })
    );

    let listed = session.list_skills().await.unwrap();
    assert_eq!(listed.len(), 1);
    let entry = &listed[0];
    assert_eq!(entry.uri().as_str(), "skill://code-review/SKILL.md");
    assert_eq!(entry.frontmatter().name().as_str(), "code-review");

    // Direct lookup via skills/get matches listing.
    let fetched = session.get_skill(entry.uri()).await.unwrap();
    assert_eq!(&fetched, entry);

    // Verified read of SKILL.md and supporting file succeeds.
    let skill_md = session
        .read_verified_skill_file(entry, &SkillFileUri::from(entry.uri()))
        .await
        .unwrap();
    assert_eq!(skill_md.as_utf8(), Some(REVIEW_SKILL));
    assert_eq!(skill_md.mime_type, "text/markdown");

    let ref_uri = entry
        .uri()
        .resolve_relative("references/checklist.md")
        .unwrap();
    let ref_file = session
        .read_verified_skill_file(entry, &ref_uri)
        .await
        .unwrap();
    assert_eq!(ref_file.as_utf8(), Some(REVIEW_CHECKLIST));

    // Unlisted URI is refused before sending resources/read.
    let before = launcher.server(0).with_method("resources/read").len();
    let unlisted = entry
        .uri()
        .resolve_relative("references/secret.md")
        .unwrap();
    assert!(matches!(
        session.read_verified_skill_file(entry, &unlisted).await,
        Err(McpError::Malformed(_))
    ));
    assert_eq!(
        launcher.server(0).with_method("resources/read").len(),
        before
    );

    // Directory reading lists direct children and rejects trailing slash.
    let root = session
        .read_skill_directory("skill://code-review")
        .await
        .unwrap();
    assert_eq!(root.len(), 2);
    let refs = session
        .read_skill_directory("skill://code-review/references")
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "checklist.md");
    assert!(matches!(
        session.read_skill_directory("skill://code-review/").await,
        Err(McpError::Malformed(_))
    ));

    // Unknown skill URI returns JSON-RPC -32602.
    let missing = SkillUri::new("skill://missing/SKILL.md").unwrap();
    assert!(matches!(
        session.get_skill(&missing).await,
        Err(McpError::Remote { code: -32602, .. })
    ));

    servers.stop().await;
}

#[tokio::test]
async fn client_gates_calls_on_capabilities_and_rejects_tampered_reads() {
    // Server without skills extension: refuses before sending skills/list or skills/get.
    let (plain_session, plain_servers, plain_launcher, _) = session(Behaviour::default()).await;
    assert_eq!(plain_session.skills_capability(), None);
    assert!(matches!(
        plain_session.list_skills().await,
        Err(McpError::Malformed(_))
    ));
    assert!(plain_launcher
        .server(0)
        .with_method("skills/list")
        .is_empty());
    plain_servers.stop().await;

    // Server with directoryRead: false refuses resources/directory/read before sending.
    let (no_dir_session, no_dir_servers, no_dir_launcher, _) =
        session(skills_behaviour(test_catalog(), false)).await;
    assert_eq!(
        no_dir_session.skills_capability(),
        Some(SkillsCapability {
            directory_read: false
        })
    );
    assert!(matches!(
        no_dir_session
            .read_skill_directory("skill://code-review")
            .await,
        Err(McpError::Malformed(_))
    ));
    assert!(no_dir_launcher
        .server(0)
        .with_method("resources/directory/read")
        .is_empty());
    no_dir_servers.stop().await;

    // Server serving tampered content on resources/read is rejected by digest verification.
    let catalog = test_catalog();
    let tampered = Behaviour {
        version: Some("2026-07-28"),
        on_request: Some(Arc::new(move |request| {
            let method = request["method"].as_str().unwrap_or_default();
            let id = request["id"].clone();
            if method == "initialize" {
                let mut caps = Map::new();
                caps.insert("tools".to_owned(), json!({}));
                insert_server_skills_capabilities(&mut caps);
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2026-07-28",
                        "capabilities": Value::Object(caps),
                    }
                }));
            }
            if method == "resources/read" {
                let mut body = REVIEW_CHECKLIST.as_bytes().to_vec();
                body[0] = b'!';
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "resultType": "complete",
                        "contents": [{
                            "uri": "skill://code-review/references/checklist.md",
                            "mimeType": "text/markdown",
                            "text": String::from_utf8(body).unwrap(),
                        }],
                    }
                }));
            }
            let params = request.get("params").unwrap_or(&Value::Null);
            handle_skill_method(&catalog, method, params).map(|handled| match handled {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
                }
            })
        })),
        ..Behaviour::default()
    };
    let (tampered_session, tampered_servers, _, _) = session(tampered).await;
    let entry = &tampered_session.list_skills().await.unwrap()[0];
    let ref_uri = entry
        .uri()
        .resolve_relative("references/checklist.md")
        .unwrap();
    assert!(matches!(
        tampered_session
            .read_verified_skill_file(entry, &ref_uri)
            .await,
        Err(McpError::Malformed(_))
    ));
    tampered_servers.stop().await;
}

#[tokio::test]
async fn stand_in_forwards_skills_capabilities_and_methods_to_harness() {
    let (session, servers, _, _) = session(skills_behaviour(test_catalog(), true)).await;
    let mut harness = Harness::attach(session);

    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2026-07-28", "capabilities": {} }
        }))
        .await;
    let init = harness.next().await.unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(
        init["result"]["capabilities"]["extensions"][EXTENSION]["directoryRead"],
        json!(true)
    );

    harness
        .send(json!({ "jsonrpc": "2.0", "id": 2, "method": "skills/list", "params": {} }))
        .await;
    let listed = harness.next().await.unwrap();
    assert_eq!(
        listed["result"]["skills"][0]["uri"],
        "skill://code-review/SKILL.md"
    );

    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": { "uri": "skill://code-review/SKILL.md" }
        }))
        .await;
    let read = harness.next().await.unwrap();
    assert_eq!(read["result"]["contents"][0]["text"], REVIEW_SKILL);

    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "resources/directory/read",
            "params": { "uri": "skill://code-review/references" }
        }))
        .await;
    let dir = harness.next().await.unwrap();
    assert_eq!(dir["result"]["resources"][0]["name"], "checklist.md");

    drop(harness);
    servers.stop().await;
}
