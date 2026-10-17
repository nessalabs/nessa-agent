//! Taking every notice back as the app leaves, from whichever way it leaves.
//!
//! `RunEvent::Exit` covers a quit. A restart (`AppHandle::restart`, after a
//! refused launch or an installed update) skips that event, so its callers
//! come here first. A crash reaches neither: macOS takes the notices back on
//! the next launch; Linux's service cannot be asked about a previous run's.

use std::time::Duration;

use tauri::AppHandle;

use crate::composition;

/// How long leaving waits for the system to take the notices back.
const LEAVING: Duration = Duration::from_secs(2);

pub(crate) fn leave_before_exit(app: &AppHandle) {
    if let Some(deps) = composition::resolve(app) {
        deps.review_notices.leave(LEAVING);
    }
}
