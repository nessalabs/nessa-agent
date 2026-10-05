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
