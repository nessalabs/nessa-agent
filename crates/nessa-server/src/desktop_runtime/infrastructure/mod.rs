//! Private namespace files adapt the desktop signal protocol and durable upgrade audit.
#[cfg(any(target_os = "macos", test))]
mod files;
#[cfg(target_os = "macos")]
pub(crate) use files::RetirementFiles;
