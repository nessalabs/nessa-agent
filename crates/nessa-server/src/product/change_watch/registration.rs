use crate::conversation::{
    application::{
        AdmitPassiveRead, CatalogueChangeWatch, CatalogueReadScope, CatalogueWatchError,
        CatalogueWatchState, ReadRefusal,
    },
    domain::ConversationId,
};
use crate::product::{
    generated::{
        product_method, ConversationWatchCatalogueParams, ConversationWatchRecordsParams,
        MAX_CHANGE_WATCH_ID_BYTES,
    },
    state::ProductRouteState,
};
use crate::product_contract::generated::{ChangeWatchEndReason, ChangeWatchErrorCode};
use nessa_auth::application::{authorization::AuthorizeAction, session::AuthenticatedSession};
use nessa_sdk::application::agent_execution::sessions::{
    ChangeWatchError, ChangeWatchState, CommittedChangeWatch,
};
use nessa_sync::replication::domain::Id;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// One current connection's monotonic namespace, never a source or access identity.
pub(in crate::product) struct WatchToken {
    namespace: Uuid,
    accepted: u64,
}

impl WatchToken {
    pub fn new(namespace: Uuid) -> Self {
        Self {
            namespace,
            accepted: 0,
        }
    }

    pub fn can_accept(&self, count: usize) -> bool {
        u64::try_from(count)
            .ok()
            .and_then(|count| self.accepted.checked_add(count))
            .is_some()
    }

    pub fn next(&self) -> Result<String, ChangeWatchErrorCode> {
        let next = self
            .accepted
            .checked_add(1)
            .ok_or(ChangeWatchErrorCode::WatchCapacity)?;
        Ok(format!("{}-{next}", self.namespace))
    }

    // Advance only after actual producer installation. No failed-registration gaps.
    pub fn accepted(&mut self) {
        self.accepted += 1;
    }

    pub fn owns(&self, id: &str) -> bool {
        if id.len() > MAX_CHANGE_WATCH_ID_BYTES {
            return false;
        }
        let Some((namespace, counter)) = id.rsplit_once('-') else {
            return false;
        };
        let Ok(counter) = counter.parse::<u64>() else {
            return false;
        };
        counter > 0
            && counter <= self.accepted
            && id == format!("{}-{counter}", self.namespace)
            && namespace == self.namespace.to_string()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::product) enum WatchSelector {
    Records {
        conversation: ConversationId,
        receiver: String,
        epoch: u64,
    },
    Catalogue {
        receiver: String,
        epoch: u64,
    },
}

impl WatchSelector {
    pub fn decode(method: &str, params: Value) -> Result<Self, ChangeWatchErrorCode> {
        let invalid = || ChangeWatchErrorCode::InvalidRequest;
        if method == product_method::CONVERSATION_WATCH_RECORDS {
            let value: ConversationWatchRecordsParams =
                serde_json::from_value(params).map_err(|_| invalid())?;
            Ok(Self::Records {
                conversation: ConversationId::new(&value.conversation_id).map_err(|_| invalid())?,
                receiver: Id::new(value.receiver_id)
                    .map_err(|_| invalid())?
                    .as_str()
                    .to_owned(),
                epoch: super::super::passive_read::wire::decode_epoch(&value.access_epoch)
                    .map_err(|_| invalid())?,
            })
        } else if method == product_method::CONVERSATION_WATCH_CATALOGUE {
            let value: ConversationWatchCatalogueParams =
                serde_json::from_value(params).map_err(|_| invalid())?;
            Ok(Self::Catalogue {
                receiver: Id::new(value.receiver_id)
                    .map_err(|_| invalid())?
                    .as_str()
                    .to_owned(),
                epoch: super::super::passive_read::wire::decode_epoch(&value.access_epoch)
                    .map_err(|_| invalid())?,
            })
        } else {
            Err(invalid())
        }
    }

    pub fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Records { .. }, Self::Records { .. })
                | (Self::Catalogue { .. }, Self::Catalogue { .. })
        )
    }

    /// Current authority for this selector: credential, policy, receiver binding,
    /// target ownership and browser presence, through the existing passive owner.
    /// Registration and every notice ask this same function.
    async fn admit(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<Admitted, ChangeWatchErrorCode> {
        let current = super::super::socket::watch_identity(state, session)
            .await
            .map_err(|error| admission_code(ReadRefusal::from(error)))?;
        let session = &current;
        let (receivers, conversations) = state
            .passive_read
            .as_ref()
            .ok_or(ChangeWatchErrorCode::TemporarilyUnavailable)?;
        let admission = AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: state.access.as_ref(),
                clock: state.clock.as_ref(),
                policy: state.policy.as_ref(),
            },
            gateway: &state.gateway,
            receivers: receivers.as_ref(),
            conversations: conversations.as_ref(),
        };
        let admitted = match self {
            Self::Records {
                conversation,
                receiver,
                epoch,
            } => {
                admission
                    .execute(session, conversation, receiver, *epoch)
                    .await
                    .map_err(admission_code)?;
                Admitted::Records(conversation.clone())
            }
            Self::Catalogue { receiver, epoch } => Admitted::Catalogue(
                admission
                    .catalogue(session, receiver, *epoch)
                    .await
                    .map_err(admission_code)?,
            ),
        };
        super::super::socket::watch_browser_present(state, session)
            .await
            .map_err(|error| admission_code(ReadRefusal::from(error)))?;
        Ok(admitted)
    }

    pub async fn authorize(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<(), ChangeWatchErrorCode> {
        self.admit(state, session).await.map(|_| ())
    }

    pub async fn install(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        interest: &AtomicBool,
    ) -> Result<WatchHandle, ChangeWatchErrorCode> {
        // Reuse the actual current snapshot owner without a head/read lease.
        let admitted = self.admit(state, session).await?;
        if !interest.load(Ordering::Acquire) {
            return Err(ChangeWatchErrorCode::WatchClosed);
        }
        match admitted {
            Admitted::Records(conversation) => state
                .record_watches
                .as_ref()
                .ok_or(ChangeWatchErrorCode::TemporarilyUnavailable)?
                .watch(&conversation)
                .map(WatchHandle::Records)
                .map_err(|error| match error {
                    ChangeWatchError::Capacity => ChangeWatchErrorCode::WatchCapacity,
                    ChangeWatchError::Closed => ChangeWatchErrorCode::WatchClosed,
                }),
            Admitted::Catalogue(admitted) => state
                .catalogue_watches
                .as_ref()
                .ok_or(ChangeWatchErrorCode::TemporarilyUnavailable)?
                .watch(&admitted.organization_id, &admitted.owner_id)
                .map(WatchHandle::Catalogue)
                .map_err(|error| match error {
                    CatalogueWatchError::Capacity => ChangeWatchErrorCode::WatchCapacity,
                }),
        }
    }
}

/// What current admission established for one selector: the stable record
/// target, or the catalogue owner the receiver binding currently names.
enum Admitted {
    Records(ConversationId),
    Catalogue(CatalogueReadScope),
}

pub(in crate::product) enum WatchHandle {
    Records(CommittedChangeWatch),
    Catalogue(CatalogueChangeWatch),
}
impl WatchHandle {
    pub async fn changed(&mut self) -> Option<ChangeWatchEndReason> {
        match self {
            Self::Records(handle) => match handle.changed().await {
                ChangeWatchState::Dirty => None,
                ChangeWatchState::Closed => Some(ChangeWatchEndReason::Closed),
                ChangeWatchState::NotificationFailed => {
                    Some(ChangeWatchEndReason::NotificationFailed)
                }
            },
            Self::Catalogue(handle) => match handle.changed().await {
                CatalogueWatchState::Dirty => None,
                CatalogueWatchState::Closed => Some(ChangeWatchEndReason::Closed),
                CatalogueWatchState::NotificationFailed => {
                    Some(ChangeWatchEndReason::NotificationFailed)
                }
            },
        }
    }
}
fn admission_code(error: ReadRefusal) -> ChangeWatchErrorCode {
    match error {
        ReadRefusal::InvalidRequest => ChangeWatchErrorCode::InvalidRequest,
        ReadRefusal::Unauthorized => ChangeWatchErrorCode::Unauthorized,
        ReadRefusal::Forbidden => ChangeWatchErrorCode::Forbidden,
        ReadRefusal::WrongOwner => ChangeWatchErrorCode::WrongOwner,
        ReadRefusal::WrongReceiver => ChangeWatchErrorCode::WrongReceiver,
        ReadRefusal::StaleEpoch => ChangeWatchErrorCode::StaleEpoch,
        ReadRefusal::Unverifiable => ChangeWatchErrorCode::Unverifiable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontier_recognizes_exact_removed_ids_without_foreign_or_unminted_ids() {
        let mut tokens = WatchToken::new(Uuid::from_u128(1));
        let first = tokens.next().unwrap();
        assert!(!tokens.owns(&first));
        tokens.accepted();
        assert!(tokens.owns(&first));
        assert!(!tokens.owns(&tokens.next().unwrap()));
        assert!(!WatchToken::new(Uuid::from_u128(2)).owns(&first));
        assert!(!tokens.owns(&format!("{}-01", Uuid::from_u128(1))));
        assert!(!tokens.owns(&format!("{}-0", Uuid::from_u128(1))));
        tokens.accepted = u64::MAX - 1;
        assert!(tokens.can_accept(0));
        assert!(tokens.can_accept(1));
        assert!(!tokens.can_accept(2));
        assert_eq!(
            tokens.next().unwrap(),
            format!("{}-{}", Uuid::from_u128(1), u64::MAX)
        );
        tokens.accepted();
        assert!(tokens.can_accept(0));
        assert!(!tokens.can_accept(1));
        assert_eq!(tokens.next(), Err(ChangeWatchErrorCode::WatchCapacity));
        assert!(tokens.owns(&format!("{}-{}", Uuid::from_u128(1), u64::MAX)));
    }
}
