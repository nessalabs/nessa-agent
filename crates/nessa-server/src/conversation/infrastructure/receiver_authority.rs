//! Durable local authority for paired receiver identity and access epochs.

use crate::conversation::application::{ReadRefusal, ReceiverAuthority, ReceiverBinding};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension, TransactionBehavior},
    OpenError, Schema,
};
use std::{
    collections::HashMap,
    future::Future,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex},
};

const DEFINITION: &str = include_str!("receiver_authority.sql");

#[derive(Clone)]
pub struct LocalReceiverAuthority {
    connection: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverChangeError {
    Unavailable,
    Conflict,
    Missing,
    Exhausted,
}

impl LocalReceiverAuthority {
    pub fn open(
        path: &Path,
        policy_revision: &str,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, OpenError> {
        let mut connection = nessa_local_database::open(path, &Schema::new(DEFINITION)?)?;
        validate_history(&connection).map_err(OpenError::Unreadable)?;
        let observed_at_ms = i64::try_from(clock.unix_milliseconds()).map_err(|_| {
            OpenError::Database(nessa_local_database::rusqlite::Error::InvalidQuery)
        })?;
        advance_policy(&mut connection, policy_revision, observed_at_ms)
            .map_err(OpenError::Database)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            clock,
        })
    }

    /// Pair a server-minted receiver with a verified credential. The caller of
    /// this transition must be the trusted pairing use case, not a socket DTO.
    pub async fn pair(
        &self,
        credential_id: CredentialId,
        organization_id: OrganizationId,
        owner_id: PrincipalId,
        initiator_id: PrincipalId,
        request_id: String,
    ) -> Result<ReceiverBinding, ReceiverChangeError> {
        if request_id.trim().is_empty() || request_id.len() > 200 {
            return Err(ReceiverChangeError::Conflict);
        }
        let receiver_id = format!("receiver-{}", uuid::Uuid::new_v4());
        let observed_at_ms = i64::try_from(self.clock.unix_milliseconds())
            .map_err(|_| ReceiverChangeError::Exhausted)?;
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| ReceiverChangeError::Unavailable)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| ReceiverChangeError::Unavailable)?;
            let result = transaction.execute(
                "INSERT INTO receivers VALUES (?1, ?2, ?3, ?4, 1, 1)",
                params![receiver_id, credential_id.as_str(), organization_id.as_str(), owner_id.as_str()],
            );
            if result.is_err() { return Err(ReceiverChangeError::Conflict); }
            transaction.execute(
                "INSERT INTO receiver_transitions (receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_id, request_id, observed_at_ms) VALUES (?1, ?2, ?3, NULL, 1, NULL, 1, NULL, ?4, 'paired', ?5, ?6, ?7)",
                params![receiver_id, organization_id.as_str(), owner_id.as_str(), credential_id.as_str(), initiator_id.as_str(), request_id, observed_at_ms],
            ).map_err(|_| ReceiverChangeError::Conflict)?;
            transaction.commit().map_err(|_| ReceiverChangeError::Unavailable)?;
            Ok(ReceiverBinding { receiver_id, credential_id, organization_id, owner_id, access_epoch: 1, active: true })
        }).await.map_err(|_| ReceiverChangeError::Unavailable)?
    }

    /// Revoke or regrant a known receiver. Each committed transition increments
    /// its epoch; a retry with the same request ID cannot increment it twice.
    pub async fn change(
        &self,
        receiver_id: String,
        replacement_credential: Option<CredentialId>,
        active: bool,
        initiator_id: PrincipalId,
        request_id: String,
    ) -> Result<ReceiverBinding, ReceiverChangeError> {
        if request_id.trim().is_empty() || request_id.len() > 200 {
            return Err(ReceiverChangeError::Conflict);
        }
        let observed_at_ms = i64::try_from(self.clock.unix_milliseconds())
            .map_err(|_| ReceiverChangeError::Exhausted)?;
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| ReceiverChangeError::Unavailable)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| ReceiverChangeError::Unavailable)?;
            let previous = load_by_receiver(&transaction, &receiver_id)?.ok_or(ReceiverChangeError::Missing)?;
            if !active && replacement_credential.is_some() {
                return Err(ReceiverChangeError::Conflict);
            }
            if active && replacement_credential.is_none() {
                return Err(ReceiverChangeError::Conflict);
            }
            if previous.active == active {
                return Err(ReceiverChangeError::Conflict);
            }
            if active && replacement_credential.as_ref() == Some(&previous.credential_id) {
                return Err(ReceiverChangeError::Conflict);
            }
            let prior_request: Option<String> = transaction.query_row(
                "SELECT receiver_id FROM receiver_transitions WHERE initiator_id = ?1 AND request_id = ?2",
                params![initiator_id.as_str(), request_id], |row| row.get(0),
            ).optional().map_err(|_| ReceiverChangeError::Unavailable)?;
            if prior_request.is_some() { return Err(ReceiverChangeError::Conflict); }
            let epoch = previous.access_epoch.checked_add(1).ok_or(ReceiverChangeError::Exhausted)?;
            let stored_epoch = i64::try_from(epoch).map_err(|_| ReceiverChangeError::Exhausted)?;
            let credential_id = replacement_credential.unwrap_or(previous.credential_id.clone());
            transaction.execute(
                "UPDATE receivers SET credential_id = ?2, access_epoch = ?3, active = ?4 WHERE receiver_id = ?1",
                params![receiver_id, credential_id.as_str(), stored_epoch, active],
            ).map_err(|_| ReceiverChangeError::Conflict)?;
            transaction.execute(
                "INSERT INTO receiver_transitions (receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_id, request_id, observed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![receiver_id, previous.organization_id.as_str(), previous.owner_id.as_str(), i64::try_from(previous.access_epoch).map_err(|_| ReceiverChangeError::Exhausted)?, stored_epoch, previous.active, active,
                    previous.credential_id.as_str(), credential_id.as_str(), if active { "regranted" } else { "revoked" }, initiator_id.as_str(), request_id, observed_at_ms],
            ).map_err(|_| ReceiverChangeError::Unavailable)?;
            transaction.commit().map_err(|_| ReceiverChangeError::Unavailable)?;
            Ok(ReceiverBinding { credential_id, access_epoch: epoch, active, ..previous })
        }).await.map_err(|_| ReceiverChangeError::Unavailable)?
    }
}

/// Reconstruct every receiver's epoch, credential, and active state from
/// append-only evidence before a stored row can authorize a read.
fn validate_history(connection: &Connection) -> nessa_local_database::rusqlite::Result<()> {
    type State = (i64, i64, String, String, String);
    let invalid = || nessa_local_database::rusqlite::Error::InvalidQuery;
    let mut states: HashMap<String, State> = HashMap::new();
    let mut query = connection.prepare(
        "SELECT sequence, receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_id, request_id, observed_at_ms FROM receiver_transitions ORDER BY sequence",
    )?;
    let mut rows = query.query([])?;
    let mut sequence = 0_i64;
    while let Some(row) = rows.next()? {
        sequence = sequence.checked_add(1).ok_or_else(invalid)?;
        let stored_sequence: i64 = row.get(0)?;
        let receiver_id: String = row.get(1)?;
        let organization_id: String = row.get(2)?;
        let owner_id: String = row.get(3)?;
        let before_epoch: Option<i64> = row.get(4)?;
        let after_epoch: i64 = row.get(5)?;
        let before_active: Option<i64> = row.get(6)?;
        let after_active: i64 = row.get(7)?;
        let before_credential: Option<String> = row.get(8)?;
        let after_credential: String = row.get(9)?;
        let cause: String = row.get(10)?;
        let initiator: String = row.get(11)?;
        let request_id: String = row.get(12)?;
        let observed_at_ms: i64 = row.get(13)?;
        if stored_sequence != sequence
            || receiver_id.is_empty()
            || receiver_id.len() > 128
            || initiator.is_empty()
            || request_id.is_empty()
            || request_id.len() > 200
            || observed_at_ms < 0
            || CredentialId::new(after_credential.clone()).is_err()
            || OrganizationId::new(organization_id.clone()).is_err()
            || PrincipalId::new(owner_id.clone()).is_err()
            || (initiator != "system" && PrincipalId::new(initiator.clone()).is_err())
        {
            return Err(invalid());
        }
        match states.get(&receiver_id) {
            None if cause == "paired"
                && before_epoch.is_none()
                && before_active.is_none()
                && before_credential.is_none()
                && after_epoch == 1
                && after_active == 1
                && initiator != "system" => {}
            Some((epoch, active, credential, organization, owner))
                if cause != "paired"
                    && *organization == organization_id
                    && *owner == owner_id
                    && before_epoch == Some(*epoch)
                    && before_active == Some(*active)
                    && before_credential.as_deref() == Some(credential.as_str())
                    && after_epoch == epoch.checked_add(1).ok_or_else(invalid)?
                    && match cause.as_str() {
                        "revoked" => {
                            *active == 1
                                && after_active == 0
                                && after_credential == *credential
                                && initiator != "system"
                        }
                        "regranted" => {
                            *active == 0
                                && after_active == 1
                                && after_credential != *credential
                                && initiator != "system"
                        }
                        "policy_changed" => {
                            after_active == *active
                                && after_credential == *credential
                                && initiator == "system"
                        }
                        _ => false,
                    } => {}
            _ => return Err(invalid()),
        }
        states.insert(
            receiver_id,
            (
                after_epoch,
                after_active,
                after_credential,
                organization_id,
                owner_id,
            ),
        );
    }
    let mut query = connection.prepare(
        "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers",
    )?;
    let receivers = query.query_map([], row_binding)?;
    let mut count = 0;
    for receiver in receivers {
        let receiver = receiver?;
        let state = states.get(&receiver.receiver_id).ok_or_else(invalid)?;
        if state.0 != i64::try_from(receiver.access_epoch).map_err(|_| invalid())?
            || state.1 != i64::from(receiver.active)
            || state.2 != receiver.credential_id.as_str()
            || state.3 != receiver.organization_id.as_str()
            || state.4 != receiver.owner_id.as_str()
        {
            return Err(invalid());
        }
        count += 1;
    }
    if count != states.len() {
        return Err(invalid());
    }
    let policy: Option<String> = connection
        .query_row(
            "SELECT revision FROM receiver_policy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if (!states.is_empty() && policy.is_none()) || policy.is_some_and(|value| value.is_empty()) {
        return Err(invalid());
    }
    Ok(())
}

fn advance_policy(
    connection: &mut Connection,
    revision: &str,
    observed_at_ms: i64,
) -> nessa_local_database::rusqlite::Result<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous: Option<String> = transaction
        .query_row(
            "SELECT revision FROM receiver_policy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match previous {
        None => {
            transaction.execute("INSERT INTO receiver_policy VALUES (1, ?1)", [revision])?;
        }
        Some(previous) if previous != revision => {
            let receivers = {
                let mut query = transaction.prepare(
                    "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers",
                )?;
                let rows = query
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })?
                    .collect::<nessa_local_database::rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (receiver_id, credential_id, organization_id, owner_id, epoch, active) in receivers
            {
                let next = epoch
                    .checked_add(1)
                    .ok_or(nessa_local_database::rusqlite::Error::InvalidQuery)?;
                transaction.execute(
                    "UPDATE receivers SET access_epoch = ?2 WHERE receiver_id = ?1",
                    params![receiver_id, next],
                )?;
                transaction.execute(
                    "INSERT INTO receiver_transitions (receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_id, request_id, observed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?7, 'policy_changed', 'system', ?8, ?9)",
                    params![receiver_id, organization_id, owner_id, epoch, next, active, credential_id, format!("policy-{}", uuid::Uuid::new_v4()), observed_at_ms],
                )?;
            }
            transaction.execute(
                "UPDATE receiver_policy SET revision = ?1 WHERE id = 1",
                [revision],
            )?;
        }
        Some(_) => {}
    }
    transaction.commit()
}

fn row_binding(
    row: &nessa_local_database::rusqlite::Row<'_>,
) -> nessa_local_database::rusqlite::Result<ReceiverBinding> {
    let receiver_id: String = row.get(0)?;
    let credential_id: String = row.get(1)?;
    let organization_id: String = row.get(2)?;
    let owner_id: String = row.get(3)?;
    let epoch: i64 = row.get(4)?;
    let active: i64 = row.get(5)?;
    let convert = || nessa_local_database::rusqlite::Error::InvalidQuery;
    Ok(ReceiverBinding {
        receiver_id,
        credential_id: CredentialId::new(credential_id).map_err(|_| convert())?,
        organization_id: OrganizationId::new(organization_id).map_err(|_| convert())?,
        owner_id: PrincipalId::new(owner_id).map_err(|_| convert())?,
        access_epoch: u64::try_from(epoch)
            .ok()
            .filter(|epoch| *epoch > 0)
            .ok_or_else(convert)?,
        active: match active {
            0 => false,
            1 => true,
            _ => return Err(convert()),
        },
    })
}

fn load_by_receiver(
    connection: &Connection,
    receiver_id: &str,
) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
    connection.query_row(
        "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers WHERE receiver_id = ?1",
        [receiver_id], row_binding,
    ).optional().map_err(|_| ReceiverChangeError::Unavailable)
}

impl ReceiverAuthority for LocalReceiverAuthority {
    fn resolve<'a>(
        &'a self,
        credential_id: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        let connection = self.connection.clone();
        let credential_id = credential_id.as_str().to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let connection = connection.lock().map_err(|_| ReadRefusal::Unverifiable)?;
                connection.query_row(
                    "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers WHERE credential_id = ?1",
                    [credential_id], row_binding,
                ).optional().map_err(|_| ReadRefusal::Unverifiable)
            }).await.map_err(|_| ReadRefusal::Unverifiable)?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            123_000
        }
    }

    fn open(path: &Path, revision: &str) -> Result<LocalReceiverAuthority, OpenError> {
        LocalReceiverAuthority::open(path, revision, Arc::new(FixedClock))
    }

    #[tokio::test]
    async fn receiver_identity_and_epoch_survive_reconnect_and_restart() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let original = CredentialId::new("credential-one").unwrap();
        let replacement = CredentialId::new("credential-two").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let organization = OrganizationId::new("org").unwrap();
        let receiver_id;
        {
            let store = open(&path, "policy-one").unwrap();
            let paired = store
                .pair(
                    original.clone(),
                    organization.clone(),
                    owner.clone(),
                    owner.clone(),
                    "pair".into(),
                )
                .await
                .unwrap();
            receiver_id = paired.receiver_id.clone();
            assert_eq!(paired.access_epoch, 1);
            assert_eq!(store.resolve(&original).await.unwrap(), Some(paired));
            let revoked = store
                .change(
                    receiver_id.clone(),
                    None,
                    false,
                    owner.clone(),
                    "revoke".into(),
                )
                .await
                .unwrap();
            assert!(!revoked.active);
            assert_eq!(revoked.access_epoch, 2);
            assert_eq!(
                store
                    .change(
                        receiver_id.clone(),
                        None,
                        false,
                        owner.clone(),
                        "repeat".into()
                    )
                    .await,
                Err(ReceiverChangeError::Conflict)
            );
            assert_eq!(
                store
                    .pair(
                        CredentialId::new("credential-unused").unwrap(),
                        organization.clone(),
                        owner.clone(),
                        owner.clone(),
                        "pair".into(),
                    )
                    .await,
                Err(ReceiverChangeError::Conflict)
            );
            assert_eq!(
                store
                    .resolve(&CredentialId::new("credential-unused").unwrap())
                    .await
                    .unwrap(),
                None
            );
        }
        let reopened = open(&path, "policy-one").unwrap();
        assert_eq!(
            reopened
                .resolve(&original)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            2
        );
        let regranted = reopened
            .change(
                receiver_id.clone(),
                Some(replacement.clone()),
                true,
                owner,
                "regrant".into(),
            )
            .await
            .unwrap();
        assert!(regranted.active);
        assert_eq!(regranted.access_epoch, 3);
        assert_eq!(regranted.receiver_id, receiver_id);
        assert_eq!(reopened.resolve(&original).await.unwrap(), None);
        assert_eq!(
            reopened.resolve(&replacement).await.unwrap(),
            Some(regranted)
        );
        drop(reopened);
        let updated = open(&path, "policy-two").unwrap();
        assert_eq!(
            updated
                .resolve(&replacement)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            4
        );
        drop(updated);
        let same = open(&path, "policy-two").unwrap();
        assert_eq!(
            same.resolve(&replacement)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            4
        );
        drop(same);
        let restored_policy = open(&path, "policy-one").unwrap();
        assert_eq!(
            restored_policy
                .resolve(&replacement)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            5
        );
        let raw = Connection::open(&path).unwrap();
        let transitions = raw
            .prepare("SELECT receiver_id, before_epoch, after_epoch, cause, initiator_id, before_credential, after_credential, observed_at_ms FROM receiver_transitions ORDER BY sequence")
            .unwrap()
            .query_map([], |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            )))
            .unwrap()
            .collect::<nessa_local_database::rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(transitions.len(), 5);
        assert_eq!(
            transitions
                .iter()
                .map(|entry| entry.3.as_str())
                .collect::<Vec<_>>(),
            [
                "paired",
                "revoked",
                "regranted",
                "policy_changed",
                "policy_changed"
            ]
        );
        assert!(transitions.iter().all(|entry| entry.0 == receiver_id));
        assert_eq!(transitions[0].1, None);
        assert_eq!(transitions[0].2, 1);
        assert_eq!(transitions[0].4, "owner");
        assert_eq!(transitions[0].5, None);
        assert_eq!(transitions[0].6, "credential-one");
        assert_eq!(transitions[2].1, Some(2));
        assert_eq!(transitions[2].2, 3);
        assert_eq!(transitions[2].5.as_deref(), Some("credential-one"));
        assert_eq!(transitions[2].6, "credential-two");
        assert_eq!(transitions[3].4, "system");
        assert!(transitions.iter().all(|entry| entry.7 == 123_000));
        drop(restored_policy);
        raw.execute(
            "UPDATE receivers SET access_epoch = 3 WHERE receiver_id = ?1",
            [&receiver_id],
        )
        .unwrap();
        assert!(matches!(
            open(&path, "policy-one"),
            Err(OpenError::Unreadable(_))
        ));
        raw.execute(
            "UPDATE receivers SET access_epoch = 5, owner_id = 'other' WHERE receiver_id = ?1",
            [&receiver_id],
        )
        .unwrap();
        assert!(matches!(
            open(&path, "policy-one"),
            Err(OpenError::Unreadable(_))
        ));
        raw.execute(
            "UPDATE receivers SET owner_id = 'owner' WHERE receiver_id = ?1",
            [&receiver_id],
        )
        .unwrap();
        raw.execute("DELETE FROM receiver_policy", []).unwrap();
        assert!(matches!(
            open(&path, "policy-one"),
            Err(OpenError::Unreadable(_))
        ));
    }
}
