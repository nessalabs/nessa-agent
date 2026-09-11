//! The application imports model metadata and answers list/select requests.
//! It translates boundary DTOs into domain objects and projects results back to
//! DTOs. The domain constructors remain the owners of metadata rules.
//!
//! ```text
//! infrastructure --> input DTOs --> mapping --> domain Catalog
//!                                                |
//! caller <-- output DTOs <-- list / select <------+
//! ```
//! Arrows show data flow. Composition supplies the catalog to ModelCatalog;
//! this layer adds entry context and setup guidance to errors without doing I/O.

pub mod dto;
mod mapping;
pub mod model_catalog;
