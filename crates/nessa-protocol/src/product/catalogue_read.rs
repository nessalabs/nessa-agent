//! Lossless catalogue DTO mapping; shared passive helpers own decimal and JSON bounds.

use super::passive_read::{
    decimal_u64, decode_payload, decode_scope, encode_response, wire_scope, ReadEncodeError,
    ReadWireError,
};
use crate::product::generated::{
    CatalogueDescriptor, CatalogueEntryKey, CatalogueManifestRequest, CataloguePass as WirePass,
    ConversationCatalogueHeadResult, ConversationCatalogueManifestResult,
    ConversationCatalogueResolveResult,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_sync::replication::{
    catalogue::{
        validate_manifest, validate_resolved, CataloguePass, EntryKey, ManifestEntry, ManifestPage,
        ManifestRequest, ResolvedEntry, MAX_CATALOGUE_ENTRIES,
    },
    domain::{Id, Scope},
};

pub fn decode_key(wire: &CatalogueEntryKey) -> Result<EntryKey, ReadWireError> {
    Ok(EntryKey {
        creation: decimal_u64(&wire.creation)?,
        id: Id::new(&wire.id).map_err(|_| ReadWireError::InvalidRequest)?,
    })
}

pub fn decode_descriptor(wire: &CatalogueDescriptor) -> Result<ManifestEntry, ReadWireError> {
    Ok(ManifestEntry {
        key: decode_key(&wire.key)?,
        revision: decimal_u64(&wire.revision)?,
        deleted: wire.deleted,
    })
}

pub fn decode_pass(wire: &WirePass) -> Result<CataloguePass, ReadWireError> {
    Ok(CataloguePass {
        scope: decode_scope(&wire.scope)?,
        completed: decimal_u64(&wire.completed)?,
        boundary: decimal_u64(&wire.boundary)?,
        cursor: wire.cursor.as_ref().map(decode_key).transpose()?,
        generation: decimal_u64(&wire.generation)?,
    })
}

pub fn decode_manifest(wire: &CatalogueManifestRequest) -> Result<ManifestRequest, ReadWireError> {
    Ok(ManifestRequest {
        pass: decode_pass(&wire.pass)?,
        max_entries: usize::try_from(wire.max_entries)
            .map_err(|_| ReadWireError::InvalidRequest)?,
    })
}

pub fn wire_key(key: &EntryKey) -> CatalogueEntryKey {
    CatalogueEntryKey {
        creation: key.creation.to_string(),
        id: key.id.as_str().to_owned(),
    }
}

pub fn wire_descriptor(entry: &ManifestEntry) -> CatalogueDescriptor {
    CatalogueDescriptor {
        key: wire_key(&entry.key),
        revision: entry.revision.to_string(),
        deleted: entry.deleted,
    }
}

pub fn wire_pass(pass: &CataloguePass) -> WirePass {
    WirePass {
        scope: wire_scope(&pass.scope),
        completed: pass.completed.to_string(),
        boundary: pass.boundary.to_string(),
        cursor: pass.cursor.as_ref().map(wire_key),
        generation: pass.generation.to_string(),
    }
}

pub fn encode_head(request_id: &str, scope: &Scope, head: u64) -> Result<String, ReadEncodeError> {
    encode_response(
        request_id,
        &ConversationCatalogueHeadResult {
            scope: wire_scope(scope),
            head: head.to_string(),
        },
    )
}

pub fn encode_manifest(
    request_id: &str,
    expected: &ManifestRequest,
    page: ManifestPage,
) -> Result<String, ReadEncodeError> {
    validate_manifest(expected, &page, MAX_CATALOGUE_ENTRIES)
        .map_err(|_| ReadEncodeError::InvalidPayload)?;
    encode_response(
        request_id,
        &ConversationCatalogueManifestResult {
            request: CatalogueManifestRequest {
                pass: wire_pass(&page.request.pass),
                max_entries: page.request.max_entries as u64,
            },
            entries: page.entries.iter().map(wire_descriptor).collect(),
            has_more: page.has_more,
        },
    )
}

pub fn encode_resolved(
    request_id: &str,
    pass: &CataloguePass,
    descriptor: &ManifestEntry,
    max_payload_bytes: usize,
    resolved: ResolvedEntry,
) -> Result<String, ReadEncodeError> {
    validate_resolved(descriptor, &resolved, max_payload_bytes)
        .map_err(|_| ReadEncodeError::InvalidPayload)?;
    encode_response(
        request_id,
        &ConversationCatalogueResolveResult {
            pass: wire_pass(pass),
            descriptor: wire_descriptor(descriptor),
            entry: wire_descriptor(&resolved.manifest),
            payload: STANDARD.encode(resolved.payload),
        },
    )
}

pub fn decode_head(wire: ConversationCatalogueHeadResult) -> Result<(Scope, u64), ReadWireError> {
    Ok((decode_scope(&wire.scope)?, decimal_u64(&wire.head)?))
}
pub fn decode_manifest_result(
    wire: ConversationCatalogueManifestResult,
) -> Result<ManifestPage, ReadWireError> {
    if wire.entries.len() > MAX_CATALOGUE_ENTRIES {
        return Err(ReadWireError::ResponseTooLarge);
    }
    Ok(ManifestPage {
        request: decode_manifest(&wire.request)?,
        entries: wire
            .entries
            .iter()
            .map(decode_descriptor)
            .collect::<Result<_, _>>()?,
        has_more: wire.has_more,
    })
}
pub fn decode_resolved_result(
    wire: ConversationCatalogueResolveResult,
    maximum: usize,
) -> Result<(CataloguePass, ManifestEntry, ResolvedEntry), ReadWireError> {
    Ok((
        decode_pass(&wire.pass)?,
        decode_descriptor(&wire.descriptor)?,
        ResolvedEntry {
            manifest: decode_descriptor(&wire.entry)?,
            payload: decode_payload(&wire.payload, maximum)?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::generated::MAX_RECORD_RESPONSE_BYTES;
    use nessa_sync::replication::catalogue::{MAX_CATALOGUE_ENTRIES, MAX_CATALOGUE_PAYLOAD_BYTES};
    use serde_json::Value;

    fn pass() -> CataloguePass {
        let escaped = Id::new("\u{0001}".repeat(128)).unwrap();
        CataloguePass {
            scope: Scope::new(
                escaped.clone(),
                escaped.clone(),
                escaped.clone(),
                escaped.clone(),
                escaped,
                Id::new(u64::MAX.to_string()).unwrap(),
            ),
            completed: 0,
            boundary: u64::MAX,
            cursor: None,
            generation: u64::MAX,
        }
    }
    fn descriptor(index: usize) -> ManifestEntry {
        ManifestEntry {
            key: EntryKey {
                creation: (index + 1) as u64,
                id: Id::new(format!("018fa012-2222-7222-8222-{index:012}")).unwrap(),
            },
            revision: u64::MAX,
            deleted: false,
        }
    }

    #[test]
    fn maximal_authentic_manifest_and_base64_payload_fit_measured_wire_budget() {
        let request_id = "\u{0001}".repeat(256);
        let pass = pass();
        let request = ManifestRequest {
            pass: pass.clone(),
            max_entries: MAX_CATALOGUE_ENTRIES,
        };
        let page = ManifestPage {
            request,
            entries: (0..MAX_CATALOGUE_ENTRIES).map(descriptor).collect(),
            has_more: true,
        };
        let manifest = encode_manifest(&request_id, &page.request.clone(), page).unwrap();
        let entry = descriptor(0);
        let resolved = ResolvedEntry {
            manifest: entry.clone(),
            payload: vec![0xff; 65_536],
        };
        let response = encode_resolved(&request_id, &pass, &entry, 65_536, resolved).unwrap();
        assert!(
            manifest.len() < MAX_RECORD_RESPONSE_BYTES,
            "manifest bytes {}",
            manifest.len()
        );
        assert!(
            response.len() < MAX_RECORD_RESPONSE_BYTES,
            "resolve bytes {}",
            response.len()
        );
        eprintln!(
            "catalogue maximal manifest={} resolve65536={} wire ceiling={}",
            manifest.len(),
            response.len(),
            MAX_RECORD_RESPONSE_BYTES
        );
        let decoded: ConversationCatalogueResolveResult = serde_json::from_str::<Value>(&response)
            .and_then(|value| serde_json::from_value(value["payload"].clone()))
            .unwrap();
        assert_eq!(STANDARD.decode(decoded.payload).unwrap().len(), 65_536);
    }

    #[test]
    fn core_allowed_payload_that_exceeds_actual_wire_cap_is_typed_refusal() {
        let pass = pass();
        let entry = descriptor(0);
        let resolved = ResolvedEntry {
            manifest: entry.clone(),
            payload: vec![0xff; MAX_CATALOGUE_PAYLOAD_BYTES],
        };
        assert_eq!(
            encode_resolved(
                "request",
                &pass,
                &entry,
                MAX_CATALOGUE_PAYLOAD_BYTES,
                resolved
            ),
            Err(ReadEncodeError::ResponseTooLarge)
        );
    }

    #[test]
    fn custom_source_response_contradictions_are_refused_by_core_codec() {
        let requested = ManifestRequest {
            pass: pass(),
            max_entries: 1,
        };
        let mut page = ManifestPage {
            request: requested.clone(),
            entries: vec![descriptor(0)],
            has_more: false,
        };
        assert!(encode_manifest("request", &requested, page.clone()).is_ok());
        page.request.pass.generation -= 1;
        assert_eq!(
            encode_manifest("request", &requested, page),
            Err(ReadEncodeError::InvalidPayload)
        );
        let expected = ManifestEntry {
            revision: 2,
            ..descriptor(0)
        };
        let mut actual = ResolvedEntry {
            manifest: expected.clone(),
            payload: vec![1],
        };
        assert!(encode_resolved("request", &requested.pass, &expected, 1, actual.clone()).is_ok());
        actual.manifest.key = descriptor(1).key;
        assert_eq!(
            encode_resolved("request", &requested.pass, &expected, 1, actual),
            Err(ReadEncodeError::InvalidPayload)
        );
        let deleted = ResolvedEntry {
            manifest: ManifestEntry {
                deleted: true,
                ..expected.clone()
            },
            payload: vec![],
        };
        assert_eq!(
            encode_resolved("request", &requested.pass, &expected, 1, deleted.clone()),
            Err(ReadEncodeError::InvalidPayload)
        );
        let newer_deleted = ResolvedEntry {
            manifest: ManifestEntry {
                revision: 3,
                ..deleted.manifest
            },
            payload: deleted.payload,
        };
        assert!(encode_resolved("request", &requested.pass, &expected, 1, newer_deleted).is_ok());
    }

    #[test]
    fn decimal_mapping_preserves_max_u64_and_rejects_noncanonical_revisions() {
        let entry = descriptor(0);
        let mut wire = wire_descriptor(&entry);
        assert_eq!(decode_descriptor(&wire).unwrap(), entry);
        for revision in ["01", "+1", "-1", "1.0", "18446744073709551616", ""] {
            wire.revision = revision.into();
            assert!(matches!(
                decode_descriptor(&wire),
                Err(ReadWireError::InvalidRequest)
            ));
        }
    }
}
