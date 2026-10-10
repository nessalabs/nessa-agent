use super::super::passive_read::PUBLISHED_PASSIVE_READ_GRANTS;
use crate::conversation::application::{
    access_refusal, AdmitPassiveRead, CatalogueChangeWatch, CatalogueWatchError,
    CatalogueWatchState, PassiveRead,
};
use crate::product::socket::close_reason;
use crate::product::state::ProductRouteState;
use nessa_auth::application::ports::AccessError;
use nessa_auth::application::{authorization::AuthorizeAction, session::AuthenticatedSession};
use nessa_protocol::conversation::{
    domain::ConversationId,
    read_scope::{CatalogueReadScope, ReadRefusal},
};
use nessa_protocol::product::generated::{
    product_method, ConversationWatchCatalogueParams, ConversationWatchRecordsParams,
    MAX_CHANGE_WATCH_ID_BYTES, MAX_CONNECTION_CATALOGUE_WATCHES, MAX_CONNECTION_RECORD_WATCHES,
};
use nessa_protocol::product::passive_read::decode_epoch;
use nessa_protocol::product_contract::generated::{
    ChangeWatchEndReason, ChangeWatchErrorCode, SessionCloseReason,
};
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
                epoch: decode_epoch(&value.access_epoch).map_err(|_| invalid())?,
            })
        } else if method == product_method::CONVERSATION_WATCH_CATALOGUE {
            let value: ConversationWatchCatalogueParams =
                serde_json::from_value(params).map_err(|_| invalid())?;
            Ok(Self::Catalogue {
                receiver: Id::new(value.receiver_id)
                    .map_err(|_| invalid())?
                    .as_str()
                    .to_owned(),
                epoch: decode_epoch(&value.access_epoch).map_err(|_| invalid())?,
            })
        } else {
            Err(invalid())
        }
    }

    /// How many targets of this kind one connection may hold, as published by
    /// the product schema.
    pub fn connection_limit(&self) -> usize {
        match self {
            Self::Records { .. } => MAX_CONNECTION_RECORD_WATCHES,
            Self::Catalogue { .. } => MAX_CONNECTION_CATALOGUE_WATCHES,
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
    ) -> Result<Admitted, WatchRefusal> {
        let current = super::super::socket::watch_identity(state, session)
            .await
            .map_err(WatchRefusal::Access)?;
        let admitted = self.admit_current(state, &current).await?;
        super::super::socket::watch_browser_present(state, &current)
            .await
            .map_err(WatchRefusal::Access)?;
        Ok(admitted)
    }

    /// Passive-read admission alone, for a session whose identity and browser
    /// presence the caller has just checked.
    async fn admit_current(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<Admitted, WatchRefusal> {
        let (receivers, conversations, read_grants) = state
            .passive_read
            .as_ref()
            .ok_or(WatchRefusal::Unavailable)?;
        // Watch methods publish no grant of their own. A records watch is
        // admitted as a record head, and a catalogue watch as a catalogue head:
        // the same read the watch follows.
        let admission = AdmitPassiveRead {
            authorization: AuthorizeAction {
                access: state.access.as_ref(),
                clock: state.clock.as_ref(),
                policy: state.policy.as_ref(),
            },
            gateway: &state.gateway,
            receivers: receivers.as_ref(),
            conversations: conversations.as_ref(),
            grants: &PUBLISHED_PASSIVE_READ_GRANTS,
            read_grants: read_grants.as_ref(),
        };
        let admitted = match self {
            Self::Records {
                conversation,
                receiver,
                epoch,
            } => {
                admission
                    .execute(
                        session,
                        conversation,
                        receiver,
                        *epoch,
                        PassiveRead::RecordHead,
                    )
                    .await
                    .map_err(WatchRefusal::Read)?;
                Admitted::Records(conversation.clone())
            }
            Self::Catalogue { receiver, epoch } => {
                let admitted = admission
                    .catalogue(session, receiver, *epoch, PassiveRead::CatalogueHead)
                    .await
                    .map_err(WatchRefusal::Read)?;
                // A catalogue watch is the owner's: its notices fire on every
                // change to the owner's catalogue, granted or not. A paired
                // device signs in as that owner; a peer gateway is another
                // party, and the timing of the owner's ungranted work is not
                // its to see, so it is refused the watch and polls its head,
                // which moves only with what it was granted
                // (`docs/design/auth/peer-gateways.md`).
                if admitted.owner_id != *session.context().principal_id() {
                    return Err(WatchRefusal::Read(ReadRefusal::Forbidden));
                }
                Admitted::Catalogue(admitted)
            }
        };
        Ok(admitted)
    }

    /// The connection's periodic re-check of its live watches (row A3). The
    /// connection's own refresh has just confirmed `current` (identity and
    /// browser presence) and passes it in, so this asks only passive-read
    /// admission, once per distinct target, one after another. Returns one
    /// result per entry of `selectors`, in order.
    pub async fn recheck(
        selectors: &[Self],
        state: &ProductRouteState,
        current: &AuthenticatedSession,
    ) -> Vec<Result<(), WatchRefusal>> {
        let mut results: Vec<Result<(), WatchRefusal>> = Vec::with_capacity(selectors.len());
        for (index, selector) in selectors.iter().enumerate() {
            let earlier = selectors[..index]
                .iter()
                .position(|other| other == selector);
            let result = match earlier {
                Some(earlier) => results[earlier],
                None => selector.admit_current(state, current).await.map(|_| ()),
            };
            results.push(result);
        }
        results
    }

    pub async fn authorize(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Result<(), WatchRefusal> {
        self.admit(state, session).await.map(|_| ())
    }

    pub async fn install(
        &self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        interest: &AtomicBool,
    ) -> Result<WatchHandle, ChangeWatchErrorCode> {
        // Reuse the actual current snapshot owner without a head/read lease.
        let admitted = self
            .admit(state, session)
            .await
            .map_err(WatchRefusal::code)?;
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
/// Why current authority refused a watch. An access error stays one, so that a
/// closing connection reports it through the socket's single mapping.
#[derive(Clone, Copy, Debug)]
pub(in crate::product) enum WatchRefusal {
    /// The session's credential, membership or browser session is no longer current.
    Access(AccessError),
    /// Passive-read admission refused: grant, receiver binding, epoch or ownership.
    Read(ReadRefusal),
    /// The gateway has no passive-read authority composed, or the task failed.
    Unavailable,
}

impl WatchRefusal {
    /// The registration reply's code.
    pub fn code(self) -> ChangeWatchErrorCode {
        match self {
            Self::Access(error) => admission_code(access_refusal(error)),
            Self::Read(refusal) => admission_code(refusal),
            Self::Unavailable => ChangeWatchErrorCode::TemporarilyUnavailable,
        }
    }

    /// How a live connection closes when a notice or a periodic check is refused.
    pub fn close_reason(self) -> SessionCloseReason {
        match self {
            Self::Access(error) => close_reason(error),
            Self::Read(ReadRefusal::Unverifiable) | Self::Unavailable => {
                SessionCloseReason::TemporaryUnavailable
            }
            Self::Read(_) => SessionCloseReason::AuthorizationLost,
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
    use nessa_protocol::product::generated::CHANGE_WATCH_ID_PATTERN;

    /// Row R6: the identities the server mints, up to the last counter, match
    /// the pattern and length the schema publishes, so the format has one owner
    /// and a schema change that disagrees with minting fails here.
    #[test]
    fn minted_identities_match_the_published_pattern_and_length() {
        let pattern = regex::Regex::new(CHANGE_WATCH_ID_PATTERN).unwrap();
        let mut tokens = WatchToken::new(Uuid::from_u128(u128::MAX));
        let first = tokens.next().unwrap();
        assert!(pattern.is_match(&first), "{first}");
        assert!(first.len() <= MAX_CHANGE_WATCH_ID_BYTES);
        tokens.accepted = u64::MAX - 1;
        let last = tokens.next().unwrap();
        assert!(pattern.is_match(&last), "{last}");
        // The published bound is exactly the longest identity minted.
        assert_eq!(last.len(), MAX_CHANGE_WATCH_ID_BYTES);
    }

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
