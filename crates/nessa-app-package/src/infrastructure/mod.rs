//! Read external representations and hand their contents to the package validator.

pub(super) mod inline;
pub(super) mod launcher;
pub(super) mod manifest;
pub(super) mod server;

#[cfg(feature = "disposable-store")]
pub(crate) mod installed_store;
