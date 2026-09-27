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
//! ```text
//! caller -> agent_execution::agents::Agent -> injected provider
//! backend -> agent_execution::executions -> domain session
//!                        |
//!                        +-> permissions evidence + tools review input
//! ```
//! Arrows show data flow and calls. Composition supplies the catalog to ModelCatalog;
//! this layer coordinates invocation, session snapshots, and provider calls.
//! Infrastructure owns storage and provider I/O.
//! Execution observations and controls use application-owned types.

pub mod agent_execution;
pub mod dto;
mod mapping;
pub mod model_catalog;
