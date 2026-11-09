//! The purge-before-forgetting store and the recheck rule, against doubles
//! that record the order of effects.
use super::super::SubscriptionCode;
use super::*;
use nessa_auth::domain::{
    pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId},
    MAX_IDENTIFIER_BYTES,
};
use nessa_protocol::product_contract::generated::{ConversationErrorCode, SessionCloseReason};

#[derive(Default)]
struct Effects(Mutex<Vec<&'static str>>);
impl Effects {
    fn push(&self, effect: &'static str) {
        self.0.lock().unwrap().push(effect);
    }
    fn taken(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}
/// A record store holding either an issued credential or only a pending
/// enrollment.
struct Store {
    effects: Arc<Effects>,
    issued: Mutex<Option<(PublicIntent, String)>>,
    removal_failure: Option<PrivateStateError>,
}
impl Store {
    fn new(effects: Arc<Effects>, issued: bool) -> Self {
        Self {
            effects,
            issued: Mutex::new(issued.then(|| (intent(), "saved-receiver".to_owned()))),
            removal_failure: None,
        }
    }
}
fn enrollment() -> PendingEnrollment {
    PendingEnrollment::new(
        PrivateKeyMaterial::new(zeroize::Zeroizing::new([7; 32])),
        [9; 44],
        intent(),
    )
}
impl ClientPendingStore for Store {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.effects.push("load_pending");
        Ok(self.issued.lock().unwrap().is_none().then(enrollment))
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.effects.push("load_credential");
        Ok(self
            .issued
            .lock()
            .unwrap()
            .as_ref()
            .map(|(intent, receiver)| {
                DeviceCredential::new(
                    PendingEnrollment::new(
                        PrivateKeyMaterial::new(zeroize::Zeroizing::new([7; 32])),
                        [9; 44],
                        *intent,
                    ),
                    CredentialId::new("device-credential").unwrap(),
                    ResourceId::new(receiver).unwrap(),
                )
            }))
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.effects.push("save_credential");
        Ok(())
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        self.effects.push("end_enrollment");
        if let Some(error) = self.removal_failure {
            return Err(error);
        }
        *self.issued.lock().unwrap() = None;
        Ok(())
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.effects.push("save_pending");
        Ok(())
    }
}
struct Purges(Arc<Effects>, Option<CacheError>);
impl CachePurges for Purges {
    fn purge_receiver(&mut self, receiver: &ResourceId) -> Result<PurgeReceipt, CacheError> {
        self.0.push("purge");
        if let Some(error) = self.1.clone() {
            return Err(error);
        }
        Ok(PurgeReceipt {
            receiver: receiver.clone(),
            transcripts: 1,
            records: 2,
            catalogue_entries: 3,
            observed_at_ms: 4,
        })
    }
}
fn intent() -> PublicIntent {
    intent_for(2)
}
fn intent_for(attempt: u8) -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([attempt; AttemptId::LENGTH]),
        ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
        1,
        9,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
fn store(effects: &Arc<Effects>, open: Result<Option<CacheError>, CacheError>) -> PurgeBeforeEnd {
    holding(effects, open, true)
}
fn holding(
    effects: &Arc<Effects>,
    open: Result<Option<CacheError>, CacheError>,
    issued: bool,
) -> PurgeBeforeEnd {
    let purging = effects.clone();
    PurgeBeforeEnd::new(
        Arc::new(Store::new(effects.clone(), issued)),
        Box::new(move || {
            purging.push("open");
            open.clone().map(|failure| {
                Box::new(Purges(purging.clone(), failure)) as Box<dyn CachePurges + Send>
            })
        }),
    )
}

/// Row PC3: an ended enrollment purges the saved receiver, with its receipt,
/// before the record goes; every other record operation is the store's own.
#[test]
fn purge_precedes_record_removal() {
    let effects = Arc::new(Effects::default());
    let purging = store(&effects, Ok(None));
    assert!(purging.purge().is_none(), "nothing purged before an end");
    purging.load_pending().unwrap();
    purging.load_credential().unwrap();
    assert_eq!(effects.taken(), ["load_pending", "load_credential"]);
    purging.end_enrollment(intent()).unwrap();
    assert_eq!(
        effects.taken(),
        ["load_credential", "open", "purge", "end_enrollment"]
    );
    let receipt = purging.purge().unwrap().unwrap();
    assert_eq!(receipt.receiver.as_str(), "saved-receiver");
}

/// Rows PC3, PC4: a purge that cannot run or complete keeps the record, so the
/// next authenticated Terminal status purges again; the cause is kept.
#[test]
fn a_failed_purge_keeps_the_enrollment_record() {
    for (open, expected) in [
        (Err(CacheError::Unavailable), vec!["open"]),
        (Ok(Some(CacheError::Uncertain)), vec!["open", "purge"]),
    ] {
        let effects = Arc::new(Effects::default());
        let purging = store(&effects, open.clone());
        assert_eq!(
            purging.end_enrollment(intent()),
            Err(PrivateStateError::Unavailable)
        );
        let expected: Vec<_> = std::iter::once("load_credential").chain(expected).collect();
        assert_eq!(effects.taken(), expected, "no record removal");
        let cause = match open {
            Err(error) | Ok(Some(error)) => error,
            Ok(None) => unreachable!(),
        };
        assert_eq!(purging.purge(), Some(Err(cause)));
    }
}

/// Review F3: ending a pending enrollment (an authenticated Unclaimed status)
/// removes it without opening or purging any cache.
#[test]
fn only_ending_an_issued_credential_purges() {
    let effects = Arc::new(Effects::default());
    let pending = holding(&effects, Ok(None), false);
    pending.end_enrollment(intent()).unwrap();
    assert_eq!(effects.taken(), ["load_credential", "end_enrollment"]);
    assert!(pending.purge().is_none());
}

#[test]
fn mismatched_end_intent_opens_no_cache_and_removes_no_credential() {
    let effects = Arc::new(Effects::default());
    let purging = store(&effects, Ok(None));
    let other = intent_for(4);
    assert_eq!(
        purging.end_enrollment(other),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(effects.taken(), ["load_credential"]);
    assert!(purging.purge().is_none());
    assert_eq!(
        purging.load_credential().unwrap().unwrap().intent(),
        intent()
    );
}

#[test]
fn one_purge_policy_uses_each_new_issued_receivers_binding() {
    let effects = Arc::new(Effects::default());
    let state = Arc::new(Store::new(effects.clone(), false));
    let opening = effects.clone();
    let purging = PurgeBeforeEnd::new(
        state.clone(),
        Box::new(move || {
            opening.push("open");
            Ok(Box::new(Purges(opening.clone(), None)))
        }),
    );
    for (attempt, receiver) in [(2, "first-receiver"), (4, "second-receiver")] {
        let expected = intent_for(attempt);
        *state.issued.lock().unwrap() = Some((expected, receiver.to_owned()));
        purging.end_enrollment(expected).unwrap();
        assert_eq!(
            purging.purge().unwrap().unwrap().receiver.as_str(),
            receiver
        );
        assert!(state.issued.lock().unwrap().is_none());
    }
}

#[test]
fn successful_purge_evidence_survives_failed_credential_removal() {
    let effects = Arc::new(Effects::default());
    let mut state = Store::new(effects.clone(), true);
    state.removal_failure = Some(PrivateStateError::Uncertain);
    let opening = effects.clone();
    let purging = PurgeBeforeEnd::new(
        Arc::new(state),
        Box::new(move || {
            opening.push("open");
            Ok(Box::new(Purges(opening.clone(), None)))
        }),
    );
    assert_eq!(
        purging.end_enrollment(intent()),
        Err(PrivateStateError::Uncertain)
    );
    assert_eq!(
        effects.taken(),
        ["load_credential", "open", "purge", "end_enrollment"]
    );
    assert_eq!(
        purging.purge().unwrap().unwrap().receiver.as_str(),
        "saved-receiver"
    );
}

/// Row PC5: authority refusals ask status again; transport, timing, source and
/// shape failures do not, and nothing here purges by itself.
#[test]
fn only_authority_refusals_ask_status_again() {
    for asks in [
        GatewayError::ProductRefused,
        GatewayError::Authentication(SessionCloseReason::AuthenticationFailed),
        GatewayError::Authentication(SessionCloseReason::CredentialRevoked),
        GatewayError::Authentication(SessionCloseReason::CredentialExpired),
        GatewayError::Authentication(SessionCloseReason::AuthorizationLost),
        GatewayError::Record(RecordReadErrorCode::Unauthorized),
        GatewayError::Record(RecordReadErrorCode::Forbidden),
        GatewayError::Record(RecordReadErrorCode::WrongReceiver),
        GatewayError::Record(RecordReadErrorCode::StaleEpoch),
        GatewayError::Catalogue(CatalogueReadErrorCode::Unauthorized),
        GatewayError::Catalogue(CatalogueReadErrorCode::Forbidden),
        GatewayError::Catalogue(CatalogueReadErrorCode::WrongReceiver),
        GatewayError::Catalogue(CatalogueReadErrorCode::StaleEpoch),
        GatewayError::HeldFile(HeldFileRefusal::Unauthorized),
        GatewayError::HeldFile(HeldFileRefusal::Forbidden),
        GatewayError::Subscription(SubscriptionRefusal::Unauthorized),
        GatewayError::Subscription(SubscriptionRefusal::Forbidden),
        GatewayError::Closed(Some(SessionCloseReason::AuthenticationFailed)),
        GatewayError::Closed(Some(SessionCloseReason::CredentialRevoked)),
        GatewayError::Closed(Some(SessionCloseReason::CredentialExpired)),
        GatewayError::Closed(Some(SessionCloseReason::AuthorizationLost)),
    ] {
        assert!(asks_status(asks), "{asks:?}");
    }
    for keeps in [
        GatewayError::Closed(None),
        GatewayError::Closed(Some(SessionCloseReason::TemporaryUnavailable)),
        GatewayError::Closed(Some(SessionCloseReason::ServerShutdown)),
        GatewayError::Closed(Some(SessionCloseReason::ProtocolIncompatible)),
        GatewayError::Closed(Some(SessionCloseReason::TransportInterrupted)),
        GatewayError::Authentication(SessionCloseReason::HandshakeTimeout),
        GatewayError::Authentication(SessionCloseReason::TemporaryUnavailable),
        GatewayError::Authentication(SessionCloseReason::GatewayRestarting),
        GatewayError::Authentication(SessionCloseReason::GatewayOverloaded),
        GatewayError::Authentication(SessionCloseReason::TransportInterrupted),
        GatewayError::Authentication(SessionCloseReason::ServerShutdown),
        GatewayError::Authentication(SessionCloseReason::ProtocolIncompatible),
        GatewayError::Transport,
        GatewayError::TimedOut,
        GatewayError::NativeHandshake,
        GatewayError::Correlation,
        GatewayError::ResponseTooLarge,
        GatewayError::Record(RecordReadErrorCode::WrongOwner),
        GatewayError::Record(RecordReadErrorCode::Unverifiable),
        GatewayError::Record(RecordReadErrorCode::SourcePreparing),
        GatewayError::Catalogue(CatalogueReadErrorCode::ServerBusy),
        GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::ConversationNotFound,
        )),
        GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::TemporarilyUnavailable,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Conversation(
            ConversationErrorCode::ConversationNotFound,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Conversation(
            ConversationErrorCode::ConversationDeleted,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Subscription(
            SubscriptionCode::CursorAhead,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Subscription(
            SubscriptionCode::Duplicate,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Subscription(
            SubscriptionCode::Capacity,
        )),
        GatewayError::Subscription(SubscriptionRefusal::Subscription(SubscriptionCode::Unknown)),
    ] {
        assert!(!asks_status(keeps), "{keeps:?}");
    }
}

#[test]
fn published_pinned_classifier_preserves_phase_receiver_and_epoch() {
    use nessa_auth::domain::pairing::{DisclosedConsent, TerminalCause};
    use nessa_auth::domain::{AudienceId, OrganizationId, Resource};
    let public = intent();
    let consent = Box::new(
        DisclosedConsent::new(
            public,
            AudienceId::new("gateway").unwrap(),
            ConsentClass::DeviceRead
                .grant(Resource::new(
                    OrganizationId::new("org").unwrap(),
                    ResourceId::new("owner").unwrap(),
                ))
                .unwrap(),
        )
        .unwrap(),
    );
    for status in [
        NativePairingStatus::Pending(public),
        NativePairingStatus::Claimed(consent.clone()),
        NativePairingStatus::Approved(consent.clone()),
        NativePairingStatus::Staging(consent.clone()),
    ] {
        assert_eq!(
            PinnedStatus::from_native(&status),
            Some(PinnedStatus::NotActive)
        );
    }
    let active = NativePairingStatus::Active {
        consent: consent.clone(),
        credential: CredentialId::new("credential").unwrap(),
        receiver: ResourceId::new("receiver").unwrap(),
        access_epoch: 7,
        tunnel_secret: nessa_protocol::tunnel::TunnelSecret::new([9; 32]),
    };
    assert_eq!(
        PinnedStatus::from_native(&active),
        Some(PinnedStatus::Active {
            receiver: Id::new("receiver").unwrap(),
            access_epoch: 7,
        })
    );
    let oversized = NativePairingStatus::Active {
        consent: consent.clone(),
        credential: CredentialId::new("credential").unwrap(),
        receiver: ResourceId::new("r".repeat(nessa_sync::replication::domain::MAX_ID_BYTES + 1))
            .unwrap(),
        access_epoch: 7,
        tunnel_secret: nessa_protocol::tunnel::TunnelSecret::new([9; 32]),
    };
    assert_eq!(PinnedStatus::from_native(&oversized), None);
    let terminal = NativePairingStatus::Terminal {
        consent,
        cause: TerminalCause::CredentialRevoked,
    };
    assert_eq!(
        PinnedStatus::from_native(&terminal),
        Some(PinnedStatus::Terminal)
    );
}

/// Pairing receiver identities keep their full byte bound when an authenticated
/// Terminal status purges and removes their credential.
#[test]
fn terminal_cleanup_accepts_pairing_receivers_beyond_the_sync_id_bound() {
    for receiver in [
        "r".repeat(MAX_IDENTIFIER_BYTES),
        "é".repeat(MAX_IDENTIFIER_BYTES / 2),
    ] {
        let effects = Arc::new(Effects::default());
        let state = Arc::new(Store::new(effects.clone(), true));
        *state.issued.lock().unwrap() = Some((intent(), receiver.clone()));
        let opening = effects.clone();
        let purging = PurgeBeforeEnd::new(
            state.clone(),
            Box::new(move || {
                opening.push("open");
                Ok(Box::new(Purges(opening.clone(), None)))
            }),
        );
        purging.end_enrollment(intent()).unwrap();
        assert_eq!(
            effects.taken(),
            ["load_credential", "open", "purge", "end_enrollment"]
        );
        assert_eq!(
            purging.purge().unwrap().unwrap().receiver.as_str(),
            receiver
        );
        assert!(state.issued.lock().unwrap().is_none());
    }
}
