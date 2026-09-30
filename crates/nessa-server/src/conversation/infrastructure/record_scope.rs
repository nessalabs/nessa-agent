//! Translate an admitted receiver into the exact SDK record source scope.

use crate::conversation::application::{
    passive_read_selector, validate_record_selector, ReadRefusal, ReceiverReadScope,
};
use nessa_sdk::infrastructure::session_storage::{NessaRecordSource, RecordStreamIdentity};
use nessa_sync::replication::domain::{Id, Scope};

/// The SDK source owns origin, stream, incarnation and schema. The gateway
/// contributes only its verified receiver and durable epoch, then compares the
/// complete requested scope before invoking `head` or `page`.
pub fn exact_record_scope(
    admitted: &ReceiverReadScope,
    source: &NessaRecordSource,
    requested: &Scope,
) -> Result<Scope, ReadRefusal> {
    exact_scope_with(admitted, requested, |receiver, epoch| {
        source.scope(receiver, epoch)
    })
}

pub fn record_scope_from_identity(
    admitted: &ReceiverReadScope,
    identity: &RecordStreamIdentity,
) -> Result<Scope, ReadRefusal> {
    let (receiver, epoch) = passive_read_selector(&admitted.receiver_id, admitted.access_epoch)?;
    let scope = identity.scope(receiver, epoch);
    validate_record_selector(admitted, &scope)?;
    Ok(scope)
}

fn exact_scope_with(
    admitted: &ReceiverReadScope,
    requested: &Scope,
    scope: impl FnOnce(Id, Id) -> Scope,
) -> Result<Scope, ReadRefusal> {
    let (receiver, epoch) = passive_read_selector(&admitted.receiver_id, admitted.access_epoch)?;
    let exact = scope(receiver, epoch);
    compare_record_scope(admitted, exact, requested)
}

fn compare_record_scope(
    admitted: &ReceiverReadScope,
    exact: Scope,
    requested: &Scope,
) -> Result<Scope, ReadRefusal> {
    validate_record_selector(admitted, &exact)?;
    validate_record_selector(admitted, requested)?;
    if &exact != requested {
        return Err(ReadRefusal::Unverifiable);
    }
    Ok(exact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::domain::ConversationId;
    use nessa_auth::domain::{OrganizationId, PrincipalId};

    #[test]
    fn exact_record_tuple_is_compared_before_source_read() {
        let conversation_id = ConversationId::new("8e024fc9-0c9d-4952-8427-bcb2b3b07f8f").unwrap();
        let admitted = ReceiverReadScope {
            receiver_id: "receiver".into(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("owner").unwrap(),
            conversation_id,
            access_epoch: 3,
        };
        let id = |value: &str| Id::new(value).unwrap();
        let exact = Scope::new(
            id("receiver"),
            id("origin"),
            id(&admitted.conversation_id.to_string()),
            id("incarnation"),
            id("nessa.physical-frame.v1"),
            id("epoch-3"),
        );
        assert_eq!(
            compare_record_scope(&admitted, exact.clone(), &exact),
            Ok(exact.clone())
        );
        let wrong_receiver = Scope::new(
            id("wrong"),
            id("origin"),
            id(&admitted.conversation_id.to_string()),
            id("incarnation"),
            id("nessa.physical-frame.v1"),
            id("epoch-3"),
        );
        assert_eq!(
            compare_record_scope(&admitted, exact.clone(), &wrong_receiver),
            Err(ReadRefusal::WrongReceiver)
        );
        let stale = Scope::new(
            id("receiver"),
            id("origin"),
            id(&admitted.conversation_id.to_string()),
            id("incarnation"),
            id("nessa.physical-frame.v1"),
            id("epoch-2"),
        );
        assert_eq!(
            compare_record_scope(&admitted, exact.clone(), &stale),
            Err(ReadRefusal::StaleEpoch)
        );
        let wrong_origin = Scope::new(
            id("receiver"),
            id("wrong-origin"),
            id(&admitted.conversation_id.to_string()),
            id("incarnation"),
            id("nessa.physical-frame.v1"),
            id("epoch-3"),
        );
        assert_eq!(
            compare_record_scope(&admitted, exact.clone(), &wrong_origin),
            Err(ReadRefusal::Unverifiable)
        );
        let wrong_stream = Scope::new(
            id("receiver"),
            id("origin"),
            id("another"),
            id("incarnation"),
            id("nessa.physical-frame.v1"),
            id("epoch-3"),
        );
        assert_eq!(
            compare_record_scope(&admitted, exact, &wrong_stream),
            Err(ReadRefusal::WrongOwner)
        );
    }
}
