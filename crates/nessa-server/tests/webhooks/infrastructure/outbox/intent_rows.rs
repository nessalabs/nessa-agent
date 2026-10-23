use super::*;

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
                matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<IntentRowTooLarge>().is_some())
            );
        }
    }
}
