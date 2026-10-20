//! Private database and bounded gateway adapters implement consuming ports.
//! The cache is one transaction owner; SDK/core validation is called directly.
//! `held_file` is the part file a fetched held file is assembled in.
//! `measure` runs the weak-link harness against a real record source and this cache.

pub(crate) mod cache;
pub(crate) mod gateway;
pub(crate) mod held_file;
pub(crate) mod measure;
