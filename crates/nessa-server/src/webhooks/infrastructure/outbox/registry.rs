//! Private registration facts and the credential lifecycle stop projection.
use super::journal::{commit, finish, refuse_pending};
use super::publication::{append, publish};
use super::{integer, invalid, unavailable, uuid, SqlOutbox};
use crate::webhooks::application::{
    DeliveryCause, MacKey, OutboxError, Registration, StopReason, Target,
};
use crate::webhooks::domain::Destination;
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, OptionalExtension, Result as SqlResult, Row};
use serde_json::json;
use std::sync::Arc;

impl SqlOutbox {
    /// The caller owns registration authorization and a per-principal issue
    /// permit. No endpoint, key, or row alone supplies conversation authority.
    pub async fn install(&self, registration: Registration, at_ms: u64) -> Result<(), OutboxError> {
        self.run(move |connection, audit| {
            let transaction = connection.transaction().map_err(unavailable)?;
            transaction.execute("INSERT INTO registrations(id,credential,principal,organization,owner,destination,mac_key,active) VALUES(?1,?2,?3,?4,?5,?6,?7,1)", params![registration.id.to_string(),registration.target.credential.as_str(),registration.target.principal.as_str(),registration.target.organization.as_str(),registration.target.owner.as_str(),registration.destination.expose_url().as_str(),registration.key.expose_bytes()]).map_err(|_| OutboxError::Conflict)?;
            append(&transaction, registration.id, None, json!({"kind":"webhook_registration","registration":registration.id.to_string(),"credential":registration.target.credential.as_str(),"principal":registration.target.principal.as_str(),"organization":registration.target.organization.as_str(),"owner":registration.target.owner.as_str(),"before":"absent","after":"delivering","cause":"credential_issue","initiator":registration.target.owner.as_str(),"atMs":at_ms}))?;
            commit(transaction,Some(registration.id),at_ms)?;
            publish(connection, audit)
        }).await
    }

    /// Only the credential lifecycle coordinator calls stop. It supplies the
    /// original revocation initiator, or the first expiry observer. The same
    /// per-registration permit used by dispatch must surround this operation.
    pub async fn stop(
        &self,
        credential: CredentialId,
        reason: StopReason,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let clock = self.owner.clock.clone();
        self.run(move |connection,audit| {
            let transaction=connection.transaction().map_err(unavailable)?;
            let row:Option<(String,i64)>=transaction.query_row("SELECT id,active FROM registrations WHERE credential=?1",[credential.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(unavailable)?;
            let Some((registration,active))=row else { return Ok(()); };
            if active==0 { return Ok(()); }
            let id=uuid(&registration)?;
            let (cause,initiator)=match reason { StopReason::Revoked{initiator} => ("credential_revoked",initiator.as_str().to_owned()), StopReason::Expired{observed_by} => ("credential_expired",format!("delivery:{observed_by}")) };
            transaction.execute("UPDATE registrations SET active=0,stop_cause=?1,stop_initiator=?2,stop_at=?4 WHERE id=?3",params![cause,initiator,registration,integer(at_ms)?]).map_err(unavailable)?;
            refuse_pending(&transaction,id,None,DeliveryCause::AuthorityStopped,at_ms)?;
            append(&transaction,id,None,json!({"kind":"webhook_registration","registration":registration,"credential":credential.as_str(),"before":"delivering","after":"stopped","cause":cause,"initiator":initiator,"atMs":at_ms}))?;
            commit(transaction,Some(id),at_ms)?;
            finish(connection,audit,clock.unix_milliseconds())
        }).await
    }

    pub async fn registration(
        &self,
        credential: CredentialId,
    ) -> Result<Option<Registration>, OutboxError> {
        self.run(move |connection, _| {
            connection.query_row("SELECT id,credential,principal,organization,owner,destination,mac_key FROM registrations WHERE credential=?1 AND active=1", [credential.as_str()], registration_row).optional().map_err(unavailable)?.map(registration_value).transpose()
        }).await
    }
}

type RegistrationRow = (String, String, String, String, String, String, Vec<u8>);
fn registration_row(row: &Row<'_>) -> SqlResult<RegistrationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}
pub(super) fn registration_value(row: RegistrationRow) -> Result<Registration, OutboxError> {
    let (id, credential, principal, organization, owner, destination, key) = row;
    Ok(Registration {
        id: uuid(&id)?,
        target: Target {
            credential: CredentialId::new(credential).map_err(invalid)?,
            principal: PrincipalId::new(principal).map_err(invalid)?,
            organization: OrganizationId::new(organization).map_err(invalid)?,
            owner: PrincipalId::new(owner).map_err(invalid)?,
        },
        destination: Destination::parse(&destination).map_err(invalid)?,
        key: Arc::new(MacKey::new(key.try_into().map_err(invalid)?)),
    })
}
