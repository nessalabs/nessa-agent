//! On-disk settings.
//!
//! There is no settings UI yet, so the file is the interface: it is written
//! with its defaults on first launch, which is what makes the keys
//! discoverable. A later settings surface reads and writes the same shape.
//! Path is stage-scoped via [`crate::local_data`] (ADR 0005).

use std::fs;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::local_data;

/// `serde(default)` so a file written by an older build — or one a person has
/// hand-edited down to a single key — still loads, with the missing keys
/// filled from the defaults rather than failing the launch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The accelerator that summons and dismisses the panel from anywhere.
    /// `CmdOrCtrl` resolves per platform. Tauri's accelerator syntax, e.g.
    /// "CmdOrCtrl+Shift+A", "Alt+Space".
    pub toggle_shortcut: String,
    pub panel: Panel,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            toggle_shortcut: String::from("CmdOrCtrl+Shift+A"),
            panel: Panel::default(),
        }
    }
}

/// The panel's geometry, in logical pixels. It opens in the lower right of the
/// screen it is summoned on, so these describe the size, not the corner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Panel {
    /// The width the panel opens at on first launch. After that the window's
    /// own width wins, so a drag on the resize edge is not thrown away.
    pub width: f64,
    /// The height the panel opens at. `null` — the default — fills whatever
    /// the work area leaves once the menu bar and the Dock have taken theirs,
    /// and keeps re-filling it as the panel moves between displays.
    pub height: Option<f64>,
    /// How narrow the resize edge may drag the panel.
    pub min_width: f64,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            width: 420.0,
            height: None,
            min_width: 420.0,
        }
    }
}

/// `settings.json` under the stage-scoped config root ([`local_data`]).
fn path(app: &AppHandle) -> Option<std::path::PathBuf> {
    local_data::config_root(app).map(|root| root.join("settings.json"))
}

/// Reads the settings, falling back to the defaults for anything missing — and
/// writes the file when it is absent, so there is something to edit. A
/// malformed file is reported and ignored rather than replaced: overwriting
/// would throw away whatever the person was in the middle of typing.
pub fn load(app: &AppHandle) -> Settings {
    let Some(path) = path(app) else {
        return Settings::default();
    };

    match fs::read_to_string(&path) {
        Ok(raw) => match parse(&raw) {
            Ok(settings) => {
                // Rewrite the merged result so keys added by a later build show
                // up in the file. Values already in it survive, because they
                // were parsed into `settings` first — this fills gaps, it does
                // not reset anything.
                write(&path, &settings);
                settings
            }
            Err(error) => {
                eprintln!("[nessa] {} is not valid settings: {error}", path.display());
                Settings::default()
            }
        },
        Err(_) => {
            let settings = Settings::default();
            write(&path, &settings);
            settings
        }
    }
}

fn parse(raw: &str) -> Result<Settings, serde_json::Error> {
    serde_json::from_str(raw)
}

/// Best-effort: an unwritable config directory costs the file, not the launch.
fn write(path: &std::path::Path, settings: &Settings) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(path, format!("{raw}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_take_their_defaults() {
        let settings = parse("{}").unwrap();
        assert_eq!(settings.toggle_shortcut, Settings::default().toggle_shortcut);
        assert_eq!(settings.panel.width, 420.0);
        assert!(settings.panel.height.is_none());
    }

    #[test]
    fn camel_case_keys_round_trip() {
        let settings = parse(
            r#"{ "toggleShortcut": "Alt+Space", "panel": { "width": 480, "minWidth": 400 } }"#,
        )
        .unwrap();
        assert_eq!(settings.toggle_shortcut, "Alt+Space");
        assert_eq!(settings.panel.width, 480.0);
        assert_eq!(settings.panel.min_width, 400.0);
        assert!(settings.panel.height.is_none());
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_default() {
        assert!(parse("{").is_err());
    }
}
