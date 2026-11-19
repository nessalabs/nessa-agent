//! The trackpad's haptic tick, through `NSHapticFeedbackManager`.
//!
//! Unverified by any test: whether the tick is felt. AppKit gives no way to
//! read it back; it is checked by hand on a Force Touch trackpad.

use objc2_app_kit::{
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer,
};
use tauri::WebviewWindow;

/// One "level change" tick: the pattern AppKit gives a control that moved by
/// one discrete step, which is what a swipe that folds one column is. AppKit
/// is asked on the main thread, as a command runs off it.
pub(super) fn level_change(window: &WebviewWindow) -> Result<(), String> {
    window
        .run_on_main_thread(|| {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::LevelChange,
                NSHapticFeedbackPerformanceTime::Now,
            );
        })
        .map_err(|error| error.to_string())
}
