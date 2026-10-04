//! One current JSON representation for source and private-cache metadata.
//! Decode bounds raw input before parsing and asks product field owners. Core
//! descriptor/revision/page checks and cache atomicity remain with their owners.

use super::catalogue_metadata::CatalogueMetadata;
use super::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId, ConversationPreview,
    ConversationSummary, ConversationTitle,
};
use crate::agents::AgentId;
use nessa_sync::replication::catalogue::MAX_CATALOGUE_PAYLOAD_BYTES;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Why a catalogue payload could not be written or read.
pub enum CataloguePayloadError {
    Oversized,
    Malformed,
    Metadata,
    WrongEntry,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawMetadata {
    id: String,
    created_at_ms: u64,
    #[serde(deserialize_with = "required_option")]
    agent: Option<String>,
    model: String,
    approval_mode: String,
    #[serde(deserialize_with = "required_option")]
    summary: Option<RawSummary>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawSummary {
    #[serde(deserialize_with = "required_option")]
    title: Option<String>,
    #[serde(deserialize_with = "required_option")]
    preview: Option<String>,
    updated_at_ms: u64,
    archived: bool,
}

fn required_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

fn correlate_entry(metadata: &str, expected: &str) -> Result<(), CataloguePayloadError> {
    if metadata != expected {
        return Err(CataloguePayloadError::WrongEntry);
    }
    Ok(())
}

/// Write `metadata` as the catalogue entry `entry`'s payload. The entry and
/// the metadata must name the same conversation.
pub fn encode(entry: &str, metadata: &CatalogueMetadata) -> Result<Vec<u8>, CataloguePayloadError> {
    let raw = RawMetadata {
        id: metadata.id().to_string(),
        created_at_ms: metadata.created_at_ms(),
        agent: metadata.agent().map(|agent| agent.name().to_owned()),
        model: metadata.model().as_str().to_owned(),
        approval_mode: metadata.approval_mode().as_str().to_owned(),
        summary: metadata.summary().map(|summary| RawSummary {
            title: summary.title().map(|title| title.as_str().to_owned()),
            preview: summary.preview().map(|preview| preview.as_str().to_owned()),
            updated_at_ms: summary.updated_at_ms(),
            archived: summary.archived(),
        }),
    };
    correlate_entry(&raw.id, entry)?;
    // Value uses the same map ordering as the former JSON-object writer,
    // including a workspace that enables serde_json's preserve_order feature.
    let value = serde_json::to_value(raw).map_err(|_| CataloguePayloadError::Malformed)?;
    let payload = serde_json::to_vec(&value).map_err(|_| CataloguePayloadError::Malformed)?;
    if payload.len() > MAX_CATALOGUE_PAYLOAD_BYTES {
        return Err(CataloguePayloadError::Oversized);
    }
    Ok(payload)
}

/// Read the catalogue entry `expected`'s payload back into its metadata.
pub fn decode(expected: &str, payload: &[u8]) -> Result<CatalogueMetadata, CataloguePayloadError> {
    if payload.len() > MAX_CATALOGUE_PAYLOAD_BYTES {
        return Err(CataloguePayloadError::Oversized);
    }
    let raw: RawMetadata =
        serde_json::from_slice(payload).map_err(|_| CataloguePayloadError::Malformed)?;
    correlate_entry(&raw.id, expected)?;
    let id = ConversationId::new(&raw.id).map_err(|_| CataloguePayloadError::Metadata)?;
    let agent = raw
        .agent
        .as_deref()
        .map(|name| AgentId::parse(name).ok_or(CataloguePayloadError::Metadata))
        .transpose()?;
    let model = ConversationModelId::new(raw.model).map_err(|_| CataloguePayloadError::Metadata)?;
    let approval = ConversationApprovalMode::parse(&raw.approval_mode)
        .ok_or(CataloguePayloadError::Metadata)?;
    let summary = raw
        .summary
        .map(|summary| {
            let title = summary
                .title
                .as_deref()
                .map(ConversationTitle::new)
                .transpose()
                .map_err(|_| CataloguePayloadError::Metadata)?;
            let preview = summary
                .preview
                .as_deref()
                .map(ConversationPreview::new)
                .transpose()
                .map_err(|_| CataloguePayloadError::Metadata)?;
            ConversationSummary::new(title, preview, summary.updated_at_ms, summary.archived)
                .map_err(|_| CataloguePayloadError::Metadata)
        })
        .transpose()?;
    CatalogueMetadata::new(id, raw.created_at_ms, agent, model, approval, summary)
        .map_err(|_| CataloguePayloadError::Metadata)
}

#[cfg(test)]
#[path = "../../tests/conversation/catalogue_payload.rs"]
mod tests;
