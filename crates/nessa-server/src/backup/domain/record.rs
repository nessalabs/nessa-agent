//! One store's contribution to a cut: a watermark, public records, and
//! the records that could not be read.

use sha2::{Digest, Sha256};

use super::inventory::StoreKind;

/// The store's own cursor at the moment the hold was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watermark(String);

impl Watermark {
    /// `token` as the store spelled it. Empty is refused: a cut that
    /// omitted the cursor would not be checkable later.
    pub fn new(token: impl Into<String>) -> Result<Self, RecordError> {
        let token = token.into();
        if token.is_empty() || token.len() > 512 || token.bytes().any(|byte| byte < 0x20) {
            return Err(RecordError::Watermark);
        }
        Ok(Self(token))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    /// The session file is above the export size cap, so it was not copied.
    Oversized,
    /// The bytes hold key material. A fingerprint may still name them.
    Secret,
}

impl SkipReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Unavailable => "unavailable",
            Self::Unnameable => "unnameable",
            Self::Oversized => "oversized",
            Self::Secret => "secret",
        }
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

/// What one store contributed from the snapshot taken while admission was held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCut {
    store: StoreKind,
    watermark: Watermark,
    records: Vec<CapturedRecord>,
    skipped: Vec<SkippedRecord>,
}

impl SourceCut {
    pub fn new(
        store: StoreKind,
        watermark: Watermark,
        records: Vec<CapturedRecord>,
        skipped: Vec<SkippedRecord>,
    ) -> Result<Self, RecordError> {
        if records.iter().any(|record| record.store != store)
            || skipped.iter().any(|record| record.store != store)
        {
            return Err(RecordError::Store);
        }
        Ok(Self {
            store,
            watermark,
            records,
            skipped,
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
}

/// Why a record was refused before it could enter a cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordError {
    /// The identity was empty, too long, or contained a control character.
    Identity,
    /// The watermark was empty or contained a control character.
    Watermark,
    /// A record named a different store than the cut it was placed in.
    Store,
}

fn checked_identity(identity: String) -> Result<String, RecordError> {
    if identity.is_empty() || identity.len() > 512 || identity.bytes().any(|byte| byte < 0x20) {
        return Err(RecordError::Identity);
    }
    Ok(identity)
}
