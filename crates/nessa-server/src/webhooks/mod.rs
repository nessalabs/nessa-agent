//! Outbound delivery of committed conversation records (issue 896).
//!
//! `application -> domain` owns delivery transitions; `infrastructure ->
//! application` signs and posts exact bytes. Authorization is supplied by the
//! conversation/auth owners, never inferred from an endpoint or a MAC key.
pub mod application;
pub mod domain;
pub mod infrastructure;
