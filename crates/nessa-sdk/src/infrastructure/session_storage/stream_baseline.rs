//! Nessa's sealed legacy baseline on the generic event-stream record ports.
//! The caller holds the legacy writer lease and checks the gateway deletion cut.
//! A successful return confirms the seal; only composition may switch authority.

use super::snapshot::MAX_BASELINE_PIECE_BYTES;
use super::{decode_baseline, BaselineExport, BaselinePiece, BaselineSeal, BaselineSection};
use crate::application::agent_execution::sessions::SessionSnapshot;
use event_stream::{
    Cursor, EventReader, EventRuntime, NewEvent, PageLimits, Payload, SchemaId, SchemaRef,
    StreamKey,
};
use std::{error::Error, fmt};

const OPEN_SCHEMA: &str = "nessa.baseline-open";
const PIECE_SCHEMA: &str = "nessa.baseline-piece";
const SEAL_SCHEMA: &str = "nessa.baseline-seal";
const VERSION: u8 = 1;
const PAGE: PageLimits = PageLimits {
    max_records: 64,
    max_bytes: 1024 * 1024,
};

/// An import fails without selecting the target as the session authority.
#[derive(Debug)]
pub enum BaselineImportError {
    /// The source baseline does not pass the SDK's session validation.
    Source(crate::application::agent_execution::sessions::StorageError),
    /// The record store failed or did not confirm the write.
    Stream(event_stream::Error),
    /// The stream contains records outside this exact import candidate.
    ConflictingStream,
    /// The store returned a noncontiguous or mismatched record or receipt.
    InvalidStream,
}

/// What a fresh process can prove from the committed stream prefix.
#[derive(Debug)]
pub enum BaselineLoad {
    /// No baseline record has been written.
    Absent,
    /// Only numbered pieces exist; the legacy source remains authoritative.
    Partial,
    /// A matching seal commits a validated snapshot and names its last cursor.
    Sealed {
        /// Reconstructed source facts; reading them starts no effects.
        snapshot: SessionSnapshot,
        /// The next fold starts strictly after this cursor.
        cursor: Cursor,
    },
}

impl From<event_stream::Error> for BaselineImportError {
    fn from(error: event_stream::Error) -> Self {
        Self::Stream(error)
    }
}

impl fmt::Display for BaselineImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => write!(f, "legacy baseline: {error}"),
            Self::Stream(error) => write!(f, "record stream: {error}"),
            Self::ConflictingStream => f.write_str("record stream has a different import prefix"),
            Self::InvalidStream => f.write_str("record stream returned invalid import evidence"),
        }
    }
}

impl Error for BaselineImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Stream(error) => Some(error),
            Self::ConflictingStream | Self::InvalidStream => None,
        }
    }
}

fn event_id(
    seal: &BaselineSeal,
    index: usize,
) -> Result<event_stream::EventId, BaselineImportError> {
    let digest = seal
        .digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    event_stream::EventId::new(format!("nessa-baseline-{digest}-{index}"))
        .map_err(|_| BaselineImportError::InvalidStream)
}

fn piece_payload(piece: &BaselinePiece) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(18 + piece.bytes.len());
    bytes.push(VERSION);
    bytes.extend_from_slice(&piece.section.index().to_be_bytes());
    bytes.extend_from_slice(&piece.part.to_be_bytes());
    bytes.extend_from_slice(&(piece.bytes.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&piece.bytes);
    bytes
}

fn seal_payload(seal: &BaselineSeal) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(49);
    bytes.push(VERSION);
    bytes.extend_from_slice(&seal.piece_count.to_be_bytes());
    bytes.extend_from_slice(&seal.content_bytes.to_be_bytes());
    bytes.extend_from_slice(&seal.digest);
    bytes
}

fn open_payload(session_id: &str, seal: &BaselineSeal) -> Result<Vec<u8>, BaselineImportError> {
    let id = session_id.as_bytes();
    let length = u16::try_from(id.len()).map_err(|_| BaselineImportError::InvalidStream)?;
    let mut bytes = Vec::with_capacity(35 + id.len());
    bytes.push(VERSION);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(id);
    bytes.extend_from_slice(&seal.digest);
    Ok(bytes)
}

fn check_open(bytes: &[u8]) -> Result<(), BaselineImportError> {
    if bytes.len() < 36 || bytes[0] != VERSION {
        return Err(BaselineImportError::InvalidStream);
    }
    let id_length = u16::from_be_bytes(bytes[1..3].try_into().unwrap()) as usize;
    if id_length == 0 || bytes.len() != 35 + id_length {
        return Err(BaselineImportError::InvalidStream);
    }
    let id = std::str::from_utf8(&bytes[3..3 + id_length])
        .map_err(|_| BaselineImportError::InvalidStream)?;
    crate::domain::agent_execution::sessions::SessionId::new(id)
        .map_err(|_| BaselineImportError::InvalidStream)?;
    Ok(())
}

fn read_piece(bytes: &[u8]) -> Result<BaselinePiece, BaselineImportError> {
    if bytes.len() < 17 || bytes[0] != VERSION {
        return Err(BaselineImportError::InvalidStream);
    }
    let section = BaselineSection::from_index(u64::from_be_bytes(bytes[1..9].try_into().unwrap()))
        .ok_or(BaselineImportError::InvalidStream)?;
    let part = u32::from_be_bytes(bytes[9..13].try_into().unwrap());
    let length = u32::from_be_bytes(bytes[13..17].try_into().unwrap()) as usize;
    if length == 0 || length > MAX_BASELINE_PIECE_BYTES || bytes.len() != 17 + length {
        return Err(BaselineImportError::InvalidStream);
    }
    Ok(BaselinePiece {
        section,
        part,
        bytes: bytes[17..].to_vec(),
    })
}

fn read_seal(bytes: &[u8]) -> Result<BaselineSeal, BaselineImportError> {
    if bytes.len() != 49 || bytes[0] != VERSION {
        return Err(BaselineImportError::InvalidStream);
    }
    Ok(BaselineSeal {
        piece_count: u64::from_be_bytes(bytes[1..9].try_into().unwrap()),
        content_bytes: u64::from_be_bytes(bytes[9..17].try_into().unwrap()),
        digest: bytes[17..49].try_into().unwrap(),
    })
}

fn expected_records(export: &BaselineExport) -> Result<Vec<NewEvent>, BaselineImportError> {
    let snapshot = decode_baseline(export).map_err(BaselineImportError::Source)?;
    let open_schema = SchemaRef {
        id: SchemaId::new(OPEN_SCHEMA).map_err(|_| BaselineImportError::InvalidStream)?,
        version: 1,
    };
    let piece_schema = SchemaRef {
        id: SchemaId::new(PIECE_SCHEMA).map_err(|_| BaselineImportError::InvalidStream)?,
        version: 1,
    };
    let seal_schema = SchemaRef {
        id: SchemaId::new(SEAL_SCHEMA).map_err(|_| BaselineImportError::InvalidStream)?,
        version: 1,
    };
    let mut records = Vec::with_capacity(export.pieces.len() + 2);
    records.push(NewEvent {
        id: event_id(&export.seal, 0)?,
        schema: open_schema,
        payload: Payload::copy_from_slice(&open_payload(snapshot.id.as_str(), &export.seal)?),
    });
    for (index, piece) in export.pieces.iter().enumerate() {
        records.push(NewEvent {
            id: event_id(&export.seal, index + 1)?,
            schema: piece_schema.clone(),
            payload: Payload::copy_from_slice(&piece_payload(piece)),
        });
    }
    records.push(NewEvent {
        id: event_id(&export.seal, export.pieces.len() + 1)?,
        schema: seal_schema,
        payload: Payload::copy_from_slice(&seal_payload(&export.seal)),
    });
    Ok(records)
}

async fn matching_prefix(
    reader: &dyn EventReader,
    stream: &StreamKey,
    expected: &[NewEvent],
) -> Result<usize, BaselineImportError> {
    let bounds = reader.bounds(stream).await?;
    if bounds.floor.offset != 0 || bounds.floor.stream != *stream || bounds.tail.stream != *stream {
        return Err(BaselineImportError::ConflictingStream);
    }
    let mut cursor = Cursor::new(stream.clone(), 0);
    let mut matched = 0;
    while cursor.offset < bounds.tail.offset {
        let page = reader.read_after(&cursor, PAGE, Some(&bounds.tail)).await?;
        if page.records.is_empty() {
            return Err(BaselineImportError::InvalidStream);
        }
        for record in page.records {
            let wanted = expected
                .get(matched)
                .ok_or(BaselineImportError::ConflictingStream)?;
            if record.cursor.stream != *stream
                || record.cursor.offset != cursor.offset + 1
                || record.event != *wanted
            {
                return Err(BaselineImportError::ConflictingStream);
            }
            cursor = record.cursor.clone();
            matched += 1;
        }
    }
    Ok(matched)
}

/// Append a validated candidate, resuming an identical prefix after a crash.
/// A lost append reply is resolved by reading the same prefix on retry; this
/// function returns an error until the matching seal is observed.
pub async fn commit_baseline_candidate<R: EventRuntime>(
    runtime: &R,
    stream: &StreamKey,
    export: &BaselineExport,
) -> Result<Cursor, BaselineImportError> {
    let expected = expected_records(export)?;
    let matched = matching_prefix(runtime, stream, &expected).await?;
    for event in expected.iter().skip(matched) {
        match runtime.append(stream, event.clone()).await {
            Ok(receipt)
                if receipt.record.cursor.stream == *stream && receipt.record.event == *event => {}
            Ok(_) => return Err(BaselineImportError::InvalidStream),
            Err(error) => {
                // CommitUnknown and a rejected append are both unconfirmed to
                // the caller. A repeat reads the prefix before appending again.
                return Err(BaselineImportError::Stream(error));
            }
        }
    }
    if matching_prefix(runtime, stream, &expected).await? == expected.len() {
        Ok(Cursor::new(
            stream.clone(),
            u64::try_from(expected.len()).map_err(|_| BaselineImportError::InvalidStream)?,
        ))
    } else {
        Err(BaselineImportError::InvalidStream)
    }
}

/// Read an imported baseline from the committed stream after process restart.
/// Later semantic records belong to the normal fold and are outside this read.
pub async fn load_baseline(
    reader: &dyn EventReader,
    stream: &StreamKey,
) -> Result<BaselineLoad, BaselineImportError> {
    let bounds = reader.bounds(stream).await?;
    if bounds.floor.offset != 0 || bounds.floor.stream != *stream || bounds.tail.stream != *stream {
        return Err(BaselineImportError::ConflictingStream);
    }
    let mut cursor = Cursor::new(stream.clone(), 0);
    let mut pieces = Vec::new();
    let mut records = Vec::new();
    while cursor.offset < bounds.tail.offset {
        let page = reader.read_after(&cursor, PAGE, Some(&bounds.tail)).await?;
        if page.records.is_empty() {
            return Err(BaselineImportError::InvalidStream);
        }
        for record in page.records {
            if record.cursor.stream != *stream || record.cursor.offset != cursor.offset + 1 {
                return Err(BaselineImportError::InvalidStream);
            }
            cursor = record.cursor.clone();
            let event = &record.event;
            if records.is_empty()
                && event.schema.id.as_str() == OPEN_SCHEMA
                && event.schema.version == 1
            {
                check_open(event.payload.as_bytes())?;
                records.push(event.clone());
            } else if !records.is_empty()
                && event.schema.id.as_str() == PIECE_SCHEMA
                && event.schema.version == 1
            {
                pieces.push(read_piece(event.payload.as_bytes())?);
                records.push(event.clone());
            } else if !pieces.is_empty()
                && event.schema.id.as_str() == SEAL_SCHEMA
                && event.schema.version == 1
            {
                let export = BaselineExport {
                    pieces,
                    seal: read_seal(event.payload.as_bytes())?,
                };
                let snapshot = decode_baseline(&export).map_err(BaselineImportError::Source)?;
                records.push(event.clone());
                if records != expected_records(&export)? {
                    return Err(BaselineImportError::ConflictingStream);
                }
                return Ok(BaselineLoad::Sealed {
                    snapshot,
                    cursor: cursor.clone(),
                });
            } else {
                return Err(BaselineImportError::ConflictingStream);
            }
        }
    }
    Ok(if records.is_empty() {
        BaselineLoad::Absent
    } else {
        BaselineLoad::Partial
    })
}
