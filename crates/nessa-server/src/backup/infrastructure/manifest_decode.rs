//! The writer's bounded canonical v1 manifest grammar, mapped into validated metadata.

use self::ManifestReadError::Malformed;
use super::encoding::decode_bounded;
use crate::backup::application::{DecodedManifest, ManifestReadError};
use crate::backup::domain::{
    encode_identity, DeferredEntry, Fingerprint, ListedRecord, ListedRecords, ListedStore,
    ManifestListing, SkipReason, SkippedRecord, Watermark, EXPORT_HEADER, MAX_NAME_BYTES,
    WRITER_INVENTORY,
};

/// `max_entries` counts records, redactions, deferred entries and skips, checked before allocation.
pub(super) fn decode_manifest(
    text: &str,
    max_entries: usize,
) -> Result<DecodedManifest, ManifestReadError> {
    let mut lines = text.split_terminator('\n').peekable();
    match lines.next() {
        Some(header) if header == EXPORT_HEADER => {}
        Some(header)
            if header
                .strip_prefix("nessa-export-cut/")
                .is_some_and(|version| decimal(version).is_ok()) =>
        {
            return Err(ManifestReadError::UnsupportedVersion);
        }
        _ => return Err(Malformed),
    }
    if !text.ends_with('\n') {
        return Err(Malformed);
    }
    let captured_at_ms = decimal(
        lines
            .next()
            .and_then(|line| line.strip_prefix("captured-at "))
            .ok_or(Malformed)?,
    )?;
    let mut stores = Vec::new();
    let mut count = 0usize;
    for kind in WRITER_INVENTORY {
        let mut tokens = lines.next().ok_or(Malformed)?.split(' ');
        if tokens.next() != Some("watermark") || tokens.next() != Some(kind.as_str()) {
            return Err(Malformed);
        }
        let watermark = Watermark::new(canonical_value(tokens.next().ok_or(Malformed)?)?)
            .map_err(|_| Malformed)?;
        if tokens.next().is_some() {
            return Err(Malformed);
        }
        let mut records = ListedRecords::default();
        let mut skipped = Vec::new();
        let mut deferred = Vec::new();
        let mut phase = 0;
        while let Some(line) = lines.peek() {
            if line.starts_with("watermark ") || line.starts_with("mac ") {
                break;
            }
            if count >= max_entries {
                return Err(ManifestReadError::EntryLimit);
            }
            let line = lines.next().ok_or(Malformed)?;
            let mut parts = line.split(' ');
            let entry = parts.next().ok_or(Malformed)?;
            if parts.next() != Some(kind.as_str()) {
                return Err(Malformed);
            }
            let identity = canonical_value(parts.next().ok_or(Malformed)?)?;
            match entry {
                "record" | "redacted" if phase == 0 => {
                    let public = entry == "record";
                    let fingerprint = strict_fingerprint(parts.next().ok_or(Malformed)?)?;
                    records = records
                        .with_record(
                            ListedRecord::new(identity, fingerprint, public)
                                .map_err(|_| Malformed)?,
                        )
                        .map_err(|_| Malformed)?;
                }
                "deferred" if phase <= 1 => {
                    phase = 1;
                    let size = decimal(parts.next().ok_or(Malformed)?)?;
                    let digest = strict_fingerprint(parts.next().ok_or(Malformed)?)?;
                    deferred.push(
                        DeferredEntry::new(*kind, identity, size, digest).map_err(|_| Malformed)?,
                    );
                }
                "skipped" => {
                    phase = 2;
                    let reason =
                        SkipReason::parse(parts.next().ok_or(Malformed)?).ok_or(Malformed)?;
                    skipped
                        .push(SkippedRecord::new(*kind, identity, reason).map_err(|_| Malformed)?);
                }
                _ => return Err(Malformed),
            }
            if parts.next().is_some() {
                return Err(Malformed);
            }
            count += 1;
        }
        stores.push(
            ListedStore::new(*kind, watermark, records, skipped, deferred)
                .map_err(|_| Malformed)?,
        );
    }
    let mac_line = lines.next().ok_or(Malformed)?;
    let mac = strict_fingerprint(mac_line.strip_prefix("mac ").ok_or(Malformed)?)?;
    if lines.next().is_some() {
        return Err(Malformed);
    }
    let canonical_len = text
        .len()
        .checked_sub(mac_line.len() + 1)
        .ok_or(Malformed)?;
    Ok(DecodedManifest {
        listing: ManifestListing::new(captured_at_ms, stores).map_err(|_| Malformed)?,
        canonical_len,
        mac,
    })
}

fn decimal(token: &str) -> Result<u64, ManifestReadError> {
    let number = token
        .parse::<u64>()
        .map_err(|_| ManifestReadError::Malformed)?;
    if number.to_string() != token {
        return Err(ManifestReadError::Malformed);
    }
    Ok(number)
}

fn canonical_value(token: &str) -> Result<String, ManifestReadError> {
    if token.len() > MAX_NAME_BYTES * 3 {
        return Err(ManifestReadError::Malformed);
    }
    let value = decode_bounded(token, MAX_NAME_BYTES).map_err(|()| ManifestReadError::Malformed)?;
    if encode_identity(&value) != token {
        return Err(ManifestReadError::Malformed);
    }
    Ok(value)
}

fn strict_fingerprint(token: &str) -> Result<Fingerprint, ManifestReadError> {
    let fingerprint = Fingerprint::from_hex(token).map_err(|_| ManifestReadError::Malformed)?;
    if fingerprint.hex() != token {
        return Err(ManifestReadError::Malformed);
    }
    Ok(fingerprint)
}
