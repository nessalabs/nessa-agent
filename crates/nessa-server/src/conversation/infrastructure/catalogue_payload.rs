//! One current JSON representation for source and private-cache metadata.
//! Decode bounds raw input before parsing and asks product field owners. Core
//! descriptor/revision/page checks and cache atomicity remain with their owners.

use crate::{
    agents::domain::AgentId,
    conversation::{
        application::{CatalogueMetadata, CatalogueValue},
        domain::{
            ConversationApprovalMode, ConversationId, ConversationModelId, ConversationPreview,
            ConversationSummary, ConversationTitle,
        },
    },
};
use nessa_sync::replication::catalogue::MAX_CATALOGUE_PAYLOAD_BYTES;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CataloguePayloadError {
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

pub(crate) fn encode(value: &CatalogueValue) -> Result<Vec<u8>, CataloguePayloadError> {
    let conversation = &value.conversation;
    let metadata = RawMetadata {
        id: conversation.id().to_string(),
        created_at_ms: conversation.creation_requested_at_ms(),
        agent: conversation.agent().map(|agent| agent.name().to_owned()),
        model: conversation.model().as_str().to_owned(),
        approval_mode: conversation.approval_mode().as_str().to_owned(),
        summary: value.summary.as_ref().map(|summary| RawSummary {
            title: summary.title().map(|title| title.as_str().to_owned()),
            preview: summary.preview().map(|preview| preview.as_str().to_owned()),
            updated_at_ms: summary.updated_at_ms(),
            archived: summary.archived(),
        }),
    };
    correlate_entry(&metadata.id, &value.descriptor.key.id.to_string())?;
    // Value uses the same map ordering as the former JSON-object writer,
    // including a workspace that enables serde_json's preserve_order feature.
    let value = serde_json::to_value(metadata).map_err(|_| CataloguePayloadError::Malformed)?;
    let payload = serde_json::to_vec(&value).map_err(|_| CataloguePayloadError::Malformed)?;
    if payload.len() > MAX_CATALOGUE_PAYLOAD_BYTES {
        return Err(CataloguePayloadError::Oversized);
    }
    Ok(payload)
}

pub(crate) fn decode(
    expected: &str,
    payload: &[u8],
) -> Result<CatalogueMetadata, CataloguePayloadError> {
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
#[path = "../../../tests/conversation/catalogue_payload.rs"]
mod tests;
