//! One immutable reset owns original source provenance and capture membership.
mod fact;
mod retention;
mod store;

pub(super) use fact::Fact;
pub(super) use retention::{validate_all, validate_owed};
pub(super) use store::{capture, delivery, observe, read};
#[cfg(test)]
pub(super) use store::{insert, READ};
