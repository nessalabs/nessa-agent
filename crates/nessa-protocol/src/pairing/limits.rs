use crate::product::generated::MAX_RECORD_RESPONSE_BYTES;
use crate::protocol::MAX_PAYLOAD_BYTES;

/// Largest product frame the gateway reads: the protocol's request bound.
pub const MAX_PROTECTED_REQUEST_BYTES: usize = MAX_PAYLOAD_BYTES as usize;
/// Largest product frame the gateway writes: the generated response bound.
pub const MAX_PROTECTED_RESPONSE_BYTES: usize = MAX_RECORD_RESPONSE_BYTES;
