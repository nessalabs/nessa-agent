//! Exact registration lookup, retained tombstone movement and local manifest reads.
use super::super::hold_record::{HoldRecord, RecordState};
use super::{archive_path, corrupt, hold_directory, path_of, Files};
use crate::{attachments::domain::ArtifactId, conversation::domain::ConversationId};
use nessa_auth::domain::OrganizationId;
use nessa_local_storage::{replace_beneath, sync_directory_beneath};
use std::io;

impl Files {
    pub(super) fn registration(
        &self,
        organization: &OrganizationId,
        conversation: &ConversationId,
        id: &ArtifactId,
    ) -> io::Result<Option<HoldRecord>> {
        let directory = hold_directory(organization, conversation);
        let archive = directory.join(format!("retired-{}.json", id.as_str()));
        let mut found = self.read_record(&archive)?;
        self.visit_primary_records(organization, conversation, |_, record| {
            let record = record?;
            if ArtifactId::from_generation(&record.generation) == *id {
                if let Some(previous) = &found {
                    if previous != &record || !matches!(&record.state, RecordState::Retired { .. })
                    {
                        return Err(corrupt(
                            "registration identity has conflicting saved records",
                        ));
                    }
                }
                found = Some(record);
            }
            Ok(())
        })?;
        Ok(found)
    }

    pub(super) fn archive(&self, record: &HoldRecord) -> io::Result<()> {
        let destination = archive_path(&record.hold, &record.generation);
        if let Some(previous) = self.read_record(&destination)? {
            if previous != *record {
                return Err(corrupt("retired registration archive conflicts"));
            }
            // The identical durable tombstone already exists; only the
            // redundant Retired primary name is removed.
            self.remove(&path_of(&record.hold))?;
        } else {
            let directory = destination
                .parent()
                .ok_or_else(|| corrupt("archive has no directory"))?;
            sync_directory_beneath(&self.root, directory)?;
            replace_beneath(&self.root, &path_of(&record.hold), &destination)?;
            #[cfg(test)]
            self.fail_publication_at(super::PublicationFault::AfterArchiveMove)?;
            sync_directory_beneath(&self.root, directory)?;
        }
        Ok(())
    }
}
