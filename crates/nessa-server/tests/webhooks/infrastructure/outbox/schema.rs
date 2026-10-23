use super::*;
use crate::webhooks::infrastructure::outbox::codec::RowTooLarge;
use nessa_local_database::rusqlite::Error;

#[test]
fn schema_text_is_bounded_before_owned_definitions_are_constructed() {
    let connection = Connection::open_in_memory().unwrap();
    for bytes in [16, 2 * SCHEMA.len()] {
        let text = "x".repeat(bytes);
        let row = connection.query_row(
            "SELECT 'table','owned','owned',?1",
            [&text],
            SchemaEntry::row,
        );
        if bytes == 16 {
            assert!(row.is_ok());
        } else {
            assert!(
                matches!(row, Err(Error::FromSqlConversionFailure(_,_,source))
                if source.downcast_ref::<RowTooLarge>().is_some())
            );
        }
    }
}

#[test]
fn retained_progress_inventory_covers_the_canonical_tables() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(SCHEMA).unwrap();
    let tables: Vec<String> = definitions(&connection, 1000)
        .unwrap()
        .into_iter()
        .filter(|entry| entry.kind == "table")
        .map(|entry| entry.name)
        .collect();
    // Owners and proof rules are documented together in the design's retained
    // initial-publication table. A new persisted family needs an explicit rule.
    assert_eq!(
        tables,
        [
            "audit",
            "cursors",
            "deliveries",
            "registration_intents",
            "registrations",
            "retirements",
            "source_reset_facts",
            "source_reset_members",
        ]
    );
}
