//! Developer diagnostics carried from a page to the terminal that started it.
//!
//! A webview owns its console, so its warnings and errors otherwise disappear
//! unless developer tools happen to be open. The page keeps writing to that
//! console and, in a debug build, mirrors those two levels through this plugin.
//! Release builds register no command and receive no page diagnostics.

use tauri::{plugin::TauriPlugin, Runtime};

#[cfg(debug_assertions)]
use serde::Deserialize;

#[cfg(debug_assertions)]
const MOST_MESSAGE_BYTES: usize = 16 * 1024;
#[cfg(debug_assertions)]
const MOST_SOURCE_BYTES: usize = 8 * 1024;

#[cfg(debug_assertions)]
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConsoleLevel {
    Warn,
    Error,
}

#[cfg(debug_assertions)]
impl ConsoleLevel {
    fn label(&self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsoleEntry {
    level: ConsoleLevel,
    message: String,
    source: String,
}

#[cfg(debug_assertions)]
#[tauri::command]
fn forward_webview_console(entry: ConsoleEntry) {
    let message = bounded(&entry.message, MOST_MESSAGE_BYTES);
    let source = bounded(&entry.source, MOST_SOURCE_BYTES);
    eprintln!(
        "[nessa:webview:{}] {message}\n{source}",
        entry.level.label()
    );
}

/// Installs the developer-only page diagnostic command.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let builder = tauri::plugin::Builder::new("dev-console");
    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(tauri::generate_handler![forward_webview_console]);
    builder.build()
}

#[cfg(debug_assertions)]
fn bounded(value: &str, most_bytes: usize) -> &str {
    if value.len() <= most_bytes {
        return value;
    }
    let mut end = most_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::bounded;

    #[test]
    fn terminal_fields_are_bounded_at_a_character_boundary() {
        assert_eq!(bounded("one 🐇 two", 6), "one ");
        assert_eq!(bounded("short", 6), "short");
    }
}
