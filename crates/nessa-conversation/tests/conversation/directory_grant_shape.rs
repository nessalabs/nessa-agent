use super::*;

#[test]
fn a_program_is_a_file_name() {
    assert!(CommandMatcher::new(RuleEffect::Allow, "rm", None).is_ok());
    assert!(CommandMatcher::new(RuleEffect::Allow, "rm/rf", None).is_err());
    assert!(ArgvPattern::new("-*").unwrap().matches("-rf"));
    assert!(!ArgvPattern::new("-*").unwrap().matches("file"));
}

#[test]
fn an_encoded_grant_past_the_frame_is_refused() {
    let path = "x".repeat(MAX_ENCODED_GRANT_BYTES);
    let error = encoded_grant(
        "local",
        &path,
        DirectoryCapabilities::from_names(["read"]).unwrap(),
        &CommandRules::none(),
    );
    assert!(matches!(error, Err(DirectoryShapeError::Frame)));
}
