//! The purge-before-forgetting store and the recheck rule, against doubles
//! that record the order of effects.
use super::*;
use nessa_auth::domain::pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId};
use nessa_protocol::product_contract::generated::SessionCloseReason;

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
struct Store(Arc<Effects>, bool);
fn enrollment() -> PendingEnrollment {
    PendingEnrollment::new(
        PrivateKeyMaterial::new(zeroize::Zeroizing::new([7; 32])),
        [9; 44],
        intent(),
    )
}
impl ClientPendingStore for Store {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.0.push("load_pending");
        Ok((!self.1).then(enrollment))
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.0.push("load_credential");
        Ok(self.1.then(|| {
            DeviceCredential::new(
                enrollment(),
                CredentialId::new("device-credential").unwrap(),
                ResourceId::new("saved-receiver").unwrap(),
            )
        }))
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.0.push("save_credential");
        Ok(())
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        self.0.push("end_enrollment");
        Ok(())
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.0.push("save_pending");
        Ok(())
    }
}
struct Purges(Arc<Effects>, Option<CacheError>);
impl CachePurges for Purges {
    fn purge_receiver(&mut self, receiver: &Id) -> Result<PurgeReceipt, CacheError> {
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
    PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([2; AttemptId::LENGTH]),
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
        Arc::new(Store(effects.clone(), issued)),
        Id::new("saved-receiver").unwrap(),
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

/// Row PC5: authority refusals ask status again; transport, timing, source and
/// shape failures do not, and nothing here purges by itself.
#[test]
fn only_authority_refusals_ask_status_again() {
    for asks in [
        GatewayError::ProductRefused,
        GatewayError::Authentication(SessionCloseReason::AuthorizationLost),
        GatewayError::Record(RecordReadErrorCode::Unauthorized),
        GatewayError::Record(RecordReadErrorCode::Forbidden),
        GatewayError::Record(RecordReadErrorCode::WrongReceiver),
        GatewayError::Record(RecordReadErrorCode::StaleEpoch),
        GatewayError::Catalogue(CatalogueReadErrorCode::Unauthorized),
        GatewayError::Catalogue(CatalogueReadErrorCode::Forbidden),
        GatewayError::Catalogue(CatalogueReadErrorCode::WrongReceiver),
        GatewayError::Catalogue(CatalogueReadErrorCode::StaleEpoch),
        GatewayError::Watch(ChangeWatchErrorCode::Unauthorized),
        GatewayError::Watch(ChangeWatchErrorCode::Forbidden),
        GatewayError::Watch(ChangeWatchErrorCode::WrongReceiver),
        GatewayError::Watch(ChangeWatchErrorCode::StaleEpoch),
    ] {
        assert!(asks_status(asks), "{asks:?}");
    }
    for keeps in [
        GatewayError::Closed(None),
        GatewayError::Transport,
        GatewayError::TimedOut,
        GatewayError::NativeHandshake,
        GatewayError::Correlation,
        GatewayError::ResponseTooLarge,
        GatewayError::Record(RecordReadErrorCode::WrongOwner),
        GatewayError::Record(RecordReadErrorCode::Unverifiable),
        GatewayError::Record(RecordReadErrorCode::SourcePreparing),
        GatewayError::Catalogue(CatalogueReadErrorCode::ServerBusy),
        GatewayError::Watch(ChangeWatchErrorCode::WrongOwner),
        GatewayError::Watch(ChangeWatchErrorCode::WatchCapacity),
        GatewayError::Watch(ChangeWatchErrorCode::WatchDuplicate),
        GatewayError::Watch(ChangeWatchErrorCode::TemporarilyUnavailable),
    ] {
        assert!(!asks_status(keeps), "{keeps:?}");
    }
}
