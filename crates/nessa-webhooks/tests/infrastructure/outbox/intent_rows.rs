use super::super::codec::RowTooLarge;
use super::*;
use nessa_auth::application::ports::Clock;
use nessa_local_database::rusqlite::Error;
use nessa_local_storage::OpenMode;

struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        10000
    }
}

#[test]
fn oversized_borrowed_row_is_refused_before_constructing_owned_configuration() {
    for extra in [0, ROW_BYTES] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        let issue = format!("{{}}{}", " ".repeat(extra));
        connection.execute("INSERT INTO registration_intents(id,issuer,request,issue,destination,mac_key,observed_at) VALUES(?1,'owner','request',?2,'https://recipient.test/',?3,0)",params![Uuid::new_v4().to_string(),issue,vec![9u8;32]]).unwrap();
        let row = connection.query_row(SELECT, [], Saved::row);
        if extra == 0 {
            assert!(row.is_ok());
        } else {
            assert!(
                matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
            );
        }
    }
}

#[test]
fn registration_schema_guards_are_checked_before_restoration_effects() {
    let mut accepted = Vec::new();
    for variant in ["immutable", "retirement", "foreign_key", "extra_trigger"] {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("outbox.db");
        drop(nessa_local_storage::open(&path, OpenMode::OpenOrCreate).unwrap());
        let connection = Connection::open(&path).unwrap();
        let schema = if variant == "foreign_key" {
            super::super::SCHEMA.replacen(
                "id TEXT PRIMARY KEY REFERENCES registration_intents(id)",
                "id TEXT PRIMARY KEY",
                1,
            )
        } else {
            super::super::SCHEMA.to_owned()
        };
        connection.execute_batch(&schema).unwrap();
        match variant {
            "immutable" => connection
                .execute_batch("DROP TRIGGER registration_intent_immutable")
                .unwrap(),
            "retirement" => connection
                .execute_batch("DROP TRIGGER registration_intent_retire")
                .unwrap(),
            "extra_trigger" => connection
                .execute_batch("CREATE TRIGGER substituted_effect AFTER INSERT ON registration_intents BEGIN DELETE FROM registration_intents WHERE id=NEW.id; END")
                .unwrap(),
            "foreign_key" => (),
            _ => unreachable!(),
        }
        drop(connection);
        match SqlOutbox::open(&path, private.join("audit"), Arc::new(Time)) {
            Ok(_) => accepted.push(variant),
            Err(OutboxError::Invalid) => (),
            Err(other) => panic!("unexpected typed failure: {other:?}"),
        }
    }
    assert!(
        accepted.is_empty(),
        "unprotected schema variants: {accepted:?}"
    );
}
