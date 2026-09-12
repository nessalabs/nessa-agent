//! The application imports model metadata, answers list/select requests, projects
//! capability snapshots, and admits execution through injected agent ports.
//! It translates boundary DTOs into domain objects and projects results back to
//! DTOs. The domain constructors remain the owners of metadata rules.
//!
//! ```text
//! infrastructure --> input DTOs --> mapping --> domain Catalog
//!                                                |
//! caller <-- output DTOs <-- list / select <------+
//! ```
//! Arrows show data flow. Composition supplies the catalog to ModelCatalog;
//! this layer adds entry context and setup guidance to errors. Agent coordinates
//! input validation and calls an injected AgentSession; infrastructure owns I/O.
//! Execution observations and controls use application-owned types.

pub mod agent_binding;
pub mod dto;
mod mapping;
pub mod model_catalog;
