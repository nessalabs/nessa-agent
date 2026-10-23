//! One immutable reset owns original source provenance and capture membership.
mod fact;
mod retention;
mod store;

pub(super) use fact::Fact;
pub(super) use retention::{validate_all, validate_owed, validate_schema};
#[cfg(test)]
pub(super) use store::READ;
pub(super) use store::{capture, delivery, insert, read};
