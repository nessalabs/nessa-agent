//! A sealed, bounded-piece baseline for one validated legacy session.
//!
//! This pure codec does not select a storage authority or run a provider. The
//! migration coordinator must hold the legacy writer lease, verify the gateway
//! ownership/deletion cut, append every piece and the seal durably, and switch
//! authority only after that seal is confirmed. Pieces retain the existing typed
//! JSON field mappings; their container is distinct from JSONL change records.

use super::{
    queue_order::QueueEvent,
    records::{Event, Metadata, Provider},
    scheduling::SchedulingEvent,
    tools::corrupt,
    validate,
};
use crate::application::agent_execution::sessions::{SessionSnapshot, StorageError};
use crate::domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext, SessionId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum raw content bytes in one event-stream baseline piece.
pub const MAX_BASELINE_PIECE_BYTES: usize = 64 * 1024;
const BASELINE_VERSION: u8 = 1;
const MAX_SECTION_BYTES: usize = 160 * 1024 * 1024;

/// The exact section of a legacy baseline, in canonical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineSection {
    /// Session and provider identity.
    Header,
    /// Saved global queue decisions.
    Queue,
    /// One invocation and its own ordered observations and decisions.
    Invocation(u32),
}

impl BaselineSection {
    fn index(self) -> u64 {
        match self {
            Self::Header => 0,
            Self::Queue => 1,
            Self::Invocation(index) => u64::from(index) + 2,
        }
    }
}

/// One immutable bounded piece to append with a deterministic event identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselinePiece {
    /// Section this piece belongs to.
    pub section: BaselineSection,
    /// Zero-based part number within the section.
    pub part: u32,
    /// Exact serialized bytes, at most 64 KiB.
    pub bytes: Vec<u8>,
}

/// Final integrity marker. A receiver exposes no imported facts before this seal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineSeal {
    /// Number of pieces included, in canonical order.
    pub piece_count: u64,
    /// Total content bytes across pieces.
    pub content_bytes: u64,
    /// SHA-256 over section identity, part identity, length and bytes of every piece.
    pub digest: [u8; 32],
}

/// One complete immutable import candidate. Storage still must commit its pieces
/// and seal before selecting it as an authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineExport {
    /// Bounded content pieces in canonical order.
    pub pieces: Vec<BaselinePiece>,
    /// Integrity marker for exactly those pieces.
    pub seal: BaselineSeal,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u8,
    id: String,
    provider: Provider,
    provider_context: Option<String>,
    invocation_count: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    metadata: Metadata,
    events: Vec<Event>,
    scheduling: Vec<SchedulingEvent>,
}

fn append_section(
    output: &mut Vec<BaselinePiece>,
    section: BaselineSection,
    bytes: &[u8],
) -> Result<(), StorageError> {
    if bytes.len() > MAX_SECTION_BYTES {
        return Err(corrupt("baseline section exceeds byte limit"));
    }
    for (part, chunk) in bytes.chunks(MAX_BASELINE_PIECE_BYTES).enumerate() {
        output.push(BaselinePiece {
            section,
            part: u32::try_from(part).map_err(|_| corrupt("baseline part count overflow"))?,
            bytes: chunk.to_vec(),
        });
    }
    Ok(())
}

fn seal(pieces: &[BaselinePiece]) -> Result<BaselineSeal, StorageError> {
    let mut hash = Sha256::new();
    let mut content_bytes = 0_u64;
    for piece in pieces {
        if piece.bytes.is_empty() || piece.bytes.len() > MAX_BASELINE_PIECE_BYTES {
            return Err(corrupt("invalid baseline piece size"));
        }
        let length =
            u64::try_from(piece.bytes.len()).map_err(|_| corrupt("piece size overflow"))?;
        content_bytes = content_bytes
            .checked_add(length)
            .ok_or_else(|| corrupt("baseline size overflow"))?;
        hash.update(piece.section.index().to_be_bytes());
        hash.update(piece.part.to_be_bytes());
        hash.update(length.to_be_bytes());
        hash.update(&piece.bytes);
    }
    Ok(BaselineSeal {
        piece_count: u64::try_from(pieces.len())
            .map_err(|_| corrupt("baseline piece count overflow"))?,
        content_bytes,
        digest: hash.finalize().into(),
    })
}

/// Encodes a validated legacy snapshot as deterministic bounded sections.
/// The caller must still hold its writer lease and verify the source cut.
/// No effect is inferred from saved input or provider observations.
pub fn encode_baseline(snapshot: &SessionSnapshot) -> Result<BaselineExport, StorageError> {
    validate(snapshot)?;
    let mut pieces = Vec::new();
    let header = Header {
        version: BASELINE_VERSION,
        id: snapshot.id.as_str().into(),
        provider: Provider {
            name: snapshot.provider.name().into(),
            model_id: snapshot.provider.model_id().into(),
            context: snapshot.provider.context().into(),
        },
        provider_context: snapshot
            .provider_context
            .recorded()
            .map(|id| id.as_str().into()),
        invocation_count: snapshot.invocations.len(),
    };
    append_section(
        &mut pieces,
        BaselineSection::Header,
        &serde_json::to_vec(&header).map_err(corrupt)?,
    )?;
    let queue: Vec<_> = snapshot
        .queue_history
        .iter()
        .map(QueueEvent::from)
        .collect();
    append_section(
        &mut pieces,
        BaselineSection::Queue,
        &serde_json::to_vec(&queue).map_err(corrupt)?,
    )?;
    for (index, record) in snapshot.invocations.iter().enumerate() {
        let invocation = Invocation {
            metadata: Metadata::from(record),
            events: record.events.iter().cloned().map(Event::from).collect(),
            scheduling: record
                .scheduling
                .iter()
                .cloned()
                .map(SchedulingEvent::from)
                .collect(),
        };
        append_section(
            &mut pieces,
            BaselineSection::Invocation(
                u32::try_from(index).map_err(|_| corrupt("invocation count overflow"))?,
            ),
            &serde_json::to_vec(&invocation).map_err(corrupt)?,
        )?;
    }
    Ok(BaselineExport {
        seal: seal(&pieces)?,
        pieces,
    })
}

fn collect_section(
    pieces: &[BaselinePiece],
    cursor: &mut usize,
    section: BaselineSection,
) -> Result<Vec<u8>, StorageError> {
    let mut bytes = Vec::new();
    let mut expected_part = 0_u32;
    while let Some(piece) = pieces.get(*cursor) {
        if piece.section != section {
            break;
        }
        if piece.part != expected_part
            || piece.bytes.is_empty()
            || piece.bytes.len() > MAX_BASELINE_PIECE_BYTES
        {
            return Err(corrupt("baseline piece is missing, repeated or oversized"));
        }
        let next = bytes
            .len()
            .checked_add(piece.bytes.len())
            .ok_or_else(|| corrupt("baseline section size overflow"))?;
        if next > MAX_SECTION_BYTES {
            return Err(corrupt("baseline section exceeds byte limit"));
        }
        bytes.extend_from_slice(&piece.bytes);
        expected_part = expected_part
            .checked_add(1)
            .ok_or_else(|| corrupt("baseline part count overflow"))?;
        *cursor += 1;
    }
    if bytes.is_empty() {
        return Err(corrupt("baseline section is missing"));
    }
    Ok(bytes)
}

/// Checks the seal and exact piece order, decodes typed fields, then runs the
/// same application validation as the original session-storage boundary.
/// A partial import or a changed byte is a typed corruption error.
pub fn decode_baseline(export: &BaselineExport) -> Result<SessionSnapshot, StorageError> {
    if seal(&export.pieces)? != export.seal {
        return Err(corrupt("baseline seal does not match pieces"));
    }
    let mut cursor = 0;
    let header: Header = serde_json::from_slice(&collect_section(
        &export.pieces,
        &mut cursor,
        BaselineSection::Header,
    )?)
    .map_err(corrupt)?;
    if header.version != BASELINE_VERSION
        || header.invocation_count > SessionSnapshot::MAX_INVOCATIONS
    {
        return Err(corrupt("unsupported baseline version or invocation count"));
    }
    let queue: Vec<QueueEvent> = serde_json::from_slice(&collect_section(
        &export.pieces,
        &mut cursor,
        BaselineSection::Queue,
    )?)
    .map_err(corrupt)?;
    let mut snapshot = SessionSnapshot {
        id: SessionId::new(header.id).map_err(corrupt)?,
        provider: header.provider.decode()?,
        provider_context: header
            .provider_context
            .map(ExecutionSessionId::new)
            .transpose()
            .map_err(corrupt)?
            .map_or(ProviderContext::Absent, ProviderContext::Recorded),
        queue_history: queue
            .into_iter()
            .map(QueueEvent::decode)
            .collect::<Result<_, _>>()?,
        invocations: Vec::with_capacity(header.invocation_count),
    };
    for index in 0..header.invocation_count {
        let section = BaselineSection::Invocation(
            u32::try_from(index).map_err(|_| corrupt("invocation count overflow"))?,
        );
        let saved: Invocation =
            serde_json::from_slice(&collect_section(&export.pieces, &mut cursor, section)?)
                .map_err(corrupt)?;
        let mut record = saved.metadata.decode()?;
        for event in saved.events {
            let context = snapshot
                .provider_context
                .recorded()
                .ok_or_else(|| corrupt("provider observation has no provider context"))?;
            record
                .events
                .push(event.decode(context, &record.request.execution_id)?);
        }
        record.scheduling = saved
            .scheduling
            .into_iter()
            .map(SchedulingEvent::decode)
            .collect::<Result<_, _>>()?;
        snapshot.invocations.push(record);
    }
    if cursor != export.pieces.len() {
        return Err(corrupt("baseline has trailing pieces"));
    }
    validate(&snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::providers::ProviderIdentity;

    #[test]
    fn a_valid_seal_does_not_make_an_unknown_payload_version_valid() {
        let snapshot = SessionSnapshot {
            id: SessionId::new("baseline-version").unwrap(),
            provider: ProviderIdentity::new("fixture", "model", "context").unwrap(),
            provider_context: ProviderContext::Absent,
            queue_history: Vec::new(),
            invocations: Vec::new(),
        };
        let mut export = encode_baseline(&snapshot).unwrap();
        let mut header: serde_json::Value =
            serde_json::from_slice(&export.pieces[0].bytes).unwrap();
        header["version"] = 2.into();
        export.pieces[0].bytes = serde_json::to_vec(&header).unwrap();
        export.seal = seal(&export.pieces).unwrap();
        assert!(matches!(
            decode_baseline(&export),
            Err(StorageError::Corrupt(_))
        ));
    }
}
