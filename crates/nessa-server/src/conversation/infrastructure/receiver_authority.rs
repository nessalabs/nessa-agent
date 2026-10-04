//! Durable local authority for paired receiver identity and access epochs.

use crate::conversation::application::{ReadRefusal, ReceiverAuthority, ReceiverBinding};
use crate::conversation::domain::{
    PairedReceiver, ReceiverInitiator, ReceiverIntent, ReceiverTransition, ReceiverTransitionError,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior},
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
        let authority = self.clone();
        tokio::task::spawn_blocking(move || {
            authority.pair_now(
                credential_id,
                organization_id,
                owner_id,
                initiator_id,
                request_id,
            )
        })
        .await
        .map_err(|_| ReceiverChangeError::Unavailable)?
    }

    /// `pair` on the calling thread, for a caller already on a blocking worker.
    /// An exact repeat of the same initiator and request returns the original
    /// receipt, without a second receiver or epoch; the same request naming
    /// another credential, organization or owner is a conflict (design row P38,
    /// `pair_retry_returns_the_original_receipt_and_fence_is_exact`).
    pub fn pair_now(
        &self,
        credential_id: CredentialId,
        organization_id: OrganizationId,
        owner_id: PrincipalId,
        initiator_id: PrincipalId,
        request_id: String,
    ) -> Result<ReceiverBinding, ReceiverChangeError> {
        let receiver_id = format!("receiver-{}", uuid::Uuid::new_v4());
        let observed_at_ms = i64::try_from(self.clock.unix_milliseconds())
            .map_err(|_| ReceiverChangeError::Exhausted)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        if let Some(original) = load_receipt(
            &transaction,
            &ReceiverInitiator::Principal(initiator_id.clone()),
            &request_id,
        )? {
            let exact = matches!(original.cause, ReceiverIntent::Pair { .. })
                && original.after.credential_id == credential_id
                && original.after.organization_id == organization_id
                && original.after.owner_id == owner_id;
            return if exact {
                Ok(original.after)
            } else {
                Err(ReceiverChangeError::Conflict)
            };
        }
        let transition = ReceiverTransition::apply(
            None,
            ReceiverIntent::Pair {
                receiver_id,
                credential_id,
                organization_id,
                owner_id,
            },
            ReceiverInitiator::Principal(initiator_id),
            request_id,
            observed_at_ms,
        )
        .map_err(map_transition_error)?;
        persist_transition(&transaction, &transition).map_err(map_write_error)?;
        transaction
            .commit()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        Ok(transition.after)
    }

    /// Look up the original pair receipt for this initiator and request,
    /// without pairing anything: `None` is a coherent absence, never a failure.
    /// Device pairing's terminal cleanup asks this instead of pairing again
    /// (design row P48).
    pub fn paired(
        &self,
        initiator_id: &PrincipalId,
        request_id: &str,
    ) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        match load_receipt(
            &connection,
            &ReceiverInitiator::Principal(initiator_id.clone()),
            request_id,
        )? {
            Some(original) if matches!(original.cause, ReceiverIntent::Pair { .. }) => {
                Ok(Some(original.after))
            }
            // Another transition under this request is not a pair receipt.
            Some(_) => Err(ReceiverChangeError::Conflict),
            None => Ok(None),
        }
    }

    /// The current binding of a credential, on the calling thread.
    pub fn binding(
        &self,
        credential_id: &CredentialId,
    ) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        load_by_credential(&connection, credential_id.as_str())
    }

    /// The current binding of `paired`'s receiver, if it still holds that
    /// pairing (`PairedReceiver::admits`, with this journal saying whether the
    /// credential was revoked on it since the pair). `None` otherwise.
    pub fn holding(
        &self,
        paired: &PairedReceiver,
    ) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        let Some(current) = load_by_receiver(&connection, &paired.receiver_id)? else {
            return Ok(None);
        };
        let revoked_since = load_revocation(&connection, paired)?.is_some();
        Ok(paired.admits(&current, revoked_since).then_some(current))
    }

    /// Fence the exact receiver a device enrollment paired, as the system, once
    /// that enrollment has ended (design rows P39, P41, P42). A repeat of
    /// `request_id` returns the original fence. A revocation of the paired
    /// credential on that receiver since the pair is the fence, with its own
    /// initiator kept, even if the receiver was regranted since; it is never
    /// fenced again. Otherwise the domain's `PairedReceiver::fence` decides.
    pub fn fence(
        &self,
        paired: &PairedReceiver,
        request_id: String,
    ) -> Result<ReceiverBinding, ReceiverChangeError> {
        let observed_at_ms = i64::try_from(self.clock.unix_milliseconds())
            .map_err(|_| ReceiverChangeError::Exhausted)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        if let Some(original) = load_receipt(&transaction, &ReceiverInitiator::System, &request_id)?
        {
            return if paired.revoked_by(&original.cause, original.before.as_ref()) {
                Ok(original.after)
            } else {
                Err(ReceiverChangeError::Conflict)
            };
        }
        if let Some(revocation) = load_revocation(&transaction, paired)? {
            return Ok(revocation);
        }
        let current = load_by_receiver(&transaction, &paired.receiver_id)?
            .ok_or(ReceiverChangeError::Missing)?;
        let transition = paired
            .fence(&current, false, request_id, observed_at_ms)
            .map_err(map_transition_error)?;
        persist_transition(&transaction, &transition).map_err(map_write_error)?;
        transaction
            .commit()
            .map_err(|_| ReceiverChangeError::Unavailable)?;
        Ok(transition.after)
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
        let observed_at_ms = i64::try_from(self.clock.unix_milliseconds())
            .map_err(|_| ReceiverChangeError::Exhausted)?;
        let connection = self.connection.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = connection.lock().map_err(|_| ReceiverChangeError::Unavailable)?;
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| ReceiverChangeError::Unavailable)?;
            let previous = load_by_receiver(&transaction, &receiver_id)?.ok_or(ReceiverChangeError::Missing)?;
            let cause = match (active, replacement_credential) {
                (false, None) => ReceiverIntent::Revoke,
                (true, Some(credential)) => ReceiverIntent::Regrant(credential),
                _ => return Err(ReceiverChangeError::Conflict),
            };
            let transition = ReceiverTransition::apply(
                Some(&previous), cause, ReceiverInitiator::Principal(initiator_id.clone()),
                request_id.clone(), observed_at_ms,
            ).map_err(map_transition_error)?;
            let prior_request: Option<String> = transaction.query_row(
                "SELECT receiver_id FROM receiver_transitions WHERE initiator_kind = 'principal' AND initiator_id = ?1 AND request_id = ?2",
                params![initiator_id.as_str(), request_id], |row| row.get(0),
            ).optional().map_err(|_| ReceiverChangeError::Unavailable)?;
            if prior_request.is_some() { return Err(ReceiverChangeError::Conflict); }
            persist_transition(&transaction, &transition).map_err(map_write_error)?;
            transaction.commit().map_err(|_| ReceiverChangeError::Unavailable)?;
            Ok(transition.after)
        }).await.map_err(|_| ReceiverChangeError::Unavailable)?
    }
}

fn map_transition_error(error: ReceiverTransitionError) -> ReceiverChangeError {
    match error {
        ReceiverTransitionError::Conflict => ReceiverChangeError::Conflict,
        ReceiverTransitionError::Exhausted => ReceiverChangeError::Exhausted,
    }
}

/// Only expected identity and receipt key collisions are conflicts. A full,
/// interrupted, or otherwise unusable SQLite write cannot blame the request.
fn map_write_error(error: nessa_local_database::rusqlite::Error) -> ReceiverChangeError {
    use nessa_local_database::rusqlite::{ffi, Error};
    match error {
        Error::SqliteFailure(code, _)
            if matches!(
                code.extended_code,
                ffi::SQLITE_CONSTRAINT_PRIMARYKEY | ffi::SQLITE_CONSTRAINT_UNIQUE
            ) =>
        {
            ReceiverChangeError::Conflict
        }
        _ => ReceiverChangeError::Unavailable,
    }
}

/// SQLite owns atomicity and sequence; the domain transition owns the state.
fn persist_transition(
    transaction: &Transaction<'_>,
    transition: &ReceiverTransition,
) -> nessa_local_database::rusqlite::Result<()> {
    let after = &transition.after;
    let after_epoch = i64::try_from(after.access_epoch)
        .map_err(|_| nessa_local_database::rusqlite::Error::InvalidQuery)?;
    match &transition.before {
        None => {
            transaction.execute(
                "INSERT INTO receivers VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    after.receiver_id,
                    after.credential_id.as_str(),
                    after.organization_id.as_str(),
                    after.owner_id.as_str(),
                    after_epoch,
                    after.active
                ],
            )?;
        }
        Some(_) => {
            let updated = transaction.execute(
                "UPDATE receivers SET credential_id = ?2, access_epoch = ?3, active = ?4 WHERE receiver_id = ?1",
                params![after.receiver_id, after.credential_id.as_str(), after_epoch, after.active],
            )?;
            if updated != 1 {
                return Err(nessa_local_database::rusqlite::Error::InvalidQuery);
            }
        }
    }
    let cause = transition.cause.cause_name();
    let (initiator_kind, initiator_id) = transition.initiator.evidence_parts();
    transaction.execute(
        "INSERT INTO receiver_transitions (receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_kind, initiator_id, request_id, observed_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![after.receiver_id, after.organization_id.as_str(), after.owner_id.as_str(),
            transition.before.as_ref().map(|before| i64::try_from(before.access_epoch)).transpose().map_err(|_| nessa_local_database::rusqlite::Error::InvalidQuery)?,
            after_epoch, transition.before.as_ref().map(|before| before.active), after.active,
            transition.before.as_ref().map(|before| before.credential_id.as_str()),
            after.credential_id.as_str(), cause, initiator_kind, initiator_id,
            transition.request_id, transition.observed_at_ms],
    )?;
    Ok(())
}

/// Reconstruct every receiver with the same domain operation used by writes.
fn validate_history(connection: &Connection) -> nessa_local_database::rusqlite::Result<()> {
    let invalid = || nessa_local_database::rusqlite::Error::InvalidQuery;
    let mut states: HashMap<String, ReceiverBinding> = HashMap::new();
    let mut query = connection.prepare(
        "SELECT sequence, receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause, initiator_kind, initiator_id, request_id, observed_at_ms FROM receiver_transitions ORDER BY sequence",
    )?;
    let mut rows = query.query([])?;
    let mut sequence = 0_i64;
    while let Some(row) = rows.next()? {
        sequence = sequence.checked_add(1).ok_or_else(invalid)?;
        if row.get::<_, i64>(0)? != sequence {
            return Err(invalid());
        }
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
        let initiator_kind: String = row.get(11)?;
        let initiator_id: Option<String> = row.get(12)?;
        let request_id: String = row.get(13)?;
        let observed_at_ms: i64 = row.get(14)?;
        let after = binding_from_fields(
            receiver_id.clone(),
            after_credential.clone(),
            organization_id.clone(),
            owner_id.clone(),
            after_epoch,
            after_active,
        )?;
        let before = match (before_epoch, before_active, before_credential) {
            (None, None, None) => None,
            (Some(epoch), Some(active), Some(credential)) => Some(binding_from_fields(
                receiver_id.clone(),
                credential,
                organization_id,
                owner_id,
                epoch,
                active,
            )?),
            _ => return Err(invalid()),
        };
        let intent = ReceiverIntent::from_cause_name(&cause, &after).ok_or_else(invalid)?;
        let initiator = ReceiverInitiator::from_evidence_parts(&initiator_kind, initiator_id)
            .ok_or_else(invalid)?;
        let transition = ReceiverTransition {
            before,
            after,
            cause: intent,
            initiator,
            request_id,
            observed_at_ms,
        };
        transition
            .verify(states.get(&receiver_id))
            .map_err(|_| invalid())?;
        states.insert(receiver_id, transition.after);
    }
    let mut query = connection.prepare(
        "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers",
    )?;
    let receivers = query.query_map([], row_binding)?;
    let mut count = 0;
    for receiver in receivers {
        let receiver = receiver?;
        if states.get(&receiver.receiver_id) != Some(&receiver) {
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
    let invalid = || nessa_local_database::rusqlite::Error::InvalidQuery;
    if revision.is_empty() {
        return Err(invalid());
    }
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
                let collected = query
                    .query_map([], row_binding)?
                    .collect::<nessa_local_database::rusqlite::Result<Vec<_>>>()?;
                collected
            };
            for receiver in receivers {
                let transition = ReceiverTransition::apply(
                    Some(&receiver),
                    ReceiverIntent::PolicyChanged,
                    ReceiverInitiator::System,
                    format!("policy-{}", uuid::Uuid::new_v4()),
                    observed_at_ms,
                )
                .map_err(|_| invalid())?;
                persist_transition(&transaction, &transition)?;
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
    binding_from_fields(
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    )
}

fn binding_from_fields(
    receiver_id: String,
    credential_id: String,
    organization_id: String,
    owner_id: String,
    epoch: i64,
    active: i64,
) -> nessa_local_database::rusqlite::Result<ReceiverBinding> {
    let convert = || nessa_local_database::rusqlite::Error::InvalidQuery;
    if receiver_id.is_empty() || receiver_id.len() > 128 {
        return Err(convert());
    }
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

/// One persisted transition, as replay reads it back.
struct Receipt {
    before: Option<ReceiverBinding>,
    after: ReceiverBinding,
    cause: ReceiverIntent,
}

const RECEIPT_COLUMNS: &str = "receiver_id, organization_id, owner_id, before_epoch, after_epoch, before_active, after_active, before_credential, after_credential, cause";

fn receipt_row(
    row: &nessa_local_database::rusqlite::Row<'_>,
) -> nessa_local_database::rusqlite::Result<Receipt> {
    let receiver_id: String = row.get(0)?;
    let organization_id: String = row.get(1)?;
    let owner_id: String = row.get(2)?;
    let before = match (
        row.get::<_, Option<i64>>(3)?,
        row.get::<_, Option<i64>>(5)?,
        row.get::<_, Option<String>>(7)?,
    ) {
        (Some(epoch), Some(active), Some(credential)) => Some(binding_from_fields(
            receiver_id.clone(),
            credential,
            organization_id.clone(),
            owner_id.clone(),
            epoch,
            active,
        )?),
        _ => None,
    };
    let after = binding_from_fields(
        receiver_id,
        row.get(8)?,
        organization_id,
        owner_id,
        row.get(4)?,
        row.get(6)?,
    )?;
    let cause = ReceiverIntent::from_cause_name(&row.get::<_, String>(9)?, &after)
        .ok_or(nessa_local_database::rusqlite::Error::InvalidQuery)?;
    Ok(Receipt {
        before,
        after,
        cause,
    })
}

/// The transition recorded under this initiator and request, if any. The
/// unique receipt index makes it at most one.
fn load_receipt(
    connection: &Connection,
    initiator: &ReceiverInitiator,
    request_id: &str,
) -> Result<Option<Receipt>, ReceiverChangeError> {
    let (kind, id) = initiator.evidence_parts();
    connection
        .query_row(
            &format!("SELECT {RECEIPT_COLUMNS} FROM receiver_transitions WHERE initiator_kind = ?1 AND ifnull(initiator_id, '') = ?2 AND request_id = ?3"),
            params![kind, id.unwrap_or(""), request_id],
            receipt_row,
        )
        .optional()
        .map_err(|_| ReceiverChangeError::Unavailable)
}

/// The first revocation of `paired`'s credential on its receiver since the
/// pair, as the domain's `PairedReceiver::revoked_by` reads the journal.
fn load_revocation(
    connection: &Connection,
    paired: &PairedReceiver,
) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
    let mut query = connection
        .prepare(&format!("SELECT {RECEIPT_COLUMNS} FROM receiver_transitions WHERE receiver_id = ?1 ORDER BY sequence"))
        .map_err(|_| ReceiverChangeError::Unavailable)?;
    let receipts = query
        .query_map([&paired.receiver_id], receipt_row)
        .map_err(|_| ReceiverChangeError::Unavailable)?;
    for receipt in receipts {
        let receipt = receipt.map_err(|_| ReceiverChangeError::Unavailable)?;
        if paired.revoked_by(&receipt.cause, receipt.before.as_ref()) {
            return Ok(Some(receipt.after));
        }
    }
    Ok(None)
}

fn load_by_credential(
    connection: &Connection,
    credential_id: &str,
) -> Result<Option<ReceiverBinding>, ReceiverChangeError> {
    connection.query_row(
        "SELECT receiver_id, credential_id, organization_id, owner_id, access_epoch, active FROM receivers WHERE credential_id = ?1",
        [credential_id], row_binding,
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

    #[test]
    fn only_expected_sqlite_key_collisions_are_conflicts() {
        use nessa_local_database::rusqlite::{ffi, Error};
        for code in [
            ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
            ffi::SQLITE_CONSTRAINT_UNIQUE,
        ] {
            assert_eq!(
                map_write_error(Error::SqliteFailure(ffi::Error::new(code), None)),
                ReceiverChangeError::Conflict
            );
        }
        for code in [
            ffi::SQLITE_FULL,
            ffi::SQLITE_IOERR,
            ffi::SQLITE_CONSTRAINT_TRIGGER,
        ] {
            assert_eq!(
                map_write_error(Error::SqliteFailure(ffi::Error::new(code), None)),
                ReceiverChangeError::Unavailable
            );
        }
    }

    fn pairing(binding: &ReceiverBinding) -> PairedReceiver {
        PairedReceiver {
            receiver_id: binding.receiver_id.clone(),
            credential_id: binding.credential_id.clone(),
            organization_id: binding.organization_id.clone(),
            owner_id: binding.owner_id.clone(),
            paired_epoch: binding.access_epoch,
        }
    }

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
    async fn a_principal_named_system_survives_pair_revoke_regrant_and_policy_restart() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let actor = PrincipalId::new("system").unwrap();
        let first_credential = CredentialId::new("first").unwrap();
        let second_credential = CredentialId::new("second").unwrap();
        let receiver_id = {
            let store = open(&path, "policy-one").unwrap();
            store
                .pair(
                    first_credential.clone(),
                    OrganizationId::new("org").unwrap(),
                    actor.clone(),
                    actor.clone(),
                    "pair".into(),
                )
                .await
                .unwrap()
                .receiver_id
        };
        let store = open(&path, "policy-one").unwrap();
        assert_eq!(
            store
                .resolve(&first_credential)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            1
        );
        store
            .change(
                receiver_id.clone(),
                None,
                false,
                actor.clone(),
                "revoke".into(),
            )
            .await
            .unwrap();
        store
            .change(
                receiver_id.clone(),
                Some(second_credential.clone()),
                true,
                actor,
                "regrant".into(),
            )
            .await
            .unwrap();
        drop(store);
        let changed_policy = open(&path, "policy-two").unwrap();
        assert_eq!(
            changed_policy
                .resolve(&second_credential)
                .await
                .unwrap()
                .unwrap()
                .access_epoch,
            4
        );
        let raw = Connection::open(&path).unwrap();
        let initiators = raw
            .prepare(
                "SELECT initiator_kind, initiator_id FROM receiver_transitions ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .unwrap()
            .collect::<nessa_local_database::rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            initiators,
            [
                ("principal".into(), Some("system".into())),
                ("principal".into(), Some("system".into())),
                ("principal".into(), Some("system".into())),
                ("system".into(), None),
            ]
        );
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
            .prepare("SELECT receiver_id, before_epoch, after_epoch, cause, initiator_kind, initiator_id, before_credential, after_credential, observed_at_ms FROM receiver_transitions ORDER BY sequence")
            .unwrap()
            .query_map([], |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
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
        assert_eq!(transitions[0].4, "principal");
        assert_eq!(transitions[0].5.as_deref(), Some("owner"));
        assert_eq!(transitions[0].6, None);
        assert_eq!(transitions[0].7, "credential-one");
        assert_eq!(transitions[2].1, Some(2));
        assert_eq!(transitions[2].2, 3);
        assert_eq!(transitions[2].6.as_deref(), Some("credential-one"));
        assert_eq!(transitions[2].7, "credential-two");
        assert_eq!(transitions[3].4, "system");
        assert_eq!(transitions[3].5, None);
        assert!(transitions.iter().all(|entry| entry.8 == 123_000));
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

    #[tokio::test]
    async fn unexpected_sql_write_failures_are_unavailable_and_roll_back_each_transition() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let store = open(&path, "policy-one").unwrap();
        let raw = Connection::open(&path).unwrap();
        let actor = PrincipalId::new("owner").unwrap();
        let credential = CredentialId::new("first").unwrap();
        let pair = |request: &str| {
            store.pair(
                credential.clone(),
                OrganizationId::new("org").unwrap(),
                actor.clone(),
                actor.clone(),
                request.into(),
            )
        };
        for (table, event) in [("receivers", "INSERT"), ("receiver_transitions", "INSERT")] {
            raw.execute_batch(&format!(
                "CREATE TRIGGER fail_write BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT, 'storage failure'); END;"
            )).unwrap();
            assert_eq!(pair(table).await, Err(ReceiverChangeError::Unavailable));
            assert_eq!(
                raw.query_row("SELECT count(*) FROM receivers", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                raw.query_row("SELECT count(*) FROM receiver_transitions", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                0
            );
            raw.execute_batch("DROP TRIGGER fail_write").unwrap();
        }
        let paired = pair("pair").await.unwrap();
        assert_eq!(
            pair("duplicate-credential").await,
            Err(ReceiverChangeError::Conflict)
        );
        assert_eq!(
            store
                .pair(
                    CredentialId::new("other").unwrap(),
                    OrganizationId::new("org").unwrap(),
                    actor.clone(),
                    actor.clone(),
                    "pair".into()
                )
                .await,
            Err(ReceiverChangeError::Conflict)
        );
        for (table, event) in [("receivers", "UPDATE"), ("receiver_transitions", "INSERT")] {
            raw.execute_batch(&format!(
                "CREATE TRIGGER fail_write BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT, 'storage failure'); END;"
            )).unwrap();
            assert_eq!(
                store
                    .change(
                        paired.receiver_id.clone(),
                        None,
                        false,
                        actor.clone(),
                        table.into()
                    )
                    .await,
                Err(ReceiverChangeError::Unavailable)
            );
            assert_eq!(
                store.resolve(&credential).await.unwrap(),
                Some(paired.clone())
            );
            assert_eq!(
                raw.query_row("SELECT count(*) FROM receiver_transitions", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                1
            );
            raw.execute_batch("DROP TRIGGER fail_write").unwrap();
        }
        drop(store);
        for (table, event) in [
            ("receivers", "UPDATE"),
            ("receiver_transitions", "INSERT"),
            ("receiver_policy", "UPDATE"),
        ] {
            raw.execute_batch(&format!(
                "CREATE TRIGGER fail_write BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT, 'storage failure'); END;"
            )).unwrap();
            assert!(matches!(
                open(&path, "policy-two"),
                Err(OpenError::Database(_))
            ));
            raw.execute_batch("DROP TRIGGER fail_write").unwrap();
            assert_eq!(
                open(&path, "policy-one")
                    .unwrap()
                    .resolve(&credential)
                    .await
                    .unwrap(),
                Some(paired.clone())
            );
        }
    }

    #[tokio::test]
    async fn malformed_transition_receipt_fails_replay_with_the_live_rule() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let store = open(&path, "policy-one").unwrap();
        store
            .pair(
                CredentialId::new("first").unwrap(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("owner").unwrap(),
                PrincipalId::new("owner").unwrap(),
                "pair".into(),
            )
            .await
            .unwrap();
        drop(store);
        let raw = Connection::open(&path).unwrap();
        raw.execute("UPDATE receiver_transitions SET request_id = '   '", [])
            .unwrap();
        assert!(matches!(
            open(&path, "policy-one"),
            Err(OpenError::Unreadable(_))
        ));
    }

    #[test]
    fn initial_policy_write_failure_does_not_publish_a_revision() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        drop(open(&path, "policy-one").unwrap());
        let raw = Connection::open(&path).unwrap();
        raw.execute("DELETE FROM receiver_policy", []).unwrap();
        raw.execute_batch("CREATE TRIGGER fail_write BEFORE INSERT ON receiver_policy BEGIN SELECT RAISE(ABORT, 'storage failure'); END;").unwrap();
        assert!(matches!(
            open(&path, "policy-one"),
            Err(OpenError::Database(_))
        ));
        assert_eq!(
            raw.query_row("SELECT count(*) FROM receiver_policy", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        raw.execute_batch("DROP TRIGGER fail_write").unwrap();
        assert!(open(&path, "policy-one").is_ok());
    }

    #[tokio::test]
    async fn regrant_credential_key_collision_is_a_conflict_without_epoch_change() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let store = open(&path, "policy-one").unwrap();
        let actor = PrincipalId::new("owner").unwrap();
        let pair = |credential: &str, request: &str| {
            store.pair(
                CredentialId::new(credential).unwrap(),
                OrganizationId::new("org").unwrap(),
                actor.clone(),
                actor.clone(),
                request.into(),
            )
        };
        let first = pair("first", "pair-first").await.unwrap();
        pair("second", "pair-second").await.unwrap();
        let revoked = store
            .change(
                first.receiver_id.clone(),
                None,
                false,
                actor.clone(),
                "revoke".into(),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .change(
                    first.receiver_id,
                    Some(CredentialId::new("second").unwrap()),
                    true,
                    actor,
                    "regrant".into()
                )
                .await,
            Err(ReceiverChangeError::Conflict)
        );
        assert_eq!(
            store
                .resolve(&CredentialId::new("first").unwrap())
                .await
                .unwrap(),
            Some(revoked)
        );
        drop(store);
        assert!(open(&path, "policy-one").is_ok());
    }

    /// Design rows P38, P39, P42, P48: a device pairing's pair is retried by its
    /// request, looked up without pairing, and fenced by the system only while
    /// the receiver still holds the paired credential.
    #[test]
    fn pair_retry_returns_the_original_receipt_and_fence_is_exact() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let store = open(&path, "policy-one").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let organization = OrganizationId::new("org").unwrap();
        let credential = CredentialId::new("device").unwrap();
        let pair = |credential: &CredentialId, organization: &OrganizationId| {
            store.pair_now(
                credential.clone(),
                organization.clone(),
                owner.clone(),
                owner.clone(),
                "stage".into(),
            )
        };
        assert_eq!(store.paired(&owner, "stage"), Ok(None));
        let paired = pair(&credential, &organization).unwrap();
        // Exact retry: the original receiver and epoch, no second receiver.
        assert_eq!(pair(&credential, &organization), Ok(paired.clone()));
        assert_eq!(store.paired(&owner, "stage"), Ok(Some(paired.clone())));
        // The same request naming another credential or organization conflicts.
        assert_eq!(
            pair(&CredentialId::new("other").unwrap(), &organization),
            Err(ReceiverChangeError::Conflict)
        );
        assert_eq!(
            pair(&credential, &OrganizationId::new("org-2").unwrap()),
            Err(ReceiverChangeError::Conflict)
        );
        // Another transition under a request is not a pair receipt.
        let other = store
            .pair_now(
                CredentialId::new("unrelated").unwrap(),
                organization.clone(),
                owner.clone(),
                owner.clone(),
                "unrelated".into(),
            )
            .unwrap();
        let mut connection = Connection::open(&path).unwrap();
        let revoke = ReceiverTransition::apply(
            Some(&other),
            ReceiverIntent::Revoke,
            ReceiverInitiator::Principal(owner.clone()),
            "not-a-pair".into(),
            1,
        )
        .unwrap();
        let transaction = connection.transaction().unwrap();
        persist_transition(&transaction, &revoke).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            store.paired(&owner, "not-a-pair"),
            Err(ReceiverChangeError::Conflict)
        );
        // A fence naming a newer epoch than the receiver has, or another
        // owner, refuses without a write.
        for expected in [
            ReceiverBinding {
                access_epoch: 2,
                ..paired.clone()
            },
            ReceiverBinding {
                owner_id: PrincipalId::new("someone").unwrap(),
                ..paired.clone()
            },
        ] {
            assert_eq!(
                store.fence(&pairing(&expected), "fence".into()),
                Err(ReceiverChangeError::Conflict)
            );
        }
        assert_eq!(
            store.fence(
                &PairedReceiver {
                    receiver_id: "receiver-missing".into(),
                    ..pairing(&paired)
                },
                "fence".into()
            ),
            Err(ReceiverChangeError::Missing)
        );
        let fenced = store.fence(&pairing(&paired), "fence".into()).unwrap();
        assert!(!fenced.active);
        assert_eq!(fenced.access_epoch, 2);
        assert_eq!(store.binding(&credential), Ok(Some(fenced.clone())));
        // Retried fence: the original, with no further epoch.
        assert_eq!(
            store.fence(&pairing(&paired), "fence".into()),
            Ok(fenced.clone())
        );
        // Another fence request finds the same revocation rather than a new one.
        assert_eq!(
            store.fence(&pairing(&paired), "fence-2".into()),
            Ok(fenced.clone())
        );
        let raw = Connection::open(&path).unwrap();
        let causes = raw
            .prepare("SELECT cause, initiator_kind FROM receiver_transitions ORDER BY sequence")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<nessa_local_database::rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            causes,
            [
                ("paired".into(), "principal".into()),
                ("paired".into(), "principal".into()),
                ("revoked".into(), "principal".into()),
                ("revoked".into(), "system".into())
            ]
        );
        drop(store);
        // Replay accepts the system fence on reopen.
        let reopened = open(&path, "policy-one").unwrap();
        assert_eq!(reopened.binding(&credential), Ok(Some(fenced)));
    }

    /// Design row P42: a receiver its owner already revoked and regranted to
    /// another credential is not fenced again; the owner's revocation stands.
    #[tokio::test]
    async fn fence_keeps_an_earlier_revocation_and_never_fences_a_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("receiver-access.sqlite3");
        let store = open(&path, "policy-one").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let paired = store
            .pair_now(
                CredentialId::new("device").unwrap(),
                OrganizationId::new("org").unwrap(),
                owner.clone(),
                owner.clone(),
                "stage".into(),
            )
            .unwrap();
        let revoked = store
            .change(
                paired.receiver_id.clone(),
                None,
                false,
                owner.clone(),
                "revoke".into(),
            )
            .await
            .unwrap();
        store
            .change(
                paired.receiver_id.clone(),
                Some(CredentialId::new("replacement").unwrap()),
                true,
                owner,
                "regrant".into(),
            )
            .await
            .unwrap();
        assert_eq!(store.fence(&pairing(&paired), "fence".into()), Ok(revoked));
        assert!(
            store
                .binding(&CredentialId::new("replacement").unwrap())
                .unwrap()
                .unwrap()
                .active
        );
    }

    /// Row P46: a receiver holds its pairing until its credential is revoked on
    /// it, even if that same credential is regranted back afterwards.
    #[tokio::test]
    async fn holding_ends_at_an_intervening_revocation_even_when_regranted_back() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let store = open(&private.join("receiver-access.sqlite3"), "policy-one").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let credential = CredentialId::new("device").unwrap();
        let paired = store
            .pair_now(
                credential.clone(),
                OrganizationId::new("org").unwrap(),
                owner.clone(),
                owner.clone(),
                "stage".into(),
            )
            .unwrap();
        assert_eq!(store.holding(&pairing(&paired)), Ok(Some(paired.clone())));
        let receiver = paired.receiver_id.clone();
        let steps = [
            (None, false, "revoke-1"),
            (
                Some(CredentialId::new("other").unwrap()),
                true,
                "regrant-other",
            ),
            (None, false, "revoke-2"),
            (Some(credential.clone()), true, "regrant-back"),
        ];
        for (replacement, active, request) in steps {
            store
                .change(
                    receiver.clone(),
                    replacement,
                    active,
                    owner.clone(),
                    request.into(),
                )
                .await
                .unwrap();
        }
        let current = store.binding(&credential).unwrap().unwrap();
        assert!(current.active && current.access_epoch > paired.access_epoch);
        assert_eq!(store.holding(&pairing(&paired)), Ok(None));
    }
}
