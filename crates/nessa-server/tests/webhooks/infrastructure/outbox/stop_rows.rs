//! Borrowed decoding budgets include nested stop records and joined offsets.
use super::*;
use crate::webhooks::infrastructure::outbox::{
    codec::RowTooLarge, publication::SavedAudit, registry::StoredRegistration, stop::SavedStop,
};
use crate::webhooks::{application::RegistrationStop, domain::ExpiryObserver};
use nessa_auth::domain::CredentialId;
use nessa_local_database::rusqlite::{Error, StatementStatus};

#[test]
fn stop_rows_are_bounded_before_owned_allocation() {
    let connection = Connection::open_in_memory().unwrap();
    let mut largest_stop =
        SavedStop::of(&RegistrationStop::expired(ExpiryObserver::Restart, 10001))
            .encode()
            .unwrap();
    largest_stop.push_str(&" ".repeat(super::super::audit::AUDIT_BYTES - largest_stop.len()));
    let oversized = "x".repeat(super::super::audit::AUDIT_BYTES + 4096);
    for offset in [0, 11] {
        // Joined delivery rows start the registration after eleven columns.
        let prefix = "0,".repeat(offset);
        let query = format!("SELECT {prefix}'id','credential','principal','organization','owner','https://recipient.test/',zeroblob(32),0,?1,10000");
        let row =
            connection.query_row(&query, [&oversized], |r| StoredRegistration::row(r, offset));
        assert!(
            matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
        );
        let row = connection.query_row(&query, ["{}"], |r| StoredRegistration::row(r, offset));
        assert!(row.is_ok()); // Semantic validation remains a separate owner.
        let query = format!("SELECT {prefix}'{}','credential','principal','organization','owner','https://recipient.test/',zeroblob(32),0,?1,10000",Uuid::new_v4());
        connection
            .query_row(&query, [&largest_stop], |r| {
                StoredRegistration::row(r, offset)
            })
            .unwrap()
            .checked()
            .unwrap();
    }
    let row = connection.query_row(
        "SELECT 'id','registration',NULL,?1,0,NULL",
        [&oversized],
        SavedAudit::row,
    );
    assert!(
        matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
    );
    let row = connection.query_row(
        "SELECT 'id','registration',NULL,?1,0,NULL",
        ["{}"],
        SavedAudit::row,
    );
    assert!(row.is_ok());
    let row = connection.query_row(
        "SELECT 'registration','credential','principal','organization','owner',?1,10001,'audit',10000,'drained'",
        [&oversized],
        SavedRetirement::row,
    );
    assert!(
        matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
    );
    let row = connection.query_row(
        "SELECT 'registration','credential','principal','organization','owner',?1,10001,'audit',10000,'drained'",
        ["{}"],
        SavedRetirement::row,
    );
    assert!(row.is_ok());
    connection
        .query_row(
            "SELECT ?1,'credential','principal','organization','owner',?2,10001,?3,10000,'drained'",
            params![
                Uuid::new_v4().to_string(),
                largest_stop,
                Uuid::new_v4().to_string()
            ],
            SavedRetirement::row,
        )
        .unwrap()
        .checked()
        .unwrap();
}

#[test]
fn retained_credential_lookup_is_indexed_and_refuses_ambiguous_ownership() {
    let mut baseline = None;
    for count in [1, MAX_REGISTRATIONS] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        // Query metadata only: these rows claim neither actual Auth provenance
        // nor a complete restorable audit history. The production schema/index
        // and production lookup are exercised at the retained count ceiling.
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        let stop = SavedStop::of(&RegistrationStop::expired(ExpiryObserver::Restart, 10001))
            .encode()
            .unwrap();
        for n in 0..count {
            connection.execute("INSERT INTO retirements(id,audit,credential,principal,organization,owner,issued_at,stop,retired_at) VALUES(?1,?2,?3,'principal','organization','owner',10000,?4,10001)",params![Uuid::new_v4().to_string(),Uuid::new_v4().to_string(),format!("credential-{n}"),stop]).unwrap();
        }
        let missing = CredentialId::new("missing").unwrap();
        assert!(by_credential(&connection, &missing).unwrap().is_none());
        let target = CredentialId::new(format!("credential-{}", count - 1)).unwrap();
        assert_eq!(
            by_credential(&connection, &target)
                .unwrap()
                .unwrap()
                .target
                .credential,
            target
        );
        let mut query = connection.prepare(&credential_query()).unwrap();
        assert!(query
            .query([missing.as_str()])
            .unwrap()
            .next()
            .unwrap()
            .is_none());
        assert_eq!(query.get_status(StatementStatus::FullscanStep), 0);
        let steps = query.get_status(StatementStatus::VmStep);
        assert!(steps < 32, "steps={steps}");
        if let Some(previous) = baseline {
            assert_eq!(steps, previous);
        }
        baseline = Some(steps);
        drop(query);
        connection.execute("INSERT INTO retirements(id,audit,credential,principal,organization,owner,issued_at,stop,retired_at) SELECT ?1,?2,credential,principal,organization,owner,issued_at,stop,retired_at FROM retirements WHERE credential=?3",params![Uuid::new_v4().to_string(),Uuid::new_v4().to_string(),target.as_str()]).unwrap();
        assert!(matches!(
            by_credential(&connection, &target),
            Err(OutboxError::Invalid)
        ));
    }
}

#[test]
fn retirement_identity_is_bounded_in_every_shared_row_read() {
    let connection = Connection::open_in_memory().unwrap();
    let oversized = "x".repeat(1024 * 1024);
    let stop = SavedStop::of(&RegistrationStop::expired(ExpiryObserver::Restart, 10001))
        .encode()
        .unwrap();
    let query =
        "SELECT ?1,'credential','principal','organization','owner',?2,10001,?3,10000,'drained'";
    let audit = Uuid::new_v4().to_string();
    let row = connection.query_row(query, params![oversized, stop, audit], SavedRetirement::row);
    assert!(
        matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
    );
    let id = Uuid::new_v4();
    let facts = connection
        .query_row(
            query,
            params![id.to_string(), stop, audit],
            SavedRetirement::row,
        )
        .unwrap()
        .checked()
        .unwrap();
    assert_eq!(facts.registration, id);
    assert_eq!(facts.audit.to_string(), audit);
}

#[test]
fn retirement_kind_is_closed_and_bounded_before_owned_allocation() {
    let connection = Connection::open_in_memory().unwrap();
    let stop = SavedStop::of(&RegistrationStop::expired(ExpiryObserver::Restart, 10001))
        .encode()
        .unwrap();
    let id = Uuid::new_v4().to_string();
    let audit = Uuid::new_v4().to_string();
    let query = "SELECT ?1,'credential','principal','organization','owner',?2,10001,?3,10000,?4";
    for kind in ["drained", "cancelled", "unsupported"] {
        let saved = connection
            .query_row(query, params![id, stop, audit, kind], SavedRetirement::row)
            .unwrap();
        if kind == "unsupported" {
            assert!(matches!(saved.checked(), Err(OutboxError::Invalid)));
        } else {
            saved.checked().unwrap();
        }
    }
    let oversized = "x".repeat(1024 * 1024);
    let row = connection.query_row(
        query,
        params![id, stop, audit, oversized],
        SavedRetirement::row,
    );
    assert!(
        matches!(row, Err(Error::FromSqlConversionFailure(_,_,source)) if source.downcast_ref::<RowTooLarge>().is_some())
    );
}
