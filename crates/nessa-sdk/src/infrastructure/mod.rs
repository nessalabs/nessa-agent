//! Infrastructure parses model metadata and implements session storage ports.
//!
//! ```text
//! model JSON -> application catalog -> domain
//! SessionManager -> session_storage -> leased memory / private files
//! ```
//! Arrows show data translation and calls. Composition injects these adapters.
pub mod model_metadata_json;
pub mod session_storage;
