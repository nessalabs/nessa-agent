//! Scoped catalogue projection and the explicit version-6 preserving upgrade.
//! A floor preserves an already-public old boundary; it conveys no grant and is
//! not recomputed after upgrade (`metadata_upgrade_preserves_old_cursors_and_grant_receipts`).
use super::{acquisition, share_grants};
use nessa_auth::domain::MAX_IDENTIFIER_BYTES;
use nessa_local_database::rusqlite::{params, Connection, Transaction};

const DEFINITION: &str = include_str!("catalogue_heads.sql");

/// Only the immediately preceding active grant can lose this party. Inactive
/// revokes break the chain; capability/no-op rows retain it (#724 ordering table).
const PREVIOUS_ACTIVE: &str = "AND j.after <> 'none'
    AND j.sequence = (SELECT MAX(previous.sequence) FROM share_grant_changes AS previous
                      WHERE previous.resource_kind = later.resource_kind
                        AND previous.resource_id = later.resource_id
                        AND previous.principal_id = later.principal_id
                        AND previous.sequence < later.sequence)";

const FLOOR: &str = "COALESCE((SELECT head FROM catalogue_reader_floors
                               WHERE organization = ?1 AND owner = ?2 AND receiver = ?3), 0)";

/// Current granted rows, this receiver's grant/revoke transitions and its own
/// active predecessor removal move the head. The immutable old floor preserves
/// saved boundaries; later replacements of other receivers do not move it.
pub(super) fn head(
    connection: &Connection,
    organization: &str,
    owner: &str,
    receiver: &str,
) -> nessa_local_database::rusqlite::Result<i64> {
    connection.query_row(
        &query(PREVIOUS_ACTIVE, FLOOR),
        params![organization, owner, receiver],
        |row| row.get(0),
    )
}

/// Seed every historical conversation receiver, including revoked receivers and
/// those without a live client. The opener owns this transaction's commit and
/// rollback. No conversation, grant, journal, receipt or attribution row changes.
pub(super) fn preserve_legacy_heads(
    transaction: &Transaction<'_>,
) -> nessa_local_database::rusqlite::Result<()> {
    transaction.execute_batch(DEFINITION)?;
    let text = |column| acquisition::text(column, MAX_IDENTIFIER_BYTES, false);
    let mut receivers = transaction.prepare(&format!(
        "SELECT DISTINCT {}, {}, {} FROM share_grant_changes AS j
         JOIN conversations AS c ON c.id = j.resource_id
         WHERE j.resource_kind = '{kind}' AND j.receiver_id IS NOT NULL",
        text("c.organization"),
        text("c.owner"),
        text("j.receiver_id"),
        kind = share_grants::CONVERSATION,
    ))?;
    let rows = receivers.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    // Exactly the version-6 projection, including its all-ancestor replacement
    // join. It is used only within this one upgrade, never as a runtime fallback.
    let legacy = query("", "0");
    for row in rows {
        let (organization, owner, receiver) = row?;
        let head: i64 =
            transaction.query_row(&legacy, params![organization, owner, receiver], |row| {
                row.get(0)
            })?;
        let owner_head: i64 = transaction.query_row(
            "SELECT head FROM catalogue_owners WHERE organization = ?1 AND owner = ?2",
            params![organization, owner],
            |row| row.get(0),
        )?;
        if head > owner_head {
            return Err(nessa_local_database::rusqlite::Error::InvalidQuery);
        }
        transaction.execute(
            "INSERT INTO catalogue_reader_floors (organization, owner, receiver, head)
             VALUES (?1, ?2, ?3, ?4)",
            params![organization, owner, receiver, head],
        )?;
    }
    Ok(())
}

/// One scoped SQL projection: runtime narrows replacement history and includes
/// the frozen floor; the one predecessor upgrade reads the exact old projection.
fn query(replacement_history: &str, floor: &str) -> String {
    format!(
        "SELECT MAX(
             COALESCE((SELECT MAX(c.change_revision) FROM conversations AS c
                       WHERE c.organization = ?1 AND c.owner = ?2 AND {granted}), 0),
             COALESCE((SELECT MAX(j.revision) FROM share_grant_changes AS j
                       JOIN conversations AS c ON c.id = j.resource_id
                       WHERE c.organization = ?1 AND c.owner = ?2
                         AND j.resource_kind = '{kind}' AND j.receiver_id = ?3
                         AND (j.before = 'none') <> (j.after = 'none')), 0),
             COALESCE((SELECT MAX(later.revision) FROM share_grant_changes AS later
                       JOIN share_grant_changes AS j
                         ON j.resource_kind = later.resource_kind
                        AND j.resource_id = later.resource_id
                        AND j.principal_id = later.principal_id
                        AND j.sequence < later.sequence
                        {replacement_history}
                        AND j.receiver_id = ?3
                        AND (later.receiver_id IS NOT j.receiver_id
                             OR later.credential_id <> j.credential_id)
                       JOIN conversations AS c ON c.id = later.resource_id
                       WHERE c.organization = ?1 AND c.owner = ?2
                         AND later.resource_kind = '{kind}'), 0),
             {floor})",
        granted = share_grants::granted("c.id", "?3"),
        kind = share_grants::CONVERSATION,
    )
}
