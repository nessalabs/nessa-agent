//! The whole checked-in bundle: every policy of every set has its own `@id`,
//! and the ids are the ones the ports' callers and logs name.
use super::bundle::{Bundle, POLICY_FILES, SCHEMA};

#[test]
fn every_checked_in_policy_has_a_unique_id() {
    let bundle = Bundle::load().expect("the checked-in bundle validates");
    let mut ids: Vec<String> = bundle
        .policies()
        .policies()
        .map(|policy| policy.id().to_string())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "app.other_server",
            "app.other_server_tool",
            "app.own_server",
            "app.visible_tool",
            "command.agent_host",
            "command.denied_program",
            "command.grant_host",
            "command.owner_host",
            "command.unlisted_program",
            "tool.edits",
            "tool.owner",
            "tool.preset.all",
            "tool.preset.ask_everything",
            "tool.reads",
        ]
    );
    // A file listed twice repeats every id it holds, and is refused.
    for file in POLICY_FILES {
        assert!(
            Bundle::from_sources(SCHEMA, &[file, file]).is_err(),
            "{file}"
        );
    }
}
