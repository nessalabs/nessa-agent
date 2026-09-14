//! Maps permission offers and serializes selected or cancelled responses.
//!
//! ```text
//! ACP offer -> domain choices; domain resolution -> ACP response
//! ```
//! Arrows show translation. Identity and lifecycle decisions stay in the domain.
pub(crate) mod wire;
