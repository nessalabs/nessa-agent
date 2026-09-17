//! Private namespace files adapt the desktop signal protocol and durable upgrade audit.
#[cfg(any(target_os = "macos", test))]
mod files;
#[cfg(any(target_os = "macos", test))]
pub(crate) use files::RetirementFiles;
