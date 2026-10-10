//! The contract between the Nessa gateway and its clients.
//!
//! Everything here is what both ends of a gateway connection need to agree on,
//! and nothing that only one end does. The gateway (`nessa-server`) depends on
//! this crate, and so will a device client; this crate depends on neither.
//!
//! ```text
//! nessa-server ──▶ nessa-protocol ◀── device client
//! ```
//! Arrows are compile-time dependencies.
//!
//! - `protocol`: wire frames and the payloads generated from the protocol schemas.
//! - `product_contract`: the product contract's pure outcome values.
//! - `product`: the generated product DTOs, the handshake rules, and the read codecs.
//! - `pairing`: native pairing framing, envelope codec, enrollment channel and socket.
//! - `conversation`: the conversation read model, and `agents`, the agent names it uses.
//! - `lease`: the lease frames a gateway and an environment (`nessa env serve`) exchange.
//! - `clock`: the monotonic clock port the pairing socket's deadlines read.
//!
//! Admission rule: no process state and no runtime of its own. `pairing::socket`
//! is the one deliberate exception, blocking std socket mechanics both ends
//! need; `tokio` is taken with `rt` only, for the one shared worker-fault mapping.
pub mod agents;
pub mod clock;
pub mod conversation;
pub mod lease;
pub mod pairing;
pub mod product;
pub mod product_contract;
pub mod protocol;
