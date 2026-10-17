//! Notices for permission reviews that are waiting on the person (#806).
//!
//! The gateway pushes the set of reviews still waiting to the desktop window
//! (`permission.reviewWaiting`). The window hands each set to the host whole,
//! and the host, which alone knows which notices are up, posts what arrived
//! and takes back what left. The webview cannot do this itself: WKWebView has
//! no web notifications at all, and WebKitGTK denies the permission when the
//! app does not answer for it.
//!
//! ```text
//! gateway ──set──▶ window ──review_notice_sync──▶ ReviewNotices ──▶ NoticeCenter
//!                    ▲                                                 │ click
//!                    └────── nessa://review-notice-opened ◀───── NoticeOpened
//! ```
//!
//! macOS posts through the User Notifications framework when run from the app
//! bundle, Linux through the session's notification service. Any other host
//! answers `unsupported` and keeps the webview's own notifications. Notices
//! are taken back when the app starts, quits or restarts (`leave`).

pub(crate) mod application;
pub(crate) mod domain;
pub(crate) mod infrastructure;
