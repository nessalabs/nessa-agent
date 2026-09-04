//! Bundled default shortcut document ([ADR 0004](../../../../docs/adr/0004-server-owned-keybindings.md)).

use crate::protocol::ShortcutsDocument;

const DEFAULTS_JSON: &str = include_str!("../../../../protocol/defaults/shortcuts.v1.json");

/// Parse the repo defaults file. Panics only if the checked-in document is invalid.
pub fn default_shortcuts() -> ShortcutsDocument {
    serde_json::from_str(DEFAULTS_JSON).expect("protocol/defaults/shortcuts.v1.json must parse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ShortcutAction, ShortcutScope};

    #[test]
    fn defaults_include_summon_and_tab_actions() {
        let doc = default_shortcuts();
        assert_eq!(doc.version, 1);
        assert!(doc.bindings.iter().any(|b| {
            b.action == ShortcutAction::PanelSummon && b.scope == ShortcutScope::Global
        }));
        assert!(doc
            .bindings
            .iter()
            .any(|b| b.action == ShortcutAction::PanelNewTab));
        assert!(doc
            .bindings
            .iter()
            .any(|b| b.action == ShortcutAction::PanelActivateTab));
    }
}
