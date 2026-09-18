use super::*;

#[test]
fn a_plain_name_is_read_as_written() {
    let name = AgentName::parse("opencode").expect("a plain name is an agent name");
    assert_eq!(name.as_str(), "opencode");
    assert_eq!(name.to_string(), "opencode");
}

#[test]
fn digits_and_hyphens_belong_in_a_name() {
    assert!(AgentName::parse("open-code-2").is_ok());
}

#[test]
fn a_name_that_could_leave_its_directory_is_refused() {
    // The one thing this type exists for: the name becomes a directory under
    // Nessa's own data directory.
    for escape in ["", ".", "..", "/etc", "a/b", "a\\b", ".hidden", "a\0b"] {
        assert!(
            AgentName::parse(escape).is_err(),
            "{escape:?} was accepted as an agent name"
        );
    }
}

#[test]
fn a_name_is_not_normalised_into_one() {
    // Accepting a spelling and quietly changing it would mean two names for one
    // agent, and a person who typed the second would be told it is installed
    // somewhere they cannot find.
    for spelling in ["Opencode", "OPENCODE", "open code", "open_code"] {
        assert!(
            AgentName::parse(spelling).is_err(),
            "{spelling:?} was accepted as an agent name"
        );
    }
}

#[test]
fn a_name_longer_than_a_path_component_is_not_a_name() {
    // The name becomes one directory. Without this the filesystem refuses it
    // instead, and a name fault arrives as a machine that could not write.
    assert!(AgentName::parse(&"a".repeat(64)).is_ok());
    assert!(AgentName::parse(&"a".repeat(65)).is_err());
}

#[test]
fn a_refusal_says_what_was_offered_and_what_is_allowed() {
    let refusal = AgentName::parse("Opencode").expect_err("uppercase is not a name");
    assert_eq!(refusal.offered(), "Opencode");
    assert!(
        refusal
            .to_string()
            .contains("lowercase letters, digits and hyphens"),
        "unhelpful message: {refusal}"
    );
}

#[test]
fn a_name_windows_answers_to_as_a_device_is_not_an_agent_name() {
    // A name becomes a directory under Nessa's own data directory, and `con` or
    // `nul` there is not a directory on Windows at all. The same rule the
    // version has, for the same reason, so that one of the two cannot drift.
    for reserved in ["con", "nul", "aux", "com1", "lpt9", "nul.txt"] {
        assert_eq!(
            AgentName::parse(reserved),
            Err(NotAnAgentName(reserved.to_string())),
            "{reserved:?} cannot be a directory name everywhere"
        );
    }
    // Neighbours that merely look like one.
    for ordinary in ["console", "com10", "nullify"] {
        assert!(AgentName::parse(ordinary).is_ok(), "{ordinary:?} is a name");
    }
    // The message is the only thing a person gets, and `con` satisfies the
    // alphabet — so a message that names only the alphabet contradicts itself.
    let message = AgentName::parse("con")
        .expect_err("a device is not a name")
        .to_string();
    assert!(
        message.contains("device"),
        "a refused device name should say so: {message}"
    );
}
