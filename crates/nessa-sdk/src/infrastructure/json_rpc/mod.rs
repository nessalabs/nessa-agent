//! Bounded JSON-RPC envelopes and newline stdio transport shared by ACP providers.
//! The event-stream codec frames bytes; this module parses JSON-RPC and preserves
//! request correlation. ACP methods and provider schemas live in their adapters.
//!
//! bytes --> event-stream NewlineFramer --> JSON-RPC envelope --> ACP runtime
//!
//! Arrows show decoding. Outbound `encode` validates exact serialized bytes before
//! ACP admission; `send_encoded` writes that same frame after ownership transfers.
//! Raw frames are not appended to a durable event history. Transport regressions
//! live in `tests/infrastructure/json_rpc/transport.rs`, included as private tests.
//! Parsing charges each value and key against one frame item budget before
//! allocation; `decoding_budget.rs` tests exact limits and ignored metadata.
mod envelope;
mod transport;
pub(crate) use envelope::{notification, parse, request, success, unsupported, Envelope, RpcId};
pub(crate) use transport::{encode, large_frame_allowance, send_encoded, write_allowance, Reader};
mod error;
pub(crate) use error::protocol;
