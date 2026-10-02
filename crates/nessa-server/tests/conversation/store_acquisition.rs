//! Actual SQLite projections refuse oversized text before selecting its bytes.
use super::{conversation, summary, text};
use crate::agents::domain::AgentId;
use crate::conversation::domain::{
    Conversation, ConversationModelId, ConversationPreview, ConversationTitle,
};
use nessa_auth::domain::MAX_IDENTIFIER_BYTES;
use nessa_local_database::rusqlite::{params, types::Value, Connection};
use nessa_sync::replication::domain::MAX_ID_BYTES;

#[test]
fn acquisition_admits_full_multibyte_text_in_utf8_and_utf16_without_selecting_oversize() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "encoding", encoding)
            .unwrap();
        connection
            .execute("CREATE TABLE saved (value TEXT)", [])
            .unwrap();
        let ceiling = MAX_IDENTIFIER_BYTES;
        for value in [
            "a".repeat(ceiling),
            "é".repeat(ceiling / 2),
            "😀".repeat(ceiling / 4),
        ] {
            connection.execute("DELETE FROM saved", []).unwrap();
            connection
                .execute("INSERT INTO saved VALUES (?1)", [&value])
                .unwrap();
            let selected: String = connection
                .query_row(
                    &format!("SELECT {} FROM saved", text("value", ceiling, false)),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(selected, value);
        }
        connection
            .execute("UPDATE saved SET value = ?1", ["x".repeat(ceiling * 3)])
            .unwrap();
        let selected: Value = connection
            .query_row(
                &format!("SELECT {} FROM saved", text("value", ceiling, false)),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(selected, Value::Blob(vec![]));
    }
}

#[test]
fn required_types_and_nullable_summary_use_one_projection() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute(
            "CREATE TABLE saved (title, preview, updated_at_ms, archived)",
            [],
        )
        .unwrap();
    connection
        .execute("INSERT INTO saved VALUES (NULL, NULL, 1, 0)", [])
        .unwrap();
    let selected: (Option<String>, Option<String>) = connection
        .query_row(&format!("SELECT {} FROM saved", summary("")), [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(selected, (None, None));
    connection
        .execute(
            "UPDATE saved SET title = ?1, preview = ?2",
            params![
                "😀".repeat(ConversationTitle::MAX_CHARS),
                "é".repeat(ConversationPreview::MAX_BYTES / 2)
            ],
        )
        .unwrap();
    let selected: (String, String) = connection
        .query_row(&format!("SELECT {} FROM saved", summary("")), [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(
        selected.0.len(),
        ConversationTitle::MAX_CHARS * char::MAX.len_utf8()
    );
    assert_eq!(selected.1.len(), ConversationPreview::MAX_BYTES);
    connection
        .execute("UPDATE saved SET title = X'00'", [])
        .unwrap();
    assert!(connection
        .query_row(&format!("SELECT {} FROM saved", summary("")), [], |row| row
            .get::<_, Option<String>>(0))
        .is_err());
}

#[test]
fn conversation_projection_consumes_supported_name_owner_and_compacts_unknown() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute("CREATE TABLE saved (id, organization, owner, creator_surface, creation_action, creation_requested_at_ms, agent, model, approval_mode)", []).unwrap();
    connection
        .execute(
            "INSERT INTO saved VALUES (?1, ?2, ?2, ?3, ?3, 1, 'claude', ?4, 'full')",
            params![
                "é".repeat(MAX_ID_BYTES / 2),
                "é".repeat(MAX_IDENTIFIER_BYTES / 2),
                "é".repeat(Conversation::MAX_CREATOR_CONTEXT_BYTES / 2),
                "é".repeat(ConversationModelId::MAX_BYTES / 2)
            ],
        )
        .unwrap();
    for agent in AgentId::ALL {
        connection
            .execute("UPDATE saved SET agent = ?1", [agent.name()])
            .unwrap();
        let selected: Option<String> = connection
            .query_row(
                &format!("SELECT {} FROM saved", conversation("")),
                [],
                |row| row.get(6),
            )
            .unwrap();
        assert_eq!(selected.as_deref().and_then(AgentId::parse), Some(*agent));
    }
    connection
        .execute("UPDATE saved SET agent = ?1", ["unknown".repeat(1_000_000)])
        .unwrap();
    let selected: Option<String> = connection
        .query_row(
            &format!("SELECT {} FROM saved", conversation("")),
            [],
            |row| row.get(6),
        )
        .unwrap();
    assert_eq!(selected, None);
    connection
        .execute("UPDATE saved SET agent = X'00'", [])
        .unwrap();
    assert!(connection
        .query_row(
            &format!("SELECT {} FROM saved", conversation("")),
            [],
            |row| row.get::<_, Option<String>>(6)
        )
        .is_err());
}
