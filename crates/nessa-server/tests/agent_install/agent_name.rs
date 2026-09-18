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
