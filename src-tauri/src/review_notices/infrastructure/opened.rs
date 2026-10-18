//! Opening a notice brings the desktop window forward on the review's
//! conversation.

use tauri::{AppHandle, Emitter};

use crate::desktop_window::{self, DESKTOP_WINDOW};
use crate::host;
use crate::review_notices::application::NoticeOpened;

pub(crate) struct OpenInDesktopWindow {
    app: AppHandle,
}

impl OpenInDesktopWindow {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl NoticeOpened for OpenInDesktopWindow {
    fn opened(&self, conversation: &str) {
        // The system calls back on its own thread, or on the main thread in
        // the middle of its own work. The window is moved from a fresh turn
        // of the main thread either way.
        let app = self.app.clone();
        let conversation = conversation.to_owned();
        let queued = self.app.run_on_main_thread(move || {
            desktop_window::open(&app);
            if let Err(error) =
                app.emit_to(DESKTOP_WINDOW, host::REVIEW_NOTICE_OPENED, conversation)
            {
                crate::host_log::warn(format!("could not open a review from its notice: {error}"));
            }
        });
        if let Err(error) = queued {
            crate::host_log::warn(format!("could not open a review from its notice: {error}"));
        }
    }
}
