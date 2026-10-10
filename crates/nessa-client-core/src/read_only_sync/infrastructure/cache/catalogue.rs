//! Host catalogue transactions share the record cache connection and deletion fences.
use super::{catalogue_rows, raw_records, rows, ReadOnlyCache};
use crate::read_only_sync::application::CacheError;
use nessa_local_database::rusqlite::{params, Connection, TransactionBehavior};
use nessa_sync::replication::{
    catalogue::{
        catalogue_progress_after_begin, catalogue_progress_after_page,
        validate_catalogue_revision_transition, CataloguePagePlan, CatalogueProgress,
        CatalogueStore, CatalogueStoreError, CatalogueValidationError, ResolvedEntry,
    },
    domain::{Id, Scope},
};

impl ReadOnlyCache {
    /// Remove one conversation the source no longer grants this receiver: its
    /// catalogue entry and its transcript, in one transaction. Catalogue
    /// progress stays, and no deletion fence is written, so a later grant
    /// (a newer revision) brings the conversation back whole.
    pub(crate) fn withdraw(&mut self, catalogue: &Scope, id: &Id) -> Result<(), CacheError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        tx.execute(
            "DELETE FROM catalogue_entries WHERE receiver=?1 AND origin=?2 AND stream=?3 AND entry_id=?4",
            params![
                catalogue.receiver().as_str(),
                catalogue.origin().as_str(),
                catalogue.stream().as_str(),
                id.as_str()
            ],
        )
        .map_err(rows::database_error)?;
        raw_records::clear_transcript_target(&tx, catalogue.receiver(), catalogue.origin(), id)?;
        let committed = tx.commit();
        // Whatever the outcome, no loaded fold may outlive rows it came from.
        self.invalidate_loaded();
        committed.map_err(|_| CacheError::Uncertain)
    }

    pub(crate) fn catalogue_entry(
        &mut self,
        scope: &Scope,
        id: &Id,
    ) -> Result<Option<ResolvedEntry>, CacheError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(rows::database_error)?;
        catalogue_rows::exact_progress(&tx, scope)?;
        let result = catalogue_rows::entry(&tx, scope, id)?;
        tx.commit().map_err(rows::database_error)?;
        Ok(result)
    }

    fn begin_catalogue(
        &mut self,
        scope: &Scope,
        expected: Option<CatalogueProgress>,
        boundary: u64,
    ) -> Result<CatalogueProgress, CacheError> {
        let replacement = catalogue_progress_after_begin(scope, expected.as_ref(), boundary)
            .map_err(CacheError::CatalogueProgress)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        let current = catalogue_rows::exact_progress(&tx, scope)?;
        if current != expected {
            return Err(CacheError::Stale);
        }
        catalogue_rows::save_progress(&tx, &replacement)?;
        tx.commit().map_err(|_| CacheError::Uncertain)?;
        Ok(replacement)
    }

    fn apply_catalogue(
        &mut self,
        plan: CataloguePagePlan,
    ) -> Result<CatalogueProgress, CacheError> {
        let replacement =
            catalogue_progress_after_page(&plan).map_err(CacheError::CatalogueProgress)?;
        for value in &plan.entries {
            catalogue_rows::metadata(value)?;
        }
        let scope = &plan.pass.scope;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        let current = catalogue_rows::exact_progress(&tx, scope)?.ok_or(CacheError::Stale)?;
        if current.active.as_ref() != Some(&plan.pass) {
            return Err(CacheError::Stale);
        }
        for unchanged in &plan.unchanged {
            let saved =
                catalogue_rows::entry(&tx, scope, &unchanged.key.id)?.ok_or(CacheError::Stale)?;
            validate_catalogue_revision_transition(unchanged, &saved.manifest)
                .map_err(revision_error)?;
        }
        let mut invalidated = false;
        for value in &plan.entries {
            let saved = catalogue_rows::entry(&tx, scope, &value.manifest.key.id)?;
            if let Some(saved) = &saved {
                let (earlier, later) = if saved.manifest.revision > value.manifest.revision {
                    (&value.manifest, &saved.manifest)
                } else {
                    (&saved.manifest, &value.manifest)
                };
                validate_catalogue_revision_transition(earlier, later).map_err(revision_error)?;
                if saved.manifest.revision > value.manifest.revision {
                    continue;
                }
                if saved.manifest.revision == value.manifest.revision {
                    if saved.payload != value.payload {
                        return Err(CacheError::ConflictingRecord);
                    }
                    continue;
                }
            }
            let fenced = catalogue_rows::fence_state(&tx, scope, &value.manifest)?;
            if value.manifest.deleted && !fenced {
                deletion(
                    &tx,
                    scope,
                    value,
                    saved.as_ref(),
                    self.clock.unix_milliseconds(),
                )?;
                invalidated = true;
            }
            catalogue_rows::save_entry(&tx, scope, value)?;
        }
        catalogue_rows::save_progress(&tx, &replacement)?;
        if tx.commit().is_err() {
            self.invalidate_loaded();
            return Err(CacheError::Uncertain);
        }
        if invalidated {
            self.invalidate_loaded();
        }
        Ok(replacement)
    }

    fn catalogue_result<T>(
        &mut self,
        result: Result<T, CacheError>,
    ) -> Result<T, CatalogueStoreError> {
        let _previous = self.take_refusal();
        result.map_err(|error| {
            let refusal = match &error {
                CacheError::CatalogueProgress(error) => CatalogueStoreError::from(error.clone()),
                CacheError::Stale => CatalogueStoreError::Stale,
                CacheError::Uncertain => CatalogueStoreError::Uncertain,
                CacheError::Scope { .. } => CatalogueStoreError::ResetRequired,
                CacheError::Fenced => CatalogueStoreError::Fenced,
                CacheError::ConflictingRecord | CacheError::CatalogueValidation(_) => {
                    CatalogueStoreError::Conflict
                }
                _ => CatalogueStoreError::Failed,
            };
            self.catalogue_refusal(error);
            refusal
        })
    }
}

impl CatalogueStore for ReadOnlyCache {
    fn progress(
        &mut self,
        scope: &Scope,
    ) -> Result<Option<CatalogueProgress>, CatalogueStoreError> {
        let result = catalogue_rows::progress(&self.connection, scope);
        self.catalogue_result(result)
    }
    fn begin(
        &mut self,
        scope: &Scope,
        expected: Option<CatalogueProgress>,
        boundary: u64,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let result = self.begin_catalogue(scope, expected, boundary);
        self.catalogue_result(result)
    }
    fn cached_revision(
        &mut self,
        scope: &Scope,
        id: &Id,
    ) -> Result<Option<u64>, CatalogueStoreError> {
        let result = self
            .catalogue_entry(scope, id)
            .map(|entry| entry.map(|value| value.manifest.revision));
        self.catalogue_result(result)
    }
    fn apply_page(
        &mut self,
        plan: CataloguePagePlan,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        let result = self.apply_catalogue(plan);
        self.catalogue_result(result)
    }
    fn reset(
        &mut self,
        _scope: &Scope,
        _expected: CatalogueProgress,
    ) -> Result<CatalogueProgress, CatalogueStoreError> {
        self.catalogue_result(Err(CacheError::ResetAttributionRequired))
    }
}

fn revision_error(error: CatalogueValidationError) -> CacheError {
    match error {
        CatalogueValidationError::DeletionFence => CacheError::Fenced,
        other => CacheError::CatalogueValidation(other),
    }
}

fn deletion(
    connection: &Connection,
    scope: &Scope,
    value: &ResolvedEntry,
    before: Option<&ResolvedEntry>,
    observed: u64,
) -> Result<(), CacheError> {
    let key = &value.manifest.key;
    // The catalogue supplies only the stable transcript key. The stored row
    // supplies its actual incarnation/schema/epoch; no catalogue scope is used
    // as a stand-in for a transcript scope.
    let transcript = rows::progress_target(connection, scope.receiver(), scope.origin(), &key.id)?;
    let before_revision = before.map(|value| value.manifest.revision.to_be_bytes());
    let downloaded = transcript
        .as_ref()
        .map(|progress| progress.downloaded.to_be_bytes());
    let applied = transcript
        .as_ref()
        .map(|progress| progress.applied.to_be_bytes());
    let facts = transcript
        .as_ref()
        .map(|progress| progress.facts.to_be_bytes());
    let generation = transcript
        .as_ref()
        .map(|progress| progress.generation.to_be_bytes());
    connection.execute(
        "INSERT INTO catalogue_deletions(receiver,origin,conversation,catalogue_stream,catalogue_incarnation,catalogue_schema,access_epoch,creation,revision,before_revision,before_deleted,transcript_downloaded,transcript_applied,transcript_generation,observed_at_ms,transcript_incarnation,transcript_schema,transcript_epoch,transcript_facts,cause,initiator)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,'SourceDeletion','RemoteCatalogue')",
        params![scope.receiver().as_str(),scope.origin().as_str(),key.id.as_str(),scope.stream().as_str(),scope.incarnation().as_str(),scope.schema().as_str(),scope.access_epoch().as_str(),key.creation.to_be_bytes().as_slice(),value.manifest.revision.to_be_bytes().as_slice(),before_revision.as_ref().map(|v|v.as_slice()),before.map(|value|i64::from(value.manifest.deleted)),downloaded.as_ref().map(|v|v.as_slice()),applied.as_ref().map(|v|v.as_slice()),generation.as_ref().map(|v|v.as_slice()),observed.to_be_bytes().as_slice(),transcript.as_ref().map(|p|p.scope.incarnation().as_str()),transcript.as_ref().map(|p|p.scope.schema().as_str()),transcript.as_ref().map(|p|p.scope.access_epoch().as_str()),facts.as_ref().map(|v|v.as_slice())],
    ).map_err(rows::database_error)?;
    raw_records::clear_transcript_target(connection, scope.receiver(), scope.origin(), &key.id)
}
