use crate::product::generated::{
    ConversationRecordsHeadResult, ConversationRecordsPageResult, RecordPageRequest,
    RecordWireRecord, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES,
    MAX_RECORD_PAGE_RECORDS,
};
use crate::product::passive_read::wire::{
    decimal_u64, decode_payload, decode_scope, encode_response, wire_scope, ReadEncodeError,
    ReadWireError,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_sync::replication::domain::{
    validate_page, validate_page_request, Checkpoint, Id, Limits, Page, PageRequest, Record, Scope,
};

pub(crate) fn decode_page_request(wire: &RecordPageRequest) -> Result<PageRequest, ReadWireError> {
    let max_records =
        usize::try_from(wire.max_records).map_err(|_| ReadWireError::InvalidRequest)?;
    let max_payload_bytes =
        usize::try_from(wire.max_payload_bytes).map_err(|_| ReadWireError::InvalidRequest)?;
    let max_record_bytes =
        usize::try_from(wire.max_record_bytes).map_err(|_| ReadWireError::InvalidRequest)?;
    let after = decimal_u64(&wire.after)?;
    let target = decimal_u64(&wire.target)?;
    let request = PageRequest {
        scope: decode_scope(&wire.scope)?,
        after,
        target,
        max_records,
        max_payload_bytes,
        max_record_bytes,
    };
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .expect("published nonzero product bounds");
    validate_page_request(&request, limits).map_err(|_| ReadWireError::InvalidRequest)?;
    Ok(request)
}

pub(crate) fn encode_head(
    request_id: &str,
    scope: &Scope,
    head: u64,
) -> Result<String, ReadEncodeError> {
    encode_response(
        request_id,
        &ConversationRecordsHeadResult {
            scope: wire_scope(scope),
            head: head.to_string(),
        },
    )
}

pub(crate) fn wire_request(request: &PageRequest) -> RecordPageRequest {
    RecordPageRequest {
        scope: wire_scope(&request.scope),
        after: request.after.to_string(),
        target: request.target.to_string(),
        max_records: request.max_records as u64,
        max_payload_bytes: request.max_payload_bytes as u64,
        max_record_bytes: request.max_record_bytes as u64,
    }
}

/// Validate a source page through sync-engine before flattening its shared
/// scope into the product envelope, then encode under the response ceiling.
pub(crate) fn encode_page(
    request_id: &str,
    request: &PageRequest,
    page: Page,
) -> Result<String, ReadWireError> {
    let limits = Limits::new(
        MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .expect("published nonzero page bounds");
    let plan = validate_page(
        &Checkpoint::new(request.scope.clone(), request.after),
        request,
        page,
        limits,
    )
    .map_err(|_| ReadWireError::InvalidPage)?;
    let (_, _, records) = plan.into_parts();
    let result = ConversationRecordsPageResult {
        request: wire_request(request),
        records: records
            .into_iter()
            .map(|record| RecordWireRecord {
                position: record.position.to_string(),
                id: record.id.as_str().to_owned(),
                payload: STANDARD.encode(record.payload),
            })
            .collect(),
    };
    encode_response(request_id, &result).map_err(|error| match error {
        ReadEncodeError::ResponseTooLarge => ReadWireError::ResponseTooLarge,
        ReadEncodeError::InvalidPayload => ReadWireError::InvalidPage,
    })
}

/// Convert a response while preserving every echoed request field for core validation.
pub(crate) fn decode_head(
    wire: ConversationRecordsHeadResult,
) -> Result<(Scope, u64), ReadWireError> {
    Ok((decode_scope(&wire.scope)?, decimal_u64(&wire.head)?))
}
pub(crate) fn decode_page_result(
    wire: ConversationRecordsPageResult,
    expected: &PageRequest,
) -> Result<Page, ReadWireError> {
    if wire.records.len() > expected.max_records {
        return Err(ReadWireError::ResponseTooLarge);
    }
    let request = decode_page_request(&wire.request)?;
    let mut remaining = expected.max_payload_bytes;
    let mut records = Vec::with_capacity(wire.records.len());
    for record in wire.records {
        let payload = decode_payload(&record.payload, expected.max_record_bytes.min(remaining))?;
        remaining -= payload.len();
        records.push(Record {
            scope: request.scope.clone(),
            position: decimal_u64(&record.position)?,
            id: Id::new(&record.id).map_err(|_| ReadWireError::InvalidPage)?,
            payload,
        });
    }
    Ok(Page { request, records })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::generated::{
        ConversationRecordsPageResult, RecordPageRequest, RecordScope, RecordWireRecord,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES, MAX_RECORD_PAGE_RECORDS,
        MAX_RECORD_RESPONSE_BYTES,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use nessa_sync::replication::domain::{Id, Record, MAX_ID_BYTES};

    #[test]
    fn aggregate_request_published_boundary_is_admitted_and_successor_refused() {
        assert_eq!(MAX_RECORD_PAGE_PAYLOAD_BYTES, 65546);
        let mut request = RecordPageRequest {
            scope: scope(),
            after: "0".into(),
            target: "1".into(),
            max_records: MAX_RECORD_PAGE_RECORDS as u64,
            max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES as u64,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
        };
        assert!(decode_page_request(&request).is_ok());
        request.max_payload_bytes += 1;
        assert!(decode_page_request(&request).is_err());
    }
    fn scope() -> RecordScope {
        let escaped = "\u{0001}".repeat(MAX_ID_BYTES);
        RecordScope {
            receiver: escaped.clone(),
            origin: escaped.clone(),
            stream: escaped.clone(),
            incarnation: escaped.clone(),
            schema: escaped.clone(),
            access_epoch: escaped,
        }
    }

    fn result(payload_lengths: &[usize]) -> ConversationRecordsPageResult {
        assert!(payload_lengths.len() <= MAX_RECORD_PAGE_RECORDS);
        let records = payload_lengths
            .iter()
            .map(|length| RecordWireRecord {
                position: u64::MAX.to_string(),
                id: "\u{0001}".repeat(MAX_ID_BYTES),
                payload: STANDARD.encode(vec![0xff; *length]),
            })
            .collect();
        ConversationRecordsPageResult {
            request: RecordPageRequest {
                scope: scope(),
                after: u64::MAX.to_string(),
                target: u64::MAX.to_string(),
                max_records: MAX_RECORD_PAGE_RECORDS as u64,
                max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
                max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
            },
            records,
        }
    }

    #[test]
    fn maximum_physical_record_and_mixed_page_fit_real_json_envelope() {
        for (lengths, measured_bytes) in [
            vec![MAX_PHYSICAL_RECORD_PAYLOAD_BYTES],
            std::iter::once(MAX_PHYSICAL_RECORD_PAYLOAD_BYTES - (MAX_RECORD_PAGE_RECORDS - 1))
                .chain(std::iter::repeat_n(1, MAX_RECORD_PAGE_RECORDS - 1))
                .collect(),
        ]
        .into_iter()
        .zip([94_653, 107_068])
        {
            let encoded = encode_response(&"\u{0001}".repeat(256), &result(&lengths))
                .expect("maximal response fits");
            assert_eq!(encoded.len(), measured_bytes);
            assert_eq!(encoded.capacity(), encoded.len());
            assert!(
                encoded.len() <= MAX_RECORD_RESPONSE_BYTES,
                "{}",
                encoded.len()
            );
            let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(
                parsed["payload"]["records"].as_array().unwrap().len(),
                lengths.len()
            );
            assert_eq!(
                STANDARD
                    .decode(parsed["payload"]["records"][0]["payload"].as_str().unwrap())
                    .unwrap()
                    .len(),
                lengths[0]
            );
        }
    }

    #[test]
    fn encoder_refuses_overflow_before_allocating_beyond_ceiling() {
        let mut overflowing = result(&[MAX_PHYSICAL_RECORD_PAYLOAD_BYTES]);
        overflowing.records[0].id = "\u{0001}".repeat(MAX_RECORD_RESPONSE_BYTES);
        assert_eq!(
            encode_response("request", &overflowing),
            Err(ReadEncodeError::ResponseTooLarge)
        );
    }

    #[test]
    fn page_request_rejects_unsafe_positions_and_out_of_bounds_budgets() {
        let mut wire = result(&[1]).request;
        wire.after = "0".into();
        wire.target = "1".into();
        assert!(decode_page_request(&wire).is_ok());
        for invalid in ["01", "-1", "+1", "18446744073709551616", "1.0", ""] {
            wire.target = invalid.into();
            assert!(matches!(
                decode_page_request(&wire),
                Err(ReadWireError::InvalidRequest)
            ));
        }
        wire.target = "1".into();
        for count in [0, (MAX_RECORD_PAGE_RECORDS + 1) as u64] {
            wire.max_records = count;
            assert!(matches!(
                decode_page_request(&wire),
                Err(ReadWireError::InvalidRequest)
            ));
        }
        wire.max_records = MAX_RECORD_PAGE_RECORDS as u64;
        wire.max_payload_bytes = (MAX_PHYSICAL_RECORD_PAYLOAD_BYTES + 1) as u64;
        assert!(matches!(
            decode_page_request(&wire),
            Err(ReadWireError::InvalidRequest)
        ));
    }

    #[test]
    fn sync_validation_rejects_foreign_scope_and_gap_before_encoding() {
        let mut wire = result(&[1]).request;
        wire.scope = RecordScope {
            receiver: "receiver".into(),
            origin: "origin".into(),
            stream: "conversation".into(),
            incarnation: "incarnation".into(),
            schema: "schema".into(),
            access_epoch: "epoch".into(),
        };
        wire.after = "0".into();
        wire.target = "2".into();
        let request = decode_page_request(&wire).unwrap();
        let record = Record {
            position: 1,
            id: Id::new("record").unwrap(),
            scope: request.scope.clone(),
            payload: vec![1],
        };
        let page = Page {
            request: request.clone(),
            records: vec![record.clone()],
        };
        assert!(encode_page("request", &request, page.clone()).is_ok());
        let mut wrong = page.clone();
        wrong.records[0].scope = Scope::new(
            Id::new("other").unwrap(),
            request.scope.origin().clone(),
            request.scope.stream().clone(),
            request.scope.incarnation().clone(),
            request.scope.schema().clone(),
            request.scope.access_epoch().clone(),
        );
        assert!(matches!(
            encode_page("request", &request, wrong),
            Err(ReadWireError::InvalidPage)
        ));
        let mut gap = page;
        gap.records[0].position = 2;
        assert!(matches!(
            encode_page("request", &request, gap),
            Err(ReadWireError::InvalidPage)
        ));
    }
}
