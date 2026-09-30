use crate::product::generated::{RecordScope, MAX_RECORD_RESPONSE_BYTES};
use crate::protocol::{RequestFrame, MAX_PAYLOAD_BYTES};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_sync::replication::domain::{Id, Scope};
use serde::Serialize;
use std::io::{self, Write};

/// The product response could not be encoded within its published byte budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadEncodeError {
    ResponseTooLarge,
    InvalidPayload,
}

/// Invalid wire shape or a source page that failed the sync-engine contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadWireError {
    InvalidRequest,
    InvalidPage,
    ResponseTooLarge,
}

pub(crate) fn decimal_u64(value: &str) -> Result<u64, ReadWireError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ReadWireError::InvalidRequest);
    }
    value.parse().map_err(|_| ReadWireError::InvalidRequest)
}

pub(crate) fn decode_epoch(value: &str) -> Result<u64, ReadWireError> {
    let epoch = decimal_u64(value)?;
    (epoch > 0)
        .then_some(epoch)
        .ok_or(ReadWireError::InvalidRequest)
}

pub(crate) fn decode_scope(wire: &RecordScope) -> Result<Scope, ReadWireError> {
    let id = |value: &str| Id::new(value).map_err(|_| ReadWireError::InvalidRequest);
    Ok(Scope::new(
        id(&wire.receiver)?,
        id(&wire.origin)?,
        id(&wire.stream)?,
        id(&wire.incarnation)?,
        id(&wire.schema)?,
        id(&wire.access_epoch)?,
    ))
}

pub(crate) fn wire_scope(scope: &Scope) -> RecordScope {
    RecordScope {
        receiver: scope.receiver().as_str().to_owned(),
        origin: scope.origin().as_str().to_owned(),
        stream: scope.stream().as_str().to_owned(),
        incarnation: scope.incarnation().as_str().to_owned(),
        schema: scope.schema().as_str().to_owned(),
        access_epoch: scope.access_epoch().as_str().to_owned(),
    }
}

/// Decode standard canonical base64 into at most the admitted payload bytes.
pub(crate) fn decode_payload(value: &str, maximum: usize) -> Result<Vec<u8>, ReadWireError> {
    let encoded_max = maximum
        .checked_add(2)
        .and_then(|value| (value / 3).checked_mul(4))
        .ok_or(ReadWireError::ResponseTooLarge)?;
    if value.len() > encoded_max {
        return Err(ReadWireError::ResponseTooLarge);
    }
    let capacity = base64::decoded_len_estimate(value.len()).min(maximum);
    let mut payload = vec![0; capacity];
    let count = STANDARD
        .decode_slice(value, &mut payload)
        .map_err(|_| ReadWireError::InvalidPage)?;
    payload.truncate(count);
    Ok(payload.into_boxed_slice().into_vec())
}

#[derive(Serialize)]
struct ReadResponse<'a, T: Serialize> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    ok: bool,
    payload: &'a T,
}

struct CappedWriter {
    bytes: Vec<u8>,
    exceeded: bool,
    limit: usize,
}

impl CappedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit),
            exceeded: false,
            limit,
        }
    }
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let available = self.limit.saturating_sub(self.bytes.len());
        if bytes.len() > available {
            self.exceeded = true;
            return Err(io::Error::other(
                "passive read response exceeds product byte bound",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize the actual generated page result in the product response envelope.
/// The writer stops before allocating beyond `MAX_RECORD_RESPONSE_BYTES`.
pub(crate) fn encode_response<T: Serialize>(
    request_id: &str,
    result: &T,
) -> Result<String, ReadEncodeError> {
    let mut writer = CappedWriter::new(MAX_RECORD_RESPONSE_BYTES);
    let response = ReadResponse {
        kind: "res",
        id: request_id,
        ok: true,
        payload: result,
    };
    if serde_json::to_writer(&mut writer, &response).is_err() {
        return Err(if writer.exceeded {
            ReadEncodeError::ResponseTooLarge
        } else {
            ReadEncodeError::InvalidPayload
        });
    }
    String::from_utf8(writer.bytes.into_boxed_slice().into_vec())
        .map_err(|_| ReadEncodeError::InvalidPayload)
}

/// Serialize a generated parameter DTO using the existing request frame owner.
pub(crate) fn encode_request<T: Serialize>(
    id: &str,
    method: &str,
    params: &T,
) -> Result<String, ReadEncodeError> {
    let request =
        RequestFrame::new(id, method, params).map_err(|_| ReadEncodeError::InvalidPayload)?;
    let mut writer = CappedWriter::new(
        usize::try_from(MAX_PAYLOAD_BYTES).expect("published positive request bytes"),
    );
    if serde_json::to_writer(&mut writer, &request).is_err() {
        return Err(if writer.exceeded {
            ReadEncodeError::ResponseTooLarge
        } else {
            ReadEncodeError::InvalidPayload
        });
    }
    String::from_utf8(writer.bytes.into_boxed_slice().into_vec())
        .map_err(|_| ReadEncodeError::InvalidPayload)
}

#[cfg(test)]
#[path = "../../../tests/product/passive_read/wire.rs"]
mod tests;
