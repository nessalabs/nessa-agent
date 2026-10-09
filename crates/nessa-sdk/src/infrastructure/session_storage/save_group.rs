//! One bounded codec and publication authority for unpublished save units.
//! Physical framing validates each body; this owner validates the save lineage,
//! exact boundaries and payload digest without retaining semantic bodies.

use crate::application::agent_execution::sessions::{
    records::{FactKey, FactKind},
    SessionSaveBackend, SessionSaveGeneration, StorageError,
};
use event_stream::StreamKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(super) const HEADER_BYTES: usize = 136;
pub(super) const EMPTY_CHAIN: [u8; 32] = [0; 32];
pub(super) fn validate_unit_payload(payload: &[u8]) -> Result<(), StorageError> {
    if payload.len() > super::stream_fact::MAX_BODY_BYTES - HEADER_BYTES {
        return Err(StorageError::TooLarge);
    }
    Ok(())
}

// The field declaration generates both serde's typed wire contract and the
// preflight resource map. Consumers never restate keys or array widths.
#[derive(Clone, Copy)]
pub(super) enum MetadataValueKind {
    Identity,
    FixedBytes(usize),
    Number,
}
trait MetadataValue {
    const KIND: MetadataValueKind;
}
impl<const N: usize> MetadataValue for [u8; N] {
    const KIND: MetadataValueKind = MetadataValueKind::FixedBytes(N);
}
impl MetadataValue for u64 {
    const KIND: MetadataValueKind = MetadataValueKind::Number;
}
macro_rules! checkpoint_metadata {
    ($(#[$attribute:meta])* $visibility:vis struct $name:ident {
        $($field_visibility:vis $field:ident: $field_type:ty),* $(,)?
    }) => {
        #[derive(Clone, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        $(#[$attribute])*
        $visibility struct $name {
            $($field_visibility $field: $field_type),*
        }
        impl $name {
            pub(super) fn field_kind(key: &str) -> Option<MetadataValueKind> {
                match key {
                    $(stringify!($field) => Some(<$field_type as MetadataValue>::KIND)),*,
                    _ => None,
                }
            }
        }
    };
}
checkpoint_metadata! {
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct SaveIdentity {
        stream: [u8; 32],
        incarnation: [u8; 16],
        pub(super) base: u64,
        pub(super) generation: u64,
    }
}
impl MetadataValue for SaveIdentity {
    const KIND: MetadataValueKind = MetadataValueKind::Identity;
}
impl SaveIdentity {
    pub(super) fn matches_stream(&self, stream: &StreamKey) -> bool {
        self.stream == Sha256::digest(stream.id.as_str().as_bytes()).as_slice()
            && self.incarnation == stream.incarnation.0
    }
    pub(super) fn matches_scope(&self, stream: &str, incarnation: &str) -> bool {
        self.stream == Sha256::digest(stream.as_bytes()).as_slice()
            && Uuid::parse_str(incarnation).is_ok_and(|id| id.as_bytes() == &self.incarnation)
    }
    pub(super) fn binding(binding: &SessionSaveGeneration) -> Result<Self, StorageError> {
        let SessionSaveBackend::Record {
            stream,
            incarnation,
        } = binding.backend()
        else {
            return Err(invalid());
        };
        Ok(Self {
            stream: Sha256::digest(stream.as_str().as_bytes()).into(),
            incarnation: *incarnation,
            base: binding.base(),
            generation: binding.generation(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub(super) identity: SaveIdentity,
    pub(super) ordinal: u64,
    pub(super) previous: [u8; 32],
    pub(super) payload: [u8; 32],
}
impl Header {
    pub(super) fn unit(
        identity: SaveIdentity,
        ordinal: u64,
        previous: [u8; 32],
        payload: &[u8],
    ) -> Self {
        Self {
            identity,
            ordinal,
            previous,
            payload: Sha256::digest(payload).into(),
        }
    }
    pub(super) fn encode(&self, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
        bytes.extend_from_slice(&self.identity.stream);
        bytes.extend_from_slice(&self.identity.incarnation);
        bytes.extend_from_slice(&self.identity.base.to_be_bytes());
        bytes.extend_from_slice(&self.identity.generation.to_be_bytes());
        bytes.extend_from_slice(&self.ordinal.to_be_bytes());
        bytes.extend_from_slice(&self.previous);
        bytes.extend_from_slice(&self.payload);
        bytes.extend_from_slice(payload);
        bytes
    }
    pub(super) fn decode(bytes: &[u8]) -> Result<Self, StorageError> {
        if bytes.len() < HEADER_BYTES {
            return Err(invalid());
        }
        Ok(Self {
            identity: SaveIdentity {
                stream: bytes[0..32].try_into().map_err(|_| invalid())?,
                incarnation: bytes[32..48].try_into().map_err(|_| invalid())?,
                base: u64::from_be_bytes(bytes[48..56].try_into().map_err(|_| invalid())?),
                generation: u64::from_be_bytes(bytes[56..64].try_into().map_err(|_| invalid())?),
            },
            ordinal: u64::from_be_bytes(bytes[64..72].try_into().map_err(|_| invalid())?),
            previous: bytes[72..104].try_into().map_err(|_| invalid())?,
            payload: bytes[104..136].try_into().map_err(|_| invalid())?,
        })
    }
    pub(super) fn chain(&self, payload_length: u64) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.encode(&[]));
        hash.update(payload_length.to_be_bytes());
        hash.finalize().into()
    }
}

#[derive(Clone)]
struct Extent {
    group: GroupCheckpoint,
    complete: bool,
}
checkpoint_metadata! {
    pub(super) struct GroupCheckpoint {
        identity: SaveIdentity,
        count: u64,
        chain: [u8; 32],
        unit_previous: [u8; 32],
        unit_payload: [u8; 32],
        unit_length: u64,
    }
}
impl GroupCheckpoint {
    fn agrees_with_original_unit(&self) -> bool {
        // Restore has already admitted the nonzero completed Unit count.
        Header {
            identity: self.identity.clone(),
            ordinal: self.count - 1,
            previous: self.unit_previous,
            payload: self.unit_payload,
        }
        .chain(self.unit_length)
            == self.chain
    }
}

#[derive(Clone)]
pub(super) struct GroupProgress {
    extent: Option<Extent>,
    published: u64,
    prefix: Vec<u8>,
    hash: Sha256,
    payload_length: u64,
    checkpoint: Option<GroupCheckpoint>,
    /// Physical end of a dropped save, when a later group was written from
    /// there. Semantic [`Self::published`] stays the last folded completion.
    lineage_base: Option<u64>,
}
impl GroupProgress {
    pub(super) fn after(published: u64) -> Self {
        Self {
            extent: None,
            published,
            prefix: Vec::with_capacity(HEADER_BYTES),
            hash: Sha256::new(),
            payload_length: 0,
            checkpoint: None,
            lineage_base: None,
        }
    }
    pub(super) fn lineage_base(&self) -> Option<u64> {
        self.lineage_base
    }
    pub(super) fn set_lineage_base(&mut self, base: Option<u64>) {
        self.lineage_base = base;
    }
    pub(super) fn allocation_bytes(&self) -> usize {
        self.prefix.capacity()
    }
    pub(super) fn checkpoint(&self) -> Option<GroupCheckpoint> {
        self.checkpoint.clone()
    }
    pub(super) fn restore(
        published: u64,
        checkpoint: Option<GroupCheckpoint>,
        stream: &str,
        incarnation: &str,
        facts: u64,
    ) -> Result<Self, StorageError> {
        let mut progress = Self::after(published);
        if let Some(checkpoint) = checkpoint {
            if checkpoint.count == 0
                || checkpoint.count > facts
                || ((checkpoint.identity.generation == 0) != (checkpoint.identity.base == 0))
                || (checkpoint.identity.generation == 0 && checkpoint.count != facts)
                || checkpoint.identity.base >= published
                || !checkpoint.identity.matches_scope(stream, incarnation)
                || !checkpoint.agrees_with_original_unit()
            {
                return Err(invalid());
            }
            progress.extent = Some(Extent {
                group: checkpoint.clone(),
                complete: true,
            });
            progress.checkpoint = Some(checkpoint);
        } else if published != 0 {
            return Err(invalid());
        }
        Ok(progress)
    }
    pub(super) fn is_unfinished(&self) -> bool {
        self.extent.as_ref().is_some_and(|extent| !extent.complete)
    }
    pub(super) fn published(&self) -> u64 {
        self.published
    }
    pub(super) fn reset_frame(&mut self) {
        self.prefix.clear();
        self.hash = Sha256::new();
        self.payload_length = 0;
    }
    pub(super) fn piece(&mut self, mut bytes: &[u8]) -> Result<(), StorageError> {
        let take = bytes.len().min(HEADER_BYTES - self.prefix.len());
        self.prefix.extend_from_slice(&bytes[..take]);
        bytes = &bytes[take..];
        self.payload_length = self
            .payload_length
            .checked_add(bytes.len() as u64)
            .ok_or_else(invalid)?;
        self.hash.update(bytes);
        Ok(())
    }
    pub(super) fn complete(
        &mut self,
        key: &FactKey,
        position: u64,
    ) -> Result<Header, StorageError> {
        let header = Header::decode(&self.prefix)?;
        // A frame that does not match its own header is corrupt on its own.
        // It is not a later record that only contradicts a dropped group, so
        // it must not join that group's placeholder.
        if key.ordinal() != header.ordinal
            || self.hash.clone().finalize().as_slice() != header.payload
        {
            return Err(StorageError::Corrupt(
                "semantic save envelope payload does not match".into(),
            ));
        }
        let same = self
            .extent
            .as_ref()
            .is_some_and(|extent| extent.group.identity == header.identity);
        let base_matches = header.identity.base == self.published
            || self
                .lineage_base
                .is_some_and(|base| base == header.identity.base);
        // A group written from a dropped completion carries that physical base.
        // Its generation is its own, not the last folded save's next generation.
        let follows_dropped = self
            .lineage_base
            .is_some_and(|base| base == header.identity.base && base != self.published);
        let generation_blocked = if follows_dropped {
            false
        } else {
            self.extent.as_ref().map_or(
                self.published == 0 && header.identity.generation != 0,
                |extent| {
                    extent.group.identity.generation.checked_add(1)
                        != Some(header.identity.generation)
                        || extent.group.identity.stream != header.identity.stream
                        || extent.group.identity.incarnation != header.identity.incarnation
                },
            )
        };
        if !same && (self.is_unfinished() || !base_matches || generation_blocked) {
            return Err(invalid());
        }
        let (count, chain) = if same {
            let extent = self.extent.as_ref().expect("matched extent");
            (extent.group.count, extent.group.chain)
        } else {
            (0, EMPTY_CHAIN)
        };
        if header.ordinal != count || header.previous != chain {
            return Err(invalid());
        }
        match key.kind() {
            FactKind::SaveUnit if self.payload_length != 0 => {
                self.extent = Some(Extent {
                    group: GroupCheckpoint {
                        identity: header.identity.clone(),
                        count: count.checked_add(1).ok_or_else(invalid)?,
                        chain: header.chain(self.payload_length),
                        unit_previous: header.previous,
                        unit_payload: header.payload,
                        unit_length: self.payload_length,
                    },
                    complete: false,
                });
            }
            FactKind::SaveComplete
                if self.payload_length == 0
                    && self.extent.as_ref().is_some_and(|extent| !extent.complete) =>
            {
                let group = self
                    .extent
                    .as_ref()
                    .expect("original unfinished Unit")
                    .group
                    .clone();
                self.extent = Some(Extent {
                    group: group.clone(),
                    complete: true,
                });
                self.published = position;
                self.checkpoint = Some(group);
                self.lineage_base = None;
            }
            _ => return Err(invalid()),
        }
        self.reset_frame();
        Ok(header)
    }
}
fn invalid() -> StorageError {
    StorageError::Corrupt("semantic save envelope disagrees with its lineage".into())
}
