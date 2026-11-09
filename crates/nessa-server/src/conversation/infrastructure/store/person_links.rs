//! One grant-store effect owner for live publication, next-write retry and
//! restart. Registry serialization precedes the SQL connection. The link and
//! its audit commit together; neither changes any share grant.
use super::LocalConversationStore;
use crate::conversation::application::ConversationError;
use nessa_auth::{
    application::{
        pairing::{PairingLinkEvidence, PairingLinkSource, PairingPersonLinks, PersonLinkError},
        ports::Clock,
    },
    domain::pairing::{PairingRecord, PersonLinkCause, PersonLinkChange, PersonLinkInitiator},
};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::sync::{Arc, Mutex, Weak};

/// The grant-file attribution effect owner bound once by composition.
pub struct LocalPersonLinks {
    connection: Arc<Mutex<Connection>>,
    source: Weak<dyn PairingLinkSource>,
    clock: Arc<dyn Clock>,
}
impl LocalPersonLinks {
    /// Open attribution on the existing grant file, including gateways with no
    /// agent runtime. This does not open a conversation metadata file.
    pub fn open(
        path: &std::path::Path,
        source: Arc<dyn PairingLinkSource>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<Self>, nessa_local_database::OpenError> {
        let connection = super::grant_database::open(path)?;
        Ok(Arc::new(Self {
            connection: Arc::new(Mutex::new(connection)),
            source: Arc::downgrade(&source),
            clock,
        }))
    }

    /// Keep credential serialization across reconciliation and the next effect.
    pub(super) fn with_current<T>(
        &self,
        work: impl FnOnce() -> Result<T, ConversationError>,
    ) -> Result<T, ConversationError> {
        let source = self.source.upgrade().ok_or(ConversationError::Metadata)?;
        let mut work = Some(work);
        let mut result = None;
        source
            .with_current_links(&mut |evidence| {
                // A broken source must not invoke the physical operation twice.
                let work = work.take().ok_or(PersonLinkError::Conflict)?;
                self.reconcile(evidence)?;
                result = Some(work());
                Ok(())
            })
            .map_err(|_| ConversationError::Metadata)?;
        result.ok_or(ConversationError::Metadata)?
    }
}
impl LocalConversationStore {
    /// Wire the existing credential owner to this grant store once. The weak
    /// source keeps the registry -> effect binding from forming an Arc cycle.
    pub fn bind_person_link_source(
        &self,
        source: Arc<dyn PairingLinkSource>,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<dyn PairingPersonLinks>, ConversationError> {
        let mut slot = self
            .person_links
            .write()
            .map_err(|_| ConversationError::Metadata)?;
        if slot.is_some() {
            return Err(ConversationError::Metadata);
        }
        let links = LocalPersonLinks::open(&self.grant_path, source, clock)
            .map_err(|_| ConversationError::Metadata)?;
        *slot = Some(links.clone());
        Ok(links)
    }
}
fn unavailable(_: nessa_local_database::rusqlite::Error) -> PersonLinkError {
    PersonLinkError::Unavailable
}

impl PairingPersonLinks for LocalPersonLinks {
    fn require_absent(&self, record: &PairingRecord) -> Result<(), PersonLinkError> {
        let principal = record
            .credential_principal()
            .ok_or(PersonLinkError::Conflict)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| PersonLinkError::Unavailable)?;
        let found: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM person_links WHERE organization_id=?1 AND gateway_principal=?2)", params![record.intent().resource().organization_id().as_str(), principal.as_str()], |row| row.get(0)).map_err(unavailable)?;
        if found {
            Err(PersonLinkError::Conflict)
        } else {
            Ok(())
        }
    }
    fn reconcile(&self, evidence: &[PairingLinkEvidence]) -> Result<(), PersonLinkError> {
        let observed = self.clock.unix_milliseconds();
        // Validate the whole cut before acquiring write authority or applying effects.
        let changes = evidence
            .iter()
            .map(|fact| {
                if fact
                    .revocation
                    .as_ref()
                    .is_some_and(|revocation| !revocation.matches(&fact.credential, u64::MAX))
                {
                    return Err(PersonLinkError::Conflict);
                }
                PersonLinkChange::from_record(
                    &fact.record,
                    &fact.credential,
                    fact.revocation.as_ref().map(|r| r.transition()),
                    observed / 1000,
                )
                .map_err(|_| PersonLinkError::Conflict)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let changes: Vec<_> = changes.into_iter().flatten().collect();
        let mut credentials = std::collections::HashSet::new();
        if changes
            .iter()
            .any(|change| !credentials.insert(change.credential()))
        {
            return Err(PersonLinkError::Conflict);
        }
        let mut active = std::collections::HashSet::new();
        for change in changes.iter().filter(|change| change.links()) {
            if !active.insert((change.organization(), change.gateway())) {
                return Err(PersonLinkError::Conflict);
            }
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| PersonLinkError::Unavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        // Retire earlier credentials before considering insertions, using the same
        // original evidence on live retries and restart.
        for change in changes
            .iter()
            .filter(|change| !change.links())
            .chain(changes.iter().filter(|change| change.links()))
        {
            let existing: Option<(String,String,String)> = transaction.query_row("SELECT person_principal, credential_id, invitation FROM person_links WHERE organization_id=?1 AND gateway_principal=?2", params![change.organization().as_str(), change.gateway().as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(unavailable)?;
            let invitation = change
                .invitation()
                .bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if change.links() {
                // A committed removal is the receipt that this invitation's
                // choice finished. A later read cannot apply that choice again.
                let retired: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM person_link_changes WHERE credential_id=?1 AND person_after IS NULL)", [change.credential().as_str()], |row| row.get(0)).map_err(unavailable)?;
                if retired {
                    continue;
                }
                match existing.as_ref() {
                    Some((person, credential, original))
                        if person == change.person().as_str()
                            && credential == change.credential().as_str()
                            && original == &invitation =>
                    {
                        continue
                    }
                    Some(_) => return Err(PersonLinkError::Conflict),
                    None => {}
                }
                transaction.execute("INSERT INTO person_links (organization_id,gateway_principal,person_principal,credential_id,invitation) VALUES (?1,?2,?3,?4,?5)", params![change.organization().as_str(),change.gateway().as_str(),change.person().as_str(),change.credential().as_str(),invitation]).map_err(unavailable)?;
            } else {
                let Some((person, credential, original)) = existing.as_ref() else {
                    continue;
                };
                // An old credential's cleanup cannot remove a later association.
                if credential != change.credential().as_str() {
                    continue;
                }
                if person != change.person().as_str() || original != &invitation {
                    return Err(PersonLinkError::Conflict);
                }
                transaction.execute("DELETE FROM person_links WHERE organization_id=?1 AND gateway_principal=?2 AND credential_id=?3", params![change.organization().as_str(),change.gateway().as_str(),change.credential().as_str()]).map_err(unavailable)?;
            }
            let cause = match change.cause() {
                PersonLinkCause::SamePersonChoice => "same_person_choice",
                PersonLinkCause::CredentialRevoked => "credential_revoked",
                PersonLinkCause::CredentialExpired => "credential_expired",
                PersonLinkCause::EnrollmentRevoked => "enrollment_revoked",
            };
            let (kind, id) = match change.initiator() {
                PersonLinkInitiator::Principal(id) => ("principal", Some(id.as_str())),
                PersonLinkInitiator::LocalOperator => ("local_operator", None),
                PersonLinkInitiator::Expiry => ("expiry", None),
            };
            transaction.execute("INSERT INTO person_link_changes (organization_id,gateway_principal,credential_id,invitation,person_before,person_after,cause,initiator_kind,initiator_id,observed_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![change.organization().as_str(),change.gateway().as_str(),change.credential().as_str(),invitation,existing.as_ref().map(|(person,_,_)|person.as_str()),change.links().then(||change.person().as_str()),cause,kind,id,i64::try_from(observed).map_err(|_|PersonLinkError::Conflict)?]).map_err(unavailable)?;
        }
        transaction.commit().map_err(unavailable)
    }
}
