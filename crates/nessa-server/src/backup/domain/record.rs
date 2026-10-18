//! One store's contribution to a cut: a watermark, public records, and
//! the records that could not be read.

use sha2::{Digest, Sha256};

use super::inventory::StoreKind;

/// Byte bound shared by record identities and watermarks.
pub(crate) const MAX_NAME_BYTES: usize = 512;

/// The store's own cursor at the moment the hold was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watermark(String);

impl Watermark {
    /// `token` as the store spelled it. Empty is refused: a cut that
    /// omitted the cursor would not be checkable later.
    pub fn new(token: impl Into<String>) -> Result<Self, RecordError> {
        let token = token.into();
        if token.is_empty() || token.len() > MAX_NAME_BYTES || token.bytes().any(|byte| byte < 0x20)
        {
            return Err(RecordError::Watermark);
        }
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Exact cursor of one outbox snapshot.
///
/// `rows` are the identities and byte lengths observed together. The token
/// is the hex SHA-256 of those pairs sorted by identity, one
/// `identity SP length LF` line each. An empty snapshot still has a token:
/// the hash of no lines. Two snapshots that differ by an appended byte or
/// a new row therefore differ here. `an_outbox_watermark_names_the_pinned_rows`
/// and `a_concurrent_outbox_append_is_outside_the_cut`.
pub fn outbox_watermark(rows: &[(&str, u64)]) -> Result<Watermark, RecordError> {
    let mut ordered = rows.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(right.0));
    let mut text = String::new();
    for (identity, length) in ordered {
        if identity.is_empty()
            || identity.len() > MAX_NAME_BYTES
            || identity.bytes().any(|byte| byte <= 0x20)
        {
            return Err(RecordError::Identity);
        }
        text.push_str(identity);
        text.push(' ');
        text.push_str(&length.to_string());
        text.push('\n');
    }
    Watermark::new(Fingerprint::of(text.as_bytes()).hex())
}

/// SHA-256 of bytes the bundle must not contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// `bytes` is already a 32-byte digest. This does not hash them again.
    pub fn from_digest(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Hash `bytes` and keep only the hash.
    pub fn of(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self(out)
    }

    /// Decode a SHA-256 spelling. Uppercase digits remain accepted by the
    /// live deletion inventory; strict listings compare with `hex()`.
    pub fn from_hex(hex: &str) -> Result<Self, RecordError> {
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(RecordError::Fingerprint);
        }
        let mut bytes = [0u8; 32];
        for (output, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let text = std::str::from_utf8(pair).map_err(|_| RecordError::Fingerprint)?;
            *output = u8::from_str_radix(text, 16).map_err(|_| RecordError::Fingerprint)?;
        }
        Ok(Self::from_digest(bytes))
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            text.push(HEX[(byte >> 4) as usize] as char);
            text.push(HEX[(byte & 0xf) as usize] as char);
        }
        text
    }
}

/// Whether a record's bytes may be written into the bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensitivity {
    /// The bytes are the stored record.
    Public,
    /// The bytes are secret material. The cut keeps [`Fingerprint`] only.
    Secret,
}

/// What of a record the bundle stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordBody {
    /// Public bytes, kept so a later check can hash them again.
    Public(Vec<u8>),
    /// A secret, reduced to its fingerprint. The original bytes are gone.
    Redacted(Fingerprint),
}

/// One record the cut accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedRecord {
    store: StoreKind,
    identity: String,
    body: RecordBody,
}

impl CapturedRecord {
    /// `identity` names the record inside `store`. Secret `bytes` are hashed
    /// and dropped here, so a caller cannot put them in the bundle afterwards.
    pub fn new(
        store: StoreKind,
        identity: impl Into<String>,
        sensitivity: Sensitivity,
        bytes: Vec<u8>,
    ) -> Result<Self, RecordError> {
        let identity = checked_identity(identity.into())?;
        let body = match sensitivity {
            Sensitivity::Public => RecordBody::Public(bytes),
            Sensitivity::Secret => {
                let fingerprint = Fingerprint::of(&bytes);
                drop(bytes);
                RecordBody::Redacted(fingerprint)
            }
        };
        Ok(Self {
            store,
            identity,
            body,
        })
    }

    pub fn store(&self) -> StoreKind {
        self.store
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn body(&self) -> &RecordBody {
        &self.body
    }

    /// The bytes a bundle may write, when the record is public.
    pub fn public_bytes(&self) -> Option<&[u8]> {
        match &self.body {
            RecordBody::Public(bytes) => Some(bytes),
            RecordBody::Redacted(_) => None,
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        match &self.body {
            RecordBody::Public(bytes) => Fingerprint::of(bytes),
            RecordBody::Redacted(fingerprint) => *fingerprint,
        }
    }
}

/// Why a record is named in the cut but not included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The record could not be read.
    Unreadable,
    /// The store itself could not be listed.
    Unavailable,
    /// The record's identity or watermark was not one this cut can name.
    Unnameable,
    /// The file is larger than the public-file bound, so it was not read.
    Bounded,
    /// The bytes hold key material. A fingerprint may still name them.
    Secret,
}

impl SkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Unavailable => "unavailable",
            Self::Unnameable => "unnameable",
            Self::Bounded => "bounded",
            Self::Secret => "secret",
        }
    }

    /// The canonical spelling emitted by the manifest writer.
    pub fn parse(token: &str) -> Option<Self> {
        [
            Self::Unreadable,
            Self::Unavailable,
            Self::Unnameable,
            Self::Bounded,
            Self::Secret,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == token)
    }
}

/// A record or a whole store the cut reports instead of failing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedRecord {
    store: StoreKind,
    identity: String,
    reason: SkipReason,
}

impl SkippedRecord {
    pub fn new(
        store: StoreKind,
        identity: impl Into<String>,
        reason: SkipReason,
    ) -> Result<Self, RecordError> {
        Ok(Self {
            store,
            identity: checked_identity(identity.into())?,
            reason,
        })
    }

    pub fn store(&self) -> StoreKind {
        self.store
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn reason(&self) -> SkipReason {
        self.reason
    }
}

/// A conversation left out of the snapshot, named so a later fetch can find it.
///
/// The bytes stay in the source. `digest` is the stream's incarnation, floor,
/// tail, and records at the cut. This cut does not fetch them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredEntry {
    store: StoreKind,
    conversation_id: String,
    size: u64,
    digest: Fingerprint,
}

impl DeferredEntry {
    pub fn new(
        store: StoreKind,
        conversation_id: impl Into<String>,
        size: u64,
        digest: Fingerprint,
    ) -> Result<Self, RecordError> {
        Ok(Self {
            store,
            conversation_id: checked_identity(conversation_id.into())?,
            size,
            digest,
        })
    }

    pub fn store(&self) -> StoreKind {
        self.store
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn digest(&self) -> Fingerprint {
        self.digest
    }
}

/// What one store contributed from the snapshot taken while admission was held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCut {
    store: StoreKind,
    watermark: Watermark,
    records: Vec<CapturedRecord>,
    skipped: Vec<SkippedRecord>,
    deferred: Vec<DeferredEntry>,
}

impl SourceCut {
    pub fn new(
        store: StoreKind,
        watermark: Watermark,
        records: Vec<CapturedRecord>,
        skipped: Vec<SkippedRecord>,
    ) -> Result<Self, RecordError> {
        Self::with_deferred(store, watermark, records, skipped, Vec::new())
    }

    pub fn with_deferred(
        store: StoreKind,
        watermark: Watermark,
        records: Vec<CapturedRecord>,
        skipped: Vec<SkippedRecord>,
        deferred: Vec<DeferredEntry>,
    ) -> Result<Self, RecordError> {
        if records.iter().any(|record| record.store != store)
            || skipped.iter().any(|record| record.store != store)
            || deferred.iter().any(|entry| entry.store != store)
        {
            return Err(RecordError::Store);
        }
        Ok(Self {
            store,
            watermark,
            records,
            skipped,
            deferred,
        })
    }

    pub fn store(&self) -> StoreKind {
        self.store
    }

    pub fn watermark(&self) -> &Watermark {
        &self.watermark
    }

    pub fn records(&self) -> &[CapturedRecord] {
        &self.records
    }

    pub fn skipped(&self) -> &[SkippedRecord] {
        &self.skipped
    }

    pub fn deferred(&self) -> &[DeferredEntry] {
        &self.deferred
    }

    /// The store was not on disk. No rows, and no skip.
    pub fn absent(store: StoreKind) -> Result<Self, RecordError> {
        Self::new(store, Watermark::new("absent")?, Vec::new(), Vec::new())
    }
}

/// Why a record was refused before it could enter a cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordError {
    /// The digest was not 64 ASCII hexadecimal digits.
    Fingerprint,
    /// The identity was empty, too long, or contained a control character.
    Identity,
    /// The watermark was empty or contained a control character.
    Watermark,
    /// A record named a different store than the cut it was placed in.
    Store,
}

pub(super) fn checked_identity(identity: String) -> Result<String, RecordError> {
    if identity.is_empty()
        || identity.len() > MAX_NAME_BYTES
        || identity.bytes().any(|byte| byte < 0x20)
    {
        return Err(RecordError::Identity);
    }
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outbox_watermark_names_the_pinned_rows() {
        let first = outbox_watermark(&[("b", 2), ("a", 1)]).unwrap();
        let same = outbox_watermark(&[("a", 1), ("b", 2)]).unwrap();
        let longer = outbox_watermark(&[("a", 1), ("b", 3)]).unwrap();
        let extra = outbox_watermark(&[("a", 1), ("b", 2), ("c", 1)]).unwrap();
        let empty = outbox_watermark(&[]).unwrap();
        assert_eq!(first, same);
        assert_ne!(first, longer);
        assert_ne!(first, extra);
        assert_ne!(first, empty);
        assert_eq!(first.as_str().len(), 64);
        assert!(outbox_watermark(&[("has space", 1)]).is_err());
        assert!(outbox_watermark(&[("", 1)]).is_err());
    }
}
