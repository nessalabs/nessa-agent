//! Strict manifest metadata, without inventing public record bodies.

use std::collections::HashSet;

use super::encoding::{decode_bounded, encode_identity};
use super::record::{checked_identity, MAX_NAME_BYTES};
use super::{
    DeferredEntry, Fingerprint, SkipReason, SkippedRecord, StoreKind, Watermark, EXPORT_HEADER,
    WRITER_INVENTORY,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestReadError {
    UnsupportedVersion,
    Malformed,
    EntryLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedRecord {
    identity: String,
    fingerprint: Fingerprint,
    public: bool,
}

impl ListedRecord {
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }
    pub fn is_public(&self) -> bool {
        self.public
    }
    pub fn encoded_identity(&self) -> String {
        encode_identity(&self.identity)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedStore {
    store: StoreKind,
    watermark: Watermark,
    records: Vec<ListedRecord>,
    skipped: Vec<SkippedRecord>,
    deferred: Vec<DeferredEntry>,
}

impl ListedStore {
    pub fn store(&self) -> StoreKind {
        self.store
    }
    pub fn watermark(&self) -> &Watermark {
        &self.watermark
    }
    pub fn records(&self) -> &[ListedRecord] {
        &self.records
    }
    pub fn skipped(&self) -> &[SkippedRecord] {
        &self.skipped
    }
    pub fn deferred(&self) -> &[DeferredEntry] {
        &self.deferred
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestListing {
    captured_at_ms: u64,
    stores: Vec<ListedStore>,
    canonical_len: usize,
    mac: Fingerprint,
}

impl ManifestListing {
    /// Read the writer's canonical v1 grammar. `max_entries` counts named
    /// records, redactions, deferred entries and skips; the fixed inventory's
    /// watermarks are excluded. The limit is checked before adding an entry.
    pub fn parse(text: &str, max_entries: usize) -> Result<Self, ManifestReadError> {
        use ManifestReadError::Malformed;
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
            let mut store = ListedStore {
                store: *kind,
                watermark,
                records: Vec::new(),
                skipped: Vec::new(),
                deferred: Vec::new(),
            };
            let mut public_identities = HashSet::new();
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
                let identity = checked_identity(canonical_value(parts.next().ok_or(Malformed)?)?)
                    .map_err(|_| Malformed)?;
                match entry {
                    "record" | "redacted" if phase == 0 => {
                        let public = entry == "record";
                        // Omission labels can repeat across source files. Public
                        // bodies select bundle paths, so their names are unique.
                        if public && !public_identities.insert(identity.clone()) {
                            return Err(Malformed);
                        }
                        if public && matches!(identity.as_str(), "." | "..") {
                            return Err(Malformed);
                        }
                        let fingerprint = strict_fingerprint(parts.next().ok_or(Malformed)?)?;
                        store.records.push(ListedRecord {
                            identity,
                            fingerprint,
                            public,
                        });
                    }
                    "deferred" if phase <= 1 => {
                        phase = 1;
                        let size = decimal(parts.next().ok_or(Malformed)?)?;
                        let digest = strict_fingerprint(parts.next().ok_or(Malformed)?)?;
                        store.deferred.push(
                            DeferredEntry::new(*kind, identity, size, digest)
                                .map_err(|_| Malformed)?,
                        );
                    }
                    "skipped" => {
                        phase = 2;
                        let reason =
                            SkipReason::parse(parts.next().ok_or(Malformed)?).ok_or(Malformed)?;
                        store.skipped.push(
                            SkippedRecord::new(*kind, identity, reason).map_err(|_| Malformed)?,
                        );
                    }
                    _ => return Err(Malformed),
                }
                if parts.next().is_some() {
                    return Err(Malformed);
                }
                count += 1;
            }
            stores.push(store);
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
        Ok(Self {
            captured_at_ms,
            stores,
            canonical_len,
            mac,
        })
    }

    /// Byte offset immediately before the final `mac` line.
    pub fn canonical_len(&self) -> usize {
        self.canonical_len
    }
    pub fn mac(&self) -> Fingerprint {
        self.mac
    }
    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }
    pub fn stores(&self) -> &[ListedStore] {
        &self.stores
    }
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
