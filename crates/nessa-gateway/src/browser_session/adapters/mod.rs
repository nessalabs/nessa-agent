//! Private storage retains opaque browser sessions and transition audit.
//! The memory adapter runs the same transition validation for tests.
mod store;
pub use store::{JournalOpenError, MemorySessions, PersistentSessions};
