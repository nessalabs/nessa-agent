use super::*;

#[test]
fn owner_identity_is_stable_distinct_and_length_delimited() {
    let stream = |org: &str, owner: &str| {
        conversation_catalogue_stream(
            &OrganizationId::new(org).unwrap(),
            &PrincipalId::new(owner).unwrap(),
        )
    };
    assert_eq!(stream("org", "owner"), stream("org", "owner"));
    assert_ne!(stream("org", "owner"), stream("org", "other"));
    assert_ne!(stream("org", "owner"), stream("other", "owner"));
    assert_ne!(stream("ab", "c"), stream("a", "bc"));
}

#[test]
fn every_owner_stream_has_the_shape_a_non_owner_reader_checks() {
    let stream = conversation_catalogue_stream(
        &OrganizationId::new("org").unwrap(),
        &PrincipalId::new("owner").unwrap(),
    );
    assert!(is_conversation_catalogue_stream(&stream));
    for other in [
        "conversation-owner:",
        "conversation-owner:abc",
        "conversation-other:0000000000000000000000000000000000000000000000000000000000000000",
        "conversation-owner:000000000000000000000000000000000000000000000000000000000000000G",
        "a-conversation-id",
    ] {
        assert!(
            !is_conversation_catalogue_stream(&Id::new(other).unwrap()),
            "{other}"
        );
    }
}
