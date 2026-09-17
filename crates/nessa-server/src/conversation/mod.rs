//! Product conversations bind authenticated owners to one SDK Agent.
//! `application -> domain` enforces access; `infrastructure -> application` stores metadata.
//! The SDK owns execution, queueing, permissions, and provider cleanup.
pub mod application;
pub mod domain;
pub mod infrastructure;
