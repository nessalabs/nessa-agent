//! Owner catalogue admission, operations, and injected source lifecycle.
//!
//! ```text
//! ReadCatalogue -> AdmitPassiveRead -> CatalogueReadSource
//! ```
//! Arrows are calls. Admission precedes source I/O; the source owns its lease
//! through physical completion and returns it with response evidence.

mod read;
pub use read::{
    CatalogueReadError, CatalogueReadFuture, CatalogueReadOperation, CatalogueReadResponse,
    CatalogueReadSource, CatalogueReadValue, ReadCatalogue,
};

pub(crate) use read::validate_catalogue_selector;
