//! The trackpad's haptic tick, through `NSHapticFeedbackManager`.
//!
//! Unverified by any test: whether the tick is felt. AppKit gives no way to
//! read it back; it is checked by hand on a Force Touch trackpad.

use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer,
};
use tauri::WebviewWindow;

/// One "generic" tick. Not "level change": AppKit ties that pattern to the
/// pressure levels of a force click, and a swipe presses nothing. AppKit is
/// asked on the main thread, as a command runs off it.
pub(super) fn tick(window: &WebviewWindow) -> Result<(), String> {
    window
        .run_on_main_thread(|| {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::Generic,
                NSHapticFeedbackPerformanceTime::Now,
            );
        })
        .map_err(|error| error.to_string())
}
