//! Private database and bounded gateway adapters implement consuming ports.
//! The cache is one transaction owner; SDK/core validation is called directly.

pub(crate) mod cache;
pub(crate) mod gateway;
