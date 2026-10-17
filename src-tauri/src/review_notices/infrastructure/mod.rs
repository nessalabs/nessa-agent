//! The system notification centers, the commands the window calls, and
//! where opening a notice goes.
//!
//! Unit tests reach no system center. The Linux tests start a private bus
//! with a fake notification service on it.

mod commands;
#[cfg(target_os = "linux")]
mod freedesktop;
mod leave;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod opened;

pub(crate) use commands::{
    __cmd__review_notice_access, __cmd__review_notice_ask, __cmd__review_notice_sync,
    __tauri_command_name_review_notice_access, __tauri_command_name_review_notice_ask,
    __tauri_command_name_review_notice_sync, review_notice_access, review_notice_ask,
    review_notice_sync,
};
#[cfg(target_os = "linux")]
pub(crate) use freedesktop::FreedesktopNotices;
pub(crate) use leave::leave_before_exit;
#[cfg(target_os = "macos")]
pub(crate) use macos::UserNotificationCenter;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) use opened::OpenInDesktopWindow;
