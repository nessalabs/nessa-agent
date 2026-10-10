use super::*;

/// A catalogue scope as a peer gateway is answered it, with one part
/// replaced at a time.
fn scope(receiver: &str, stream: &Id, schema: &Id, epoch: &str) -> Scope {
    Scope::new(
        Id::new(receiver).unwrap(),
        Id::new("gateway").unwrap(),
        stream.clone(),
        Id::new("incarnation").unwrap(),
        schema.clone(),
        Id::new(epoch).unwrap(),
    )
}

#[test]
fn a_granted_catalogue_scope_is_checked_for_all_a_non_owner_can_check() {
    let owner = conversation_catalogue_stream(
        &OrganizationId::new("org").unwrap(),
        &PrincipalId::new("someone").unwrap(),
    );
    let upper = Id::new(owner.as_str().to_ascii_uppercase().replacen(
        "CONVERSATION-OWNER:",
        "conversation-owner:",
        1,
    ))
    .unwrap();
    let schema = conversation_catalogue_schema();
    let other_schema = Id::new("nessa.conversation-records.v1").unwrap();
    let conversation = Id::new("a-conversation-id").unwrap();
    for (answered, expected) in [
        (scope("receiver", &owner, &schema, "epoch-3"), Ok(())),
        (
            scope("receiver", &owner, &other_schema, "epoch-3"),
            Err(ReadRefusal::WrongOwner),
        ),
        (
            scope("receiver", &conversation, &schema, "epoch-3"),
            Err(ReadRefusal::WrongOwner),
        ),
        (
            scope("receiver", &upper, &schema, "epoch-3"),
            Err(ReadRefusal::WrongOwner),
        ),
        (
            scope("other", &owner, &schema, "epoch-3"),
            Err(ReadRefusal::WrongReceiver),
        ),
        (
            scope("receiver", &owner, &schema, "epoch-4"),
            Err(ReadRefusal::StaleEpoch),
        ),
    ] {
        assert_eq!(
            check_granted_catalogue_scope("receiver", 3, &answered),
            expected,
            "{answered:?}"
        );
    }
}
