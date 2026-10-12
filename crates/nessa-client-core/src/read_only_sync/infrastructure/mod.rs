//! Private database and bounded gateway adapters implement consuming ports.
//! The cache is one transaction owner; SDK/core validation is called directly.
//! `held_file` is the part file a fetched held file is assembled in.

pub(crate) mod cache;
pub(crate) mod gateway;
pub(crate) mod held_file;
