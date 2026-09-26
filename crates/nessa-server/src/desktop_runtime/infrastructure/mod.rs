//! Private namespace files adapt the desktop signal protocol and durable upgrade audit.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod files;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) use files::RetirementFiles;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod conversation_data;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) use conversation_data::ConversationDirectory;
