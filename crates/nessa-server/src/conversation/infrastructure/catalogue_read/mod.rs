//! Current owner catalogue adapter over metadata and shared tracked workers.
//!
//! ```text
//! NessaCatalogueReadSource -> trusted_scope -> ReadWorkers -> operation
//!                                                        -> NessaCatalogueSource
//! ```
//! Arrows are calls. Trusted identity checks precede I/O. Worker/source drops
//! precede lease release; shared drain finishes before composition tears down storage.

mod operation;
mod source;
pub use source::NessaCatalogueReadSource;
