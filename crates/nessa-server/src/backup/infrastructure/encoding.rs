//! Percent-decoding at the persisted backup representation boundary.
//!
//! The alphabet lives in the domain. This module is the name the manifest
//! and inventory decoders already call.

pub(super) fn decode(value: &str) -> Result<String, ()> {
    decode_bounded(value, usize::MAX)
}

pub(super) fn decode_bounded(value: &str, max_bytes: usize) -> Result<String, ()> {
    crate::backup::domain::decode_identity(value, max_bytes)
}
