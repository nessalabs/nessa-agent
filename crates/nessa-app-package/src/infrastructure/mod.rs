//! Read external representations and hand their contents to the package validator.

pub(super) mod manifest;

#[cfg(feature = "disposable-store")]
pub(crate) mod installed_store;
