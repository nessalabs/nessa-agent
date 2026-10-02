use super::{Command, CommandError, Operation};
use crate::read_only_sync::application::offline::{SavedReads, SavedTranscript};
use crate::read_only_sync::{
    application::{reset::CacheResets, CachedProgress},
    domain::CacheReset,
};
use nessa_sync::replication::catalogue::MAX_CATALOGUE_ENTRIES;
use nessa_sync::replication::{catalogue::CatalogueProgress, domain::Scope};
use serde_json::{json, Value};
use std::io::Write;
use uuid::Uuid;

pub(crate) fn run_local<C: SavedReads + CacheResets>(
    command: &Command,
    cache: &mut C,
    revision: Uuid,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let Command::Local(command) = command else {
        return Err(CommandError::Arguments);
    };
    let data = match &command.operation {
        Operation::ResetRecords(request) => {
            let receipt = cache.reset_records(request).map_err(CommandError::Cache)?;
            json!({"state":"reset", "connectionCheck":"notPerformed",
                "request":reset_request(receipt.request()), "before":record_progress(receipt.before()),
                "after":record_progress(receipt.after()), "observedAtMs":receipt.observed_at_ms()})
        }
        Operation::ResetCatalogue(request) => {
            let receipt = cache
                .reset_catalogue(request)
                .map_err(CommandError::Cache)?;
            json!({"state":"reset", "connectionCheck":"notPerformed",
                "request":reset_request(receipt.request()), "before":catalogue_progress(receipt.before()),
                "after":catalogue_progress(receipt.after()), "observedAtMs":receipt.observed_at_ms()})
        }
        Operation::List { stream, after } => {
            match cache
                .catalogue_page(
                    &command.receiver,
                    &command.origin,
                    stream,
                    after.as_ref(),
                    MAX_CATALOGUE_ENTRIES,
                )
                .map_err(CommandError::Cache)?
            {
                None => json!({"state":"notLoaded", "connectionCheck":"notPerformed"}),
                Some(page) => {
                    let entries: Vec<Value> = page.entries().iter().map(|entry| {
                        let descriptor = entry.manifest();
                        let metadata = entry.metadata().map(|value| json!({
                            "id": value.id().to_string(), "createdAtMs": value.created_at_ms(),
                            "agent": value.agent().map(|agent| agent.name()),
                            "model": value.model().as_str(), "approvalMode": value.approval_mode().as_str(),
                            "summary": value.summary().map(|summary| json!({
                                "title":summary.title().map(|title| title.as_str()),
                                "preview":summary.preview().map(|preview| preview.as_str()),
                                "updatedAtMs":summary.updated_at_ms(), "archived":summary.archived(),
                            })),
                        }));
                        json!({"id":descriptor.key.id.as_str(), "creation":descriptor.key.creation.to_string(),
                            "revision":descriptor.revision.to_string(), "deleted":descriptor.deleted, "metadata":metadata})
                    }).collect();
                    let progress = page.progress();
                    let state = if progress.active.is_some() {
                        "partial"
                    } else {
                        "stale"
                    };
                    json!({"state":state, "connectionCheck":"notPerformed",
                        "completedRevision":progress.completed.to_string(),
                        "activeBoundary":progress.active.as_ref().map(|pass| pass.boundary.to_string()),
                        "entries":entries,
                        "next":page.next().map(|key| json!({"creation":key.creation.to_string(),"id":key.id.as_str()})),
                    })
                }
            }
        }
        Operation::Show(conversation) => match cache
            .transcript_view(&command.receiver, &command.origin, conversation, revision)
            .map_err(CommandError::Cache)?
        {
            SavedTranscript::NotLoaded => {
                json!({"state":"notLoaded", "connectionCheck":"notPerformed"})
            }
            SavedTranscript::Deleted => {
                json!({"state":"deleted", "connectionCheck":"notPerformed"})
            }
            SavedTranscript::Retained { progress, view } => json!({
                "connectionCheck":"notPerformed", "downloaded":progress.downloaded.to_string(),
                "applied":progress.applied.to_string(), "facts":progress.facts.to_string(), "view":view,
            }),
        },
    };
    serde_json::to_writer(&mut *output, &data).map_err(|_| CommandError::Output)?;
    output.write_all(b"\n").map_err(|_| CommandError::Output)
}

pub(super) fn scope(value: &Scope) -> Value {
    json!({"receiver":value.receiver().as_str(), "origin":value.origin().as_str(),
        "stream":value.stream().as_str(), "incarnation":value.incarnation().as_str(),
        "schema":value.schema().as_str(), "epoch":value.access_epoch().as_str()})
}

fn reset_request(value: &CacheReset) -> Value {
    json!({"operation":value.operation().as_str(), "caller":value.caller().as_str(),
        "cause":"ExplicitReset", "initiator":"LocalOperator",
        "expected":scope(value.expected()), "replacement":scope(value.replacement()),
        "generation":value.generation().to_string()})
}

pub(super) fn record_progress(value: &CachedProgress) -> Value {
    json!({"scope":scope(&value.scope), "downloaded":value.downloaded.to_string(),
        "applied":value.applied.to_string(), "facts":value.facts.to_string(),
        "generation":value.generation.to_string()})
}

pub(super) fn catalogue_progress(value: &CatalogueProgress) -> Value {
    json!({"scope":scope(&value.scope), "completed":value.completed.to_string(),
        "generation":value.generation.to_string(), "active":value.active.as_ref().map(|pass|
            json!({"boundary":pass.boundary.to_string(), "cursor":pass.cursor.as_ref().map(|key|
                json!({"creation":key.creation.to_string(), "id":key.id.as_str()}))}))})
}
