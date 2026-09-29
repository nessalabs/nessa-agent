//! Bounded physical records for one Nessa semantic fact.
//! This codec is pure: a caller still owns commit order, typed body validation,
//! store lifetime, and the decision to apply a complete fact.

use crate::{
    application::agent_execution::sessions::records::{FactKey, FactKind},
    domain::agent_execution::executions::ExecutionId,
};
use event_stream::{
    Cursor, EventId, EventReader, EventRuntime, NewEvent, PageLimits, Payload, SchemaId, SchemaRef,
    StreamKey,
};
use sha2::{Digest, Sha256};

const VERSION: u8 = 1;
const START_SCHEMA: &str = "nessa.fact-start";
const PIECE_SCHEMA: &str = "nessa.fact-piece";
const SEAL_SCHEMA: &str = "nessa.fact-seal";
const MAX_FRAME_BYTES: usize = 64 * 1024;
const MAX_PIECE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 160 * 1024 * 1024;
const START_TRAILER_BYTES: usize = 8 + 8 + 4 + 32 + 1;
const ABORT_SCHEMA: &str = "nessa.fact-abort";
const PAGE: PageLimits = PageLimits {
    max_records: 16,
    max_bytes: 1024 * 1024,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactFrameError {
    Invalid,
    TooLarge,
    Conflict,
}

#[derive(Debug)]
pub(crate) enum FactCommitError {
    Frame(FactFrameError),
    Stream(event_stream::Error),
    Conflict,
    InvalidStream,
}

impl From<event_stream::Error> for FactCommitError {
    fn from(value: event_stream::Error) -> Self {
        Self::Stream(value)
    }
}

impl std::fmt::Display for FactCommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(error) => write!(formatter, "invalid fact frame: {error:?}"),
            Self::Stream(error) => write!(formatter, "record stream: {error}"),
            Self::Conflict => formatter.write_str("fact conflicts with committed record"),
            Self::InvalidStream => formatter.write_str("record stream returned invalid evidence"),
        }
    }
}

impl std::error::Error for FactCommitError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FramedFact {
    pub(crate) key: FactKey,
    pub(crate) body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactDecode {
    Partial,
    Complete { fact: FramedFact, records: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactRead {
    Absent,
    Partial,
    Aborted { cursor: Cursor },
    Complete { fact: FramedFact, cursor: Cursor },
}

fn key_bytes(key: &FactKey) -> Vec<u8> {
    let id = key
        .execution_id()
        .map_or("", ExecutionId::as_str)
        .as_bytes();
    let mut bytes = Vec::with_capacity(11 + id.len());
    bytes.push(key.kind().code());
    bytes.extend_from_slice(&(id.len() as u16).to_be_bytes());
    bytes.extend_from_slice(id);
    bytes.extend_from_slice(&key.ordinal().to_be_bytes());
    bytes
}

fn event_id(key: &FactKey, attempt_start: u64, suffix: &str) -> EventId {
    let mut identity = key_bytes(key);
    identity.extend_from_slice(&attempt_start.to_be_bytes());
    let digest = Sha256::digest(identity);
    let mut value = String::from("nessa-fact-");
    for byte in digest {
        use std::fmt::Write;
        write!(value, "{byte:02x}").expect("formatting into a String cannot fail");
    }
    value.push('-');
    value.push_str(suffix);
    EventId::new(value).expect("fixed-size digest and suffix fit event ID")
}

fn schema(id: &str) -> SchemaRef {
    SchemaRef {
        id: SchemaId::new(id).expect("fixed schema ID is valid"),
        version: 1,
    }
}

/// The wire tag for a validated physical frame schema. The bounded source
/// shares this mapping with the writer's schema IDs.
pub(crate) fn frame_tag(event: &NewEvent) -> Result<u8, FactFrameError> {
    match (event.schema.id.as_str(), event.schema.version) {
        (START_SCHEMA, 1) => Ok(1),
        (PIECE_SCHEMA, 1) => Ok(2),
        (SEAL_SCHEMA, 1) => Ok(3),
        (ABORT_SCHEMA, 1) => Ok(4),
        _ => Err(FactFrameError::Conflict),
    }
}

#[cfg(test)]
pub(crate) fn schema_for_tag(tag: u8) -> Result<SchemaRef, FactFrameError> {
    match tag {
        1 => Ok(schema(START_SCHEMA)),
        2 => Ok(schema(PIECE_SCHEMA)),
        3 => Ok(schema(SEAL_SCHEMA)),
        4 => Ok(schema(ABORT_SCHEMA)),
        _ => Err(FactFrameError::Invalid),
    }
}

#[cfg(test)]
pub(crate) fn validate_start_offset(start: &NewEvent, position: u64) -> Result<(), FactFrameError> {
    if parse_start(start)?.attempt_start != position {
        return Err(FactFrameError::Invalid);
    }
    Ok(())
}

fn event(
    key: &FactKey,
    attempt_start: u64,
    suffix: &str,
    schema_id: &str,
    payload: &[u8],
) -> NewEvent {
    NewEvent {
        id: event_id(key, attempt_start, suffix),
        schema: schema(schema_id),
        payload: Payload::copy_from_slice(payload),
    }
}

pub(crate) fn frame_fact(
    fact: &FramedFact,
    attempt_start: u64,
) -> Result<Vec<NewEvent>, FactFrameError> {
    if fact.body.is_empty() || fact.body.len() > MAX_BODY_BYTES {
        return Err(FactFrameError::TooLarge);
    }
    let key = key_bytes(&fact.key);
    let digest = Sha256::digest(&fact.body);
    let inline = 1 + key.len() + START_TRAILER_BYTES + fact.body.len() <= MAX_FRAME_BYTES;
    let count = if inline {
        0
    } else {
        fact.body.len().div_ceil(MAX_PIECE_BYTES)
    };
    let count = u32::try_from(count).map_err(|_| FactFrameError::TooLarge)?;
    let mut start = Vec::with_capacity((1 + key.len() + START_TRAILER_BYTES).min(MAX_FRAME_BYTES));
    start.push(VERSION);
    start.extend_from_slice(&key);
    start.extend_from_slice(&attempt_start.to_be_bytes());
    start.extend_from_slice(&(fact.body.len() as u64).to_be_bytes());
    start.extend_from_slice(&count.to_be_bytes());
    start.extend_from_slice(&digest);
    start.push(u8::from(!inline));
    if inline {
        start.extend_from_slice(&fact.body);
    }
    let mut records = Vec::with_capacity(count as usize + if inline { 1 } else { 2 });
    records.push(event(
        &fact.key,
        attempt_start,
        "start",
        START_SCHEMA,
        &start,
    ));
    if !inline {
        for (index, bytes) in fact.body.chunks(MAX_PIECE_BYTES).enumerate() {
            let mut payload = Vec::with_capacity(9 + bytes.len());
            payload.push(VERSION);
            payload.extend_from_slice(&(index as u32).to_be_bytes());
            payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            payload.extend_from_slice(bytes);
            records.push(event(
                &fact.key,
                attempt_start,
                &format!("part-{index}"),
                PIECE_SCHEMA,
                &payload,
            ));
        }
        let mut seal = Vec::with_capacity(45);
        seal.push(VERSION);
        seal.extend_from_slice(&count.to_be_bytes());
        seal.extend_from_slice(&(fact.body.len() as u64).to_be_bytes());
        seal.extend_from_slice(&digest);
        records.push(event(&fact.key, attempt_start, "seal", SEAL_SCHEMA, &seal));
    }
    Ok(records)
}

fn take<'a>(bytes: &mut &'a [u8], count: usize) -> Result<&'a [u8], FactFrameError> {
    if bytes.len() < count {
        return Err(FactFrameError::Invalid);
    }
    let (head, rest) = bytes.split_at(count);
    *bytes = rest;
    Ok(head)
}

fn read_u8(bytes: &mut &[u8]) -> Result<u8, FactFrameError> {
    Ok(take(bytes, 1)?[0])
}

fn read_u16(bytes: &mut &[u8]) -> Result<u16, FactFrameError> {
    Ok(u16::from_be_bytes(take(bytes, 2)?.try_into().unwrap()))
}

fn read_u32(bytes: &mut &[u8]) -> Result<u32, FactFrameError> {
    Ok(u32::from_be_bytes(take(bytes, 4)?.try_into().unwrap()))
}

fn read_u64(bytes: &mut &[u8]) -> Result<u64, FactFrameError> {
    Ok(u64::from_be_bytes(take(bytes, 8)?.try_into().unwrap()))
}

fn check_event(
    actual: &NewEvent,
    key: &FactKey,
    attempt_start: u64,
    suffix: &str,
    schema_id: &str,
) -> Result<(), FactFrameError> {
    if actual.id != event_id(key, attempt_start, suffix) || actual.schema != schema(schema_id) {
        return Err(FactFrameError::Conflict);
    }
    Ok(())
}

struct ParsedStart<'a> {
    key: FactKey,
    attempt_start: u64,
    body_length: usize,
    count: u32,
    digest: [u8; 32],
    inline: Option<&'a [u8]>,
}

fn parse_start(start: &NewEvent) -> Result<ParsedStart<'_>, FactFrameError> {
    if start.schema != schema(START_SCHEMA) {
        return Err(FactFrameError::Conflict);
    }
    let mut bytes = start.payload.as_bytes();
    if read_u8(&mut bytes)? != VERSION {
        return Err(FactFrameError::Invalid);
    }
    let kind = FactKind::from_code(read_u8(&mut bytes)?).ok_or(FactFrameError::Invalid)?;
    let id_length = usize::from(read_u16(&mut bytes)?);
    let id = take(&mut bytes, id_length)?;
    let id = std::str::from_utf8(id).map_err(|_| FactFrameError::Invalid)?;
    let execution_id = if id.is_empty() {
        None
    } else {
        Some(ExecutionId::new(id).map_err(|_| FactFrameError::Invalid)?)
    };
    let ordinal = read_u64(&mut bytes)?;
    let key = FactKey::new(kind, execution_id, ordinal).ok_or(FactFrameError::Invalid)?;
    let attempt_start = read_u64(&mut bytes)?;
    check_event(start, &key, attempt_start, "start", START_SCHEMA)?;
    let body_length =
        usize::try_from(read_u64(&mut bytes)?).map_err(|_| FactFrameError::TooLarge)?;
    if body_length == 0 || body_length > MAX_BODY_BYTES {
        return Err(FactFrameError::TooLarge);
    }
    let count = read_u32(&mut bytes)?;
    let digest: [u8; 32] = take(&mut bytes, 32)?.try_into().unwrap();
    let mode = read_u8(&mut bytes)?;
    let inline = match mode {
        0 if count == 0 && start.payload.len() <= MAX_FRAME_BYTES && bytes.len() == body_length => {
            if Sha256::digest(bytes).as_slice() != digest {
                return Err(FactFrameError::Invalid);
            }
            Some(bytes)
        }
        1 if bytes.is_empty()
            && count != 0
            && usize::try_from(count).map_err(|_| FactFrameError::TooLarge)?
                == body_length.div_ceil(MAX_PIECE_BYTES)
            && 1 + key_bytes(&key).len() + START_TRAILER_BYTES + body_length > MAX_FRAME_BYTES =>
        {
            None
        }
        _ => return Err(FactFrameError::Invalid),
    };
    Ok(ParsedStart {
        key,
        attempt_start,
        body_length,
        count,
        digest,
        inline,
    })
}

pub(crate) fn decode_first(records: &[NewEvent]) -> Result<FactDecode, FactFrameError> {
    let Some(start) = records.first() else {
        return Ok(FactDecode::Partial);
    };
    let ParsedStart {
        key,
        attempt_start,
        body_length,
        count,
        digest,
        inline,
    } = parse_start(start)?;
    if let Some(bytes) = inline {
        return Ok(FactDecode::Complete {
            fact: FramedFact {
                key,
                body: bytes.to_vec(),
            },
            records: 1,
        });
    }
    let needed = count as usize + 2;
    let mut body = Vec::with_capacity(
        body_length.min(
            records
                .len()
                .saturating_sub(1)
                .saturating_mul(MAX_PIECE_BYTES),
        ),
    );
    for index in 0..count as usize {
        let Some(record) = records.get(index + 1) else {
            return Ok(FactDecode::Partial);
        };
        check_event(
            record,
            &key,
            attempt_start,
            &format!("part-{index}"),
            PIECE_SCHEMA,
        )?;
        let mut piece = record.payload.as_bytes();
        if read_u8(&mut piece)? != VERSION || read_u32(&mut piece)? != index as u32 {
            return Err(FactFrameError::Invalid);
        }
        let length = read_u32(&mut piece)? as usize;
        let expected = (body_length - body.len()).min(MAX_PIECE_BYTES);
        if length != expected || piece.len() != length {
            return Err(FactFrameError::Invalid);
        }
        body.extend_from_slice(piece);
    }
    let Some(seal) = records.get(needed - 1) else {
        if Sha256::digest(&body).as_slice() != digest {
            return Err(FactFrameError::Invalid);
        }
        return Ok(FactDecode::Partial);
    };
    check_event(seal, &key, attempt_start, "seal", SEAL_SCHEMA)?;
    let mut seal_bytes = seal.payload.as_bytes();
    if read_u8(&mut seal_bytes)? != VERSION
        || read_u32(&mut seal_bytes)? != count
        || read_u64(&mut seal_bytes)? != body_length as u64
        || take(&mut seal_bytes, 32)? != digest
        || !seal_bytes.is_empty()
        || Sha256::digest(&body).as_slice() != digest
    {
        return Err(FactFrameError::Invalid);
    }
    Ok(FactDecode::Complete {
        fact: FramedFact { key, body },
        records: needed,
    })
}

fn abort_event(start: &NewEvent, prefix_end: u64) -> Result<NewEvent, FactFrameError> {
    let parsed = parse_start(start)?;
    if parsed.inline.is_some() {
        return Err(FactFrameError::Invalid);
    }
    let mut payload = Vec::with_capacity(49);
    payload.push(VERSION);
    payload.extend_from_slice(&parsed.attempt_start.to_be_bytes());
    payload.extend_from_slice(&prefix_end.to_be_bytes());
    payload.extend_from_slice(&Sha256::digest(start.payload.as_bytes()));
    Ok(event(
        &parsed.key,
        parsed.attempt_start,
        &format!("abort-{prefix_end}"),
        ABORT_SCHEMA,
        &payload,
    ))
}

pub(crate) fn validate_abort_prefix(
    prefix: &[NewEvent],
    abort: &NewEvent,
    start_offset: u64,
    prefix_end: u64,
) -> Result<(), FactFrameError> {
    let Some(start) = prefix.first() else {
        return Err(FactFrameError::Invalid);
    };
    let parsed = parse_start(start)?;
    if parsed.attempt_start != start_offset || parsed.inline.is_some() {
        return Err(FactFrameError::Invalid);
    }
    let span = prefix_end
        .checked_sub(start_offset)
        .and_then(|distance| distance.checked_add(1))
        .ok_or(FactFrameError::Invalid)?;
    if !matches!(decode_first(prefix)?, FactDecode::Partial)
        || prefix.len() as u64 != span
        || *abort != abort_event(start, prefix_end)?
    {
        return Err(FactFrameError::Conflict);
    }
    Ok(())
}

/// Read one complete logical fact after `after`, or report an unfinished tail.
/// This stops at the first fact seal even when the page also contains later facts.
pub(crate) async fn read_next_fact(
    reader: &dyn EventReader,
    stream: &StreamKey,
    after: &Cursor,
) -> Result<FactRead, FactCommitError> {
    let bounds = reader.bounds(stream).await?;
    if after.stream != *stream
        || bounds.floor.stream != *stream
        || bounds.tail.stream != *stream
        || after.offset < bounds.floor.offset
        || after.offset > bounds.tail.offset
    {
        return Err(FactCommitError::InvalidStream);
    }
    if after.offset == bounds.tail.offset {
        return Ok(FactRead::Absent);
    }
    let mut cursor = after.clone();
    let mut expected_count = None;
    let mut events = Vec::new();
    while cursor.offset < bounds.tail.offset {
        let page = reader.read_after(&cursor, PAGE, Some(&bounds.tail)).await?;
        if page.records.is_empty() {
            return Err(FactCommitError::InvalidStream);
        }
        for record in page.records {
            if record.cursor.stream != *stream || record.cursor.offset != cursor.offset + 1 {
                return Err(FactCommitError::InvalidStream);
            }
            if expected_count.is_none() {
                let start = parse_start(&record.event).map_err(FactCommitError::Frame)?;
                if start.attempt_start != record.cursor.offset {
                    return Err(FactCommitError::InvalidStream);
                }
                expected_count = Some(start.inline.map_or(start.count as usize + 2, |_| 1));
            }
            if record.event.schema == schema(ABORT_SCHEMA) {
                validate_abort_prefix(&events, &record.event, after.offset + 1, cursor.offset)
                    .map_err(FactCommitError::Frame)?;
                return Ok(FactRead::Aborted {
                    cursor: record.cursor.clone(),
                });
            }
            events.push(record.event.clone());
            cursor = record.cursor.clone();
            if events.len() == expected_count.expect("parsed start") {
                let FactDecode::Complete { fact, records } =
                    decode_first(&events).map_err(FactCommitError::Frame)?
                else {
                    return Err(FactCommitError::InvalidStream);
                };
                if records != events.len() {
                    return Err(FactCommitError::InvalidStream);
                }
                return Ok(FactRead::Complete { fact, cursor });
            }
        }
    }
    if !matches!(
        decode_first(&events).map_err(FactCommitError::Frame)?,
        FactDecode::Partial
    ) {
        return Err(FactCommitError::InvalidStream);
    }
    Ok(FactRead::Partial)
}

/// End a fully validated incomplete physical attempt after exclusive reopen.
/// A retry after a lost abort acknowledgement observes the exact terminal marker.
pub(crate) async fn abort_partial_fact<R: EventRuntime>(
    runtime: &R,
    stream: &StreamKey,
    after: &Cursor,
) -> Result<Cursor, FactCommitError> {
    let bounds = runtime.bounds(stream).await?;
    if bounds.floor.offset > after.offset || bounds.tail.offset <= after.offset {
        return Err(FactCommitError::InvalidStream);
    }
    let mut cursor = after.clone();
    let mut prefix = Vec::new();
    while cursor.offset < bounds.tail.offset {
        let page = runtime
            .read_after(&cursor, PAGE, Some(&bounds.tail))
            .await?;
        if page.records.is_empty() {
            return Err(FactCommitError::InvalidStream);
        }
        for record in page.records {
            if record.cursor.offset != cursor.offset + 1 || record.cursor.stream != *stream {
                return Err(FactCommitError::InvalidStream);
            }
            cursor = record.cursor.clone();
            prefix.push(record.event.clone());
        }
    }
    let start = prefix.first().ok_or(FactCommitError::InvalidStream)?;
    let marker = abort_event(start, cursor.offset).map_err(FactCommitError::Frame)?;
    validate_abort_prefix(&prefix, &marker, after.offset + 1, cursor.offset)
        .map_err(FactCommitError::Frame)?;
    let receipt = runtime.append(stream, marker.clone()).await?;
    if receipt.record.event != marker
        || receipt.record.cursor.stream != *stream
        || receipt.record.cursor.offset != cursor.offset + 1
    {
        return Err(FactCommitError::InvalidStream);
    }
    match read_next_fact(runtime, stream, after).await? {
        FactRead::Aborted { cursor: verified } if verified == receipt.record.cursor => Ok(verified),
        _ => Err(FactCommitError::InvalidStream),
    }
}

async fn matching_suffix(
    reader: &dyn EventReader,
    stream: &StreamKey,
    after: &Cursor,
    expected: &[NewEvent],
) -> Result<(usize, Cursor), FactCommitError> {
    let bounds = reader.bounds(stream).await?;
    if after.stream != *stream
        || bounds.floor.stream != *stream
        || bounds.tail.stream != *stream
        || after.offset < bounds.floor.offset
        || after.offset > bounds.tail.offset
    {
        return Err(FactCommitError::InvalidStream);
    }
    let mut cursor = after.clone();
    let mut matched = 0;
    while cursor.offset < bounds.tail.offset {
        let page = reader.read_after(&cursor, PAGE, Some(&bounds.tail)).await?;
        if page.records.is_empty() {
            return Err(FactCommitError::InvalidStream);
        }
        for record in page.records {
            let wanted = expected.get(matched).ok_or(FactCommitError::Conflict)?;
            if record.cursor.stream != *stream
                || record.cursor.offset != cursor.offset + 1
                || record.event != *wanted
            {
                return Err(FactCommitError::Conflict);
            }
            cursor = record.cursor.clone();
            matched += 1;
            if matched == expected.len() {
                return Ok((matched, cursor));
            }
        }
    }
    Ok((matched, cursor))
}

/// Confirm one logical fact after its previous committed cursor. An uncertain
/// append is retried by reading the same physical IDs and exact bytes first.
/// A complete fact can be recovered even when later records follow it.
pub(crate) async fn commit_fact<R: EventRuntime>(
    runtime: &R,
    stream: &StreamKey,
    after: &Cursor,
    fact: &FramedFact,
) -> Result<Cursor, FactCommitError> {
    let expected = frame_fact(fact, after.offset + 1).map_err(FactCommitError::Frame)?;
    let (matched, mut cursor) = matching_suffix(runtime, stream, after, &expected).await?;
    if matched == expected.len() {
        return Ok(cursor);
    }
    for event in expected.iter().skip(matched) {
        let receipt = runtime.append(stream, event.clone()).await?;
        if receipt.record.event != *event
            || receipt.record.cursor.stream != *stream
            || receipt.record.cursor.offset != cursor.offset + 1
        {
            return Err(FactCommitError::InvalidStream);
        }
        cursor = receipt.record.cursor.clone();
    }
    let (verified, cursor) = matching_suffix(runtime, stream, after, &expected).await?;
    if verified != expected.len() {
        return Err(FactCommitError::InvalidStream);
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_stream::{
        infrastructure::{SqliteFailureInjection, SqliteOptions, SqliteStore},
        EventConfig, EventSink, PersistenceProfile, Runtime, RuntimeConfig, StreamId,
    };
    use std::time::Duration;

    fn key() -> FactKey {
        FactKey::new(
            FactKind::InputAccepted,
            Some(ExecutionId::new("execution").unwrap()),
            0,
        )
        .unwrap()
    }

    #[test]
    fn an_inline_fact_has_one_stable_record() {
        let fact = FramedFact {
            key: key(),
            body: br#"{"input":"hello"}"#.to_vec(),
        };
        let records = frame_fact(&fact, 1).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            decode_first(&records).unwrap(),
            FactDecode::Complete { fact, records: 1 }
        );
    }

    #[test]
    fn a_large_fact_is_invisible_until_its_seal_and_detects_changed_bytes() {
        let fact = FramedFact {
            key: key(),
            body: vec![b'x'; 4 * 1024 * 1024],
        };
        let records = frame_fact(&fact, 1).unwrap();
        assert_eq!(records.len(), 66);
        assert!(matches!(
            decode_first(&records[..65]),
            Ok(FactDecode::Partial)
        ));
        assert_eq!(
            decode_first(&records).unwrap(),
            FactDecode::Complete {
                fact: fact.clone(),
                records: 66,
            }
        );
        let mut changed = fact;
        changed.body[0] ^= 1;
        let changed_records = frame_fact(&changed, 1).unwrap();
        assert_eq!(records[0].id, changed_records[0].id);
        assert_ne!(records[0].payload, changed_records[0].payload);
        let mut corrupt = records;
        corrupt[1].payload = changed_records[1].payload.clone();
        assert_eq!(decode_first(&corrupt[..65]), Err(FactFrameError::Invalid));
        assert_eq!(decode_first(&corrupt), Err(FactFrameError::Invalid));
    }

    #[test]
    fn abort_accepts_only_an_exact_incomplete_prefix() {
        let fact = FramedFact {
            key: key(),
            body: vec![b'x'; 200_000],
        };
        let frames = frame_fact(&fact, 7).unwrap();
        let marker = abort_event(&frames[0], 8).unwrap();
        assert!(validate_abort_prefix(&frames[..2], &marker, 7, 8).is_ok());
        let mut changed_piece = frames[..2].to_vec();
        changed_piece[1].payload = Payload::copy_from_slice(b"foreign");
        assert!(validate_abort_prefix(&changed_piece, &marker, 7, 8).is_err());
        let mut changed_marker = marker.clone();
        changed_marker.payload = Payload::copy_from_slice(b"foreign");
        assert!(validate_abort_prefix(&frames[..2], &changed_marker, 7, 8).is_err());
        assert!(validate_abort_prefix(&frames, &marker, 7, frames.len() as u64 + 6).is_err());
    }

    #[tokio::test]
    async fn an_abort_is_a_physical_terminal_and_the_same_logical_key_can_start_again() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("abort.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let start = Cursor::new(stream.clone(), 0);
        let abandoned = FramedFact {
            key: key(),
            body: vec![b'x'; 200_000],
        };
        let frames = frame_fact(&abandoned, 1).unwrap();
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        assert!(matches!(
            read_next_fact(&runtime, &stream, &start).await.unwrap(),
            FactRead::Partial
        ));
        let aborted = abort_partial_fact(&runtime, &stream, &start).await.unwrap();
        assert_eq!(aborted.offset, 3);
        assert!(matches!(
            read_next_fact(&runtime, &stream, &start).await.unwrap(),
            FactRead::Aborted { cursor } if cursor == aborted
        ));
        let replacement = FramedFact {
            key: key(),
            body: vec![b'y'; 200_000],
        };
        let replacement_frames = frame_fact(&replacement, aborted.offset + 1).unwrap();
        assert_ne!(frames[0].id, replacement_frames[0].id);
        let committed = commit_fact(&runtime, &stream, &aborted, &replacement)
            .await
            .unwrap();
        assert!(matches!(
            read_next_fact(&runtime, &stream, &aborted).await.unwrap(),
            FactRead::Complete { fact, cursor } if fact == replacement && cursor == committed
        ));
        runtime.append(&stream, frames[2].clone()).await.unwrap();
        assert!(matches!(
            read_next_fact(&runtime, &stream, &committed).await,
            Err(FactCommitError::Frame(_))
        ));
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn all_pieces_with_changed_bytes_and_no_seal_refuse_abort() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("corrupt-tail.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let start = Cursor::new(stream.clone(), 0);
        let fact = FramedFact {
            key: key(),
            body: vec![b'x'; 200_000],
        };
        let frames = frame_fact(&fact, 1).unwrap();
        let mut changed = fact.clone();
        changed.body[0] = b'y';
        let changed_frames = frame_fact(&changed, 1).unwrap();
        for (index, frame) in frames.iter().enumerate().take(frames.len() - 1) {
            let mut frame = frame.clone();
            if index == 1 {
                frame.payload = changed_frames[1].payload.clone();
            }
            runtime.append(&stream, frame).await.unwrap();
        }
        assert!(matches!(
            read_next_fact(&runtime, &stream, &start).await,
            Err(FactCommitError::Frame(FactFrameError::Invalid))
        ));
        assert!(matches!(
            abort_partial_fact(&runtime, &stream, &start).await,
            Err(FactCommitError::Frame(FactFrameError::Invalid))
        ));
        assert_eq!(
            runtime.bounds(&stream).await.unwrap().tail.offset,
            (frames.len() - 1) as u64
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn sqlite_restart_resumes_a_partial_fact_and_recovers_its_original_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("facts.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let stream_id = StreamId::new("conversation").unwrap();
        let runtime = Runtime::<SqliteStore>::open(options.clone(), config.clone())
            .await
            .unwrap();
        let stream = runtime.create_stream(&stream_id).await.unwrap();
        let start = Cursor::new(stream.clone(), 0);
        let fact = FramedFact {
            key: key(),
            body: serde_json::to_vec(&"x".repeat(4 * 1024 * 1024)).unwrap(),
        };
        let records = frame_fact(&fact, 1).unwrap();
        assert!(records.len() > 2);
        runtime.append(&stream, records[0].clone()).await.unwrap();
        runtime.append(&stream, records[1].clone()).await.unwrap();
        let mut changed = fact.clone();
        changed.body[1] ^= 1;
        assert!(matches!(
            commit_fact(&runtime, &stream, &start, &changed).await,
            Err(FactCommitError::Conflict)
        ));
        let report = runtime.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(report.closed);

        let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let same_stream = reopened.create_stream(&stream_id).await.unwrap();
        assert_eq!(same_stream, stream);
        let committed = commit_fact(&reopened, &stream, &start, &fact)
            .await
            .unwrap();
        assert_eq!(committed.offset, records.len() as u64);
        assert_eq!(
            commit_fact(&reopened, &stream, &start, &fact)
                .await
                .unwrap(),
            committed
        );
        let report = reopened.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(report.closed);
    }

    #[tokio::test]
    async fn sqlite_store_reconciles_lost_append_reply_without_a_second_record() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = SqliteOptions::new(directory.path().join("lost-reply.sqlite3"));
        options.failure_injection = Some(SqliteFailureInjection::AfterCommitAcknowledgementLost);
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let start = Cursor::new(stream.clone(), 0);
        let fact = FramedFact {
            key: key(),
            body: br#"{"input":"hello"}"#.to_vec(),
        };
        let first = commit_fact(&runtime, &stream, &start, &fact).await.unwrap();
        let receipt = commit_fact(&runtime, &stream, &start, &fact).await.unwrap();
        assert_eq!(first, receipt);
        assert_eq!(receipt.offset, 1);
        assert_eq!(runtime.bounds(&stream).await.unwrap().tail.offset, 1);
        let report = runtime.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(report.closed);
    }

    #[tokio::test]
    async fn read_stops_at_each_seal_and_keeps_an_unsealed_tail_invisible() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("read.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let stream = runtime
            .create_stream(&StreamId::new("conversation").unwrap())
            .await
            .unwrap();
        let start = Cursor::new(stream.clone(), 0);
        let first = FramedFact {
            key: key(),
            body: br#"{"input":"one"}"#.to_vec(),
        };
        let second = FramedFact {
            key: FactKey::new(
                FactKind::ProviderObservation,
                Some(ExecutionId::new("execution").unwrap()),
                0,
            )
            .unwrap(),
            body: vec![b'x'; 2 * 1024 * 1024],
        };
        let first_end = commit_fact(&runtime, &stream, &start, &first)
            .await
            .unwrap();
        let second_records = frame_fact(&second, first_end.offset + 1).unwrap();
        for event in second_records.iter().take(second_records.len() - 1) {
            runtime.append(&stream, event.clone()).await.unwrap();
        }
        assert_eq!(
            read_next_fact(&runtime, &stream, &start).await.unwrap(),
            FactRead::Complete {
                fact: first,
                cursor: first_end.clone(),
            }
        );
        assert_eq!(
            read_next_fact(&runtime, &stream, &first_end).await.unwrap(),
            FactRead::Partial
        );
        runtime
            .append(&stream, second_records.last().unwrap().clone())
            .await
            .unwrap();
        let FactRead::Complete { fact, cursor } =
            read_next_fact(&runtime, &stream, &first_end).await.unwrap()
        else {
            panic!("sealed fact was not visible");
        };
        assert_eq!(fact, second);
        assert_eq!(
            read_next_fact(&runtime, &stream, &cursor).await.unwrap(),
            FactRead::Absent
        );
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }
}
