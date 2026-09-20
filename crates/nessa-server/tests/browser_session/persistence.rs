use super::*;
use crate::browser_session::{
    application::{invalidation_reason, ReadBrowserSession},
    domain::value_objects::{RemovalReason, FUTURE_TOLERANCE_SECONDS, IDLE_SECONDS},
};
use nessa_auth::{
    application::{
        ports::{
            AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier,
            PortFuture, VerifiedCredential,
        },
        session::AuthenticateSession,
    },
    domain::{
        AudienceId, Credential, CredentialId, Membership, MembershipId, MembershipRole,
        MembershipStatus, OrganizationId, PrincipalId,
    },
};

struct Authority {
    snapshot: AccessSnapshot,
    expires_at: Option<u64>,
}
impl CredentialVerifier for Authority {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            if evidence.expose_bytes() != b"secret" {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id: CredentialId::new("credential").unwrap(),
                expires_at: self.expires_at,
            })
        })
    }
}
impl AccessReader for Authority {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move { Ok(self.snapshot.clone()) })
    }
}
impl Clock for Authority {
    fn unix_milliseconds(&self) -> u64 {
        100_000
    }
}
/// A clock reading later than any instant these tests journal, so reopening a
/// store exercises replay rather than the implausible-state sweep. Tests about
/// the sweep itself call `PersistentSessions::open` with their own reading.
const REPLAY_NOW: u64 = 100 + 2 * IDLE_SECONDS;
fn reopen(path: &std::path::Path) -> Result<PersistentSessions, AccessError> {
    PersistentSessions::open(path, REPLAY_NOW)
}
fn reopen_bounded(path: &std::path::Path, bound: u64) -> Result<PersistentSessions, AccessError> {
    PersistentSessions::open_bounded(path, bound, REPLAY_NOW)
}
/// Open a journal that should be free, waiting out a lock nobody means to hold.
///
/// The store takes its journal's lock with `try_lock`, which is the right
/// thing in production: another process holding it must be refused rather
/// than queued behind. But these tests are threads of one process that also
/// spawns subprocesses, and between a fork and its exec the child holds a
/// duplicate of every descriptor this process has — `O_CLOEXEC` closes it at
/// exec, not at fork. A journal dropped by one test can therefore stay locked
/// for as long as an unrelated test takes to exec, which is what failed
/// `a_journal_at_its_bound_cannot_retire_implausible_state_and_fails_closed`
/// on a loaded Linux runner. Waiting that window out is not the same as
/// ignoring it: a journal that is genuinely unopenable still fails the test,
/// and with its own error rather than this one.
///
/// Assertions that expect a refusal call the plain helpers, so they still get
/// their answer immediately.
fn opened<T>(mut open: impl FnMut() -> Result<T, AccessError>) -> T {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match open() {
            Ok(store) => return store,
            Err(error) if std::time::Instant::now() >= deadline => {
                panic!("the journal never became openable: {error:?}")
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}
fn credential_id() -> CredentialId {
    CredentialId::new("credential").unwrap()
}
async fn session(expires_at: Option<u64>) -> BrowserSession {
    let principal = PrincipalId::new("person").unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let audience = AudienceId::new("gateway").unwrap();
    let authority = Authority {
        snapshot: AccessSnapshot {
            credential: Credential::new(
                credential_id(),
                principal.clone(),
                organization.clone(),
                audience.clone(),
                100,
                expires_at,
                vec![],
            )
            .unwrap(),
            membership: Membership::new(
                MembershipId::new("member").unwrap(),
                principal,
                organization,
                MembershipRole::Member,
                MembershipStatus::Active,
            ),
            revision: 1,
        },
        expires_at,
    };
    let authenticated = AuthenticateSession {
        verifier: &authority,
        access: &authority,
        clock: &authority,
    }
    .execute(
        &CredentialEvidence::new(b"secret".to_vec()).unwrap(),
        &audience,
    )
    .await
    .unwrap();
    BrowserSession::new(
        authenticated.context().credential_id().clone(),
        "https://127.0.0.1:1443".into(),
        100,
    )
    .unwrap()
}
#[tokio::test]
async fn renewal_crosses_original_deadline_and_restart_then_logout_is_durable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.jsonl");
    let id = "a".repeat(64);
    let store = opened(|| reopen(&path));
    assert!(reopen(&path).is_err());
    store
        .insert(id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let renewed = store
        .renew(id.clone(), 100 + 20 * 86400, credential_id())
        .await
        .unwrap();
    assert_eq!(renewed.idle_expires_at(), 100 + 50 * 86400);
    drop(store);
    let store = opened(|| reopen(&path));
    assert_eq!(store.get(id.clone()).await.unwrap().unwrap(), renewed);
    store
        .remove(
            id.clone(),
            100 + 35 * 86400,
            RemovalReason::SignOut,
            Some(credential_id()),
        )
        .await
        .unwrap();
    assert!(store
        .renew(id.clone(), 100 + 35 * 86400, credential_id())
        .await
        .is_err());
    drop(store);
    let store = opened(|| reopen(&path));
    assert!(store.get(id).await.unwrap().is_none());
    // Durable bytes are inspected only after releasing exclusive journal ownership.
    drop(store);
    let records = std::fs::read_to_string(path).unwrap();
    let last: StoredRecord = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert!(matches!(last.changes[0].reason, Reason::SignOut));
    assert_eq!(last.changes[0].initiator.as_deref(), Some("credential"));
    assert_eq!(
        last.changes[0].before.as_ref().unwrap(),
        &StoredSession::from(&renewed)
    );
    assert!(last.changes[0].after.is_none());
}

#[tokio::test]
async fn transitions_cannot_precede_the_session_state_they_replace_live_or_on_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("causal-time.jsonl");
    let id = "9".repeat(64);
    let store = opened(|| reopen(&path));
    store
        .insert(id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    store
        .renew(id.clone(), 4_000, credential_id())
        .await
        .unwrap();
    assert_eq!(
        store
            .remove(
                id.clone(),
                3_999,
                RemovalReason::SignOut,
                Some(credential_id()),
            )
            .await,
        Err(AccessError::Unavailable)
    );
    assert!(store.get(id.clone()).await.unwrap().is_some());
    store
        .remove(id, 4_001, RemovalReason::SignOut, Some(credential_id()))
        .await
        .unwrap();
    drop(store);

    let contents = std::fs::read_to_string(&path).unwrap();
    let mut records = contents
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    records.last_mut().unwrap()["at"] = serde_json::json!(3_999);
    let corrupted = records
        .into_iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
    std::fs::write(&path, corrupted).unwrap();
    assert!(reopen(&path).is_err());
}

#[tokio::test]
async fn explicit_transitions_require_the_verified_matching_credential() {
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("initiator.jsonl")).unwrap();
    let id = "6".repeat(64);
    store
        .insert(id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let other = CredentialId::new("other").unwrap();

    assert_eq!(
        store.renew(id.clone(), 4_000, other.clone()).await,
        Err(AccessError::IdentityMismatch)
    );
    assert_eq!(
        store
            .remove(id.clone(), 101, RemovalReason::SignOut, None)
            .await,
        Err(AccessError::IdentityMismatch)
    );
    assert_eq!(
        store
            .remove(
                id.clone(),
                101,
                RemovalReason::CredentialRevoked,
                Some(other),
            )
            .await,
        Err(AccessError::IdentityMismatch)
    );
    assert!(store.get(id.clone()).await.unwrap().is_some());
    store
        .remove(
            id.clone(),
            101,
            RemovalReason::SignOut,
            Some(credential_id()),
        )
        .await
        .unwrap();
    assert!(store.get(id).await.unwrap().is_none());
}

#[tokio::test]
async fn abandoned_replacement_restores_prior_session_in_one_durable_audit_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("abandoned-login.jsonl");
    let prior_id = "1".repeat(64);
    let replacement_id = "2".repeat(64);
    let prior = session(None).await;
    let store = opened(|| reopen(&path));
    store
        .insert(prior_id.clone(), prior.clone(), None, 100)
        .await
        .unwrap();
    store
        .insert(
            replacement_id.clone(),
            session(None).await,
            Some(prior_id.clone()),
            100,
        )
        .await
        .unwrap();
    store
        .abandon_login(
            replacement_id.clone(),
            Some((prior_id.clone(), prior.clone())),
            100,
        )
        .await
        .unwrap();
    assert_eq!(store.get(prior_id.clone()).await.unwrap(), Some(prior));
    assert!(store.get(replacement_id.clone()).await.unwrap().is_none());
    drop(store);

    let store = opened(|| reopen(&path));
    assert!(store.get(prior_id).await.unwrap().is_some());
    assert!(store.get(replacement_id).await.unwrap().is_none());
    // Durable bytes are inspected only after releasing exclusive journal ownership.
    drop(store);
    let records = std::fs::read_to_string(path).unwrap();
    let record: StoredRecord = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert_eq!(record.changes.len(), 2);
    assert_eq!(record.changes[0].reason, Reason::AbandonedLogin);
    assert_eq!(
        record.changes[1].reason,
        Reason::RestoredAfterAbandonedLogin
    );
    assert!(record
        .changes
        .iter()
        .all(|change| change.initiator.is_none()));
}

#[tokio::test]
async fn abandoned_replacement_restores_the_exact_prior_state_removed_by_insert() {
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("renewed-prior.jsonl")).unwrap();
    let prior_id = "4".repeat(64);
    let replacement_id = "5".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let stale_capture = store.get(prior_id.clone()).await.unwrap().unwrap();
    store
        .renew(prior_id.clone(), 3_700, credential_id())
        .await
        .unwrap();
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 3_701).unwrap(),
            Some(prior_id.clone()),
            3_701,
        )
        .await
        .unwrap()
        .expect("active prior was replaced");
    assert_ne!(replaced.1, stale_capture);
    assert_eq!(replaced.1.renewed_at(), 3_700);

    store
        .abandon_login(replacement_id, Some(replaced.clone()), 3_701)
        .await
        .unwrap();
    assert_eq!(store.get(prior_id).await.unwrap(), Some(replaced.1));
}

#[tokio::test]
async fn abandoned_replacement_is_reclaimed_after_its_prior_expires() {
    // A login that outlives the remainder of the prior's idle window: the prior
    // is no longer restorable when cleanup runs, which must not keep the
    // undisclosed replacement alive.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("expired-prior.jsonl");
    let prior_id = "6".repeat(64);
    let replacement_id = "7".repeat(64);
    let store = opened(|| reopen(&path));
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced_at = 100 + IDLE_SECONDS - 1;
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(
                credential_id(),
                "https://127.0.0.1:1443".into(),
                replaced_at,
            )
            .unwrap(),
            Some(prior_id.clone()),
            replaced_at,
        )
        .await
        .unwrap()
        .expect("active prior was replaced");

    store
        .abandon_login(
            replacement_id.clone(),
            Some(replaced.clone()),
            100 + IDLE_SECONDS,
        )
        .await
        .unwrap();

    assert!(store.get(replacement_id.clone()).await.unwrap().is_none());
    assert!(store.get(prior_id.clone()).await.unwrap().is_none());
    drop(store);

    let store = opened(|| reopen(&path));
    assert!(store.get(replacement_id).await.unwrap().is_none());
    assert!(store.get(prior_id).await.unwrap().is_none());
    drop(store);
    let records = std::fs::read_to_string(path).unwrap();
    let record: StoredRecord = serde_json::from_str(records.lines().last().unwrap()).unwrap();
    assert_eq!(record.changes.len(), 1);
    assert_eq!(record.changes[0].reason, Reason::AbandonedLogin);
    assert!(record.changes[0].initiator.is_none());
}

#[tokio::test]
async fn abandoned_replacement_restores_a_prior_active_for_one_more_second() {
    // The moment before the case above: still restorable, so it is restored.
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("just-active-prior.jsonl")).unwrap();
    let prior_id = "8".repeat(64);
    let replacement_id = "9".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced_at = 100 + IDLE_SECONDS - 1;
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(
                credential_id(),
                "https://127.0.0.1:1443".into(),
                replaced_at,
            )
            .unwrap(),
            Some(prior_id.clone()),
            replaced_at,
        )
        .await
        .unwrap()
        .expect("active prior was replaced");

    store
        .abandon_login(replacement_id.clone(), Some(replaced.clone()), replaced_at)
        .await
        .unwrap();

    assert!(store.get(replacement_id).await.unwrap().is_none());
    assert_eq!(store.get(prior_id).await.unwrap(), Some(replaced.1));
}

#[tokio::test]
async fn abandoned_replacement_leaves_a_prior_id_a_later_login_now_owns() {
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("competing-prior.jsonl")).unwrap();
    let prior_id = "a".repeat(64);
    let replacement_id = "b".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
            Some(prior_id.clone()),
            101,
        )
        .await
        .unwrap()
        .expect("active prior was replaced");
    // The freed ID is taken again before the abandoned login is reclaimed.
    let competing =
        BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 102).unwrap();
    store
        .insert(prior_id.clone(), competing.clone(), None, 102)
        .await
        .unwrap();

    store
        .abandon_login(replacement_id.clone(), Some(replaced), 103)
        .await
        .unwrap();

    assert!(store.get(replacement_id).await.unwrap().is_none());
    assert_eq!(store.get(prior_id).await.unwrap(), Some(competing));
}

#[tokio::test]
async fn abandoned_replacement_must_still_restore_a_prior_that_can_be_restored() {
    // The caller's own check refuses a cleanup that does not name the prior it
    // replaced, before any record is built.
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("omitted-caller.jsonl")).unwrap();
    let prior_id = "c".repeat(64);
    let replacement_id = "d".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
            Some(prior_id.clone()),
            101,
        )
        .await
        .unwrap()
        .expect("active prior was replaced");

    assert_eq!(
        store.abandon_login(replacement_id.clone(), None, 101).await,
        Err(AccessError::IdentityMismatch)
    );
    assert!(store.get(replacement_id).await.unwrap().is_some());
    assert!(store.get(prior_id).await.unwrap().is_none());
}

#[tokio::test]
async fn replay_rejects_dropping_a_prior_that_was_still_restorable() {
    // The same omission written straight into the journal, which is the only way
    // to reach the replay rule itself: the caller check above never gets there.
    // A prior that was active at the record's own instant must be restored by it.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dropped-restorable-prior.jsonl");
    let store = opened(|| reopen(&path));
    let prior_id = "4".repeat(64);
    let replacement_id = "5".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
            Some(prior_id),
            101,
        )
        .await
        .unwrap();
    store
        .abandon_login(replacement_id, replaced, 101)
        .await
        .unwrap();
    drop(store);

    let contents = std::fs::read_to_string(&path).unwrap();
    let mut records: Vec<serde_json::Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let changes = records.last_mut().unwrap()["changes"]
        .as_array_mut()
        .unwrap();
    changes.retain(|change| change["reason"] != "RestoredAfterAbandonedLogin");
    let corrupted = records
        .into_iter()
        .map(|record| serde_json::to_string(&record).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, corrupted).unwrap();

    assert!(reopen(&path).is_err());
}

#[tokio::test]
async fn replay_accepts_dropping_a_prior_that_had_expired_by_the_record() {
    // The positive half of the same rule, also reached only through replay: the
    // record that legitimately omits an expired prior must still reopen.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dropped-expired-prior.jsonl");
    let store = opened(|| reopen(&path));
    let prior_id = "6".repeat(64);
    let replacement_id = "7".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced_at = 100 + IDLE_SECONDS - 1;
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(
                credential_id(),
                "https://127.0.0.1:1443".into(),
                replaced_at,
            )
            .unwrap(),
            Some(prior_id.clone()),
            replaced_at,
        )
        .await
        .unwrap();
    store
        .abandon_login(replacement_id.clone(), replaced, 100 + IDLE_SECONDS)
        .await
        .unwrap();
    drop(store);

    let store = opened(|| reopen(&path));
    assert!(store.get(replacement_id).await.unwrap().is_none());
    assert!(store.get(prior_id).await.unwrap().is_none());
}

#[tokio::test]
async fn abandoned_replacement_rejects_a_fabricated_same_origin_prior() {
    let directory = tempfile::tempdir().unwrap();
    let store = reopen(&directory.path().join("fabricated-prior.jsonl")).unwrap();
    let prior_id = "4".repeat(64);
    let replacement_id = "5".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    let replaced = store
        .insert(
            replacement_id.clone(),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
            Some(prior_id.clone()),
            101,
        )
        .await
        .unwrap()
        .unwrap();
    let fabricated =
        BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap();

    assert_eq!(
        store
            .abandon_login(
                replacement_id.clone(),
                Some(("9".repeat(64), fabricated)),
                101,
            )
            .await,
        Err(AccessError::IdentityMismatch)
    );
    assert!(store.get(replacement_id.clone()).await.unwrap().is_some());
    assert!(store.get(prior_id.clone()).await.unwrap().is_none());

    store
        .abandon_login(replacement_id, Some(replaced.clone()), 101)
        .await
        .unwrap();
    assert_eq!(store.get(prior_id).await.unwrap(), Some(replaced.1));
}

#[tokio::test]
async fn replay_rejects_restoration_that_does_not_match_the_replaced_prior() {
    let directory = tempfile::tempdir().unwrap();
    for tamper in ["id", "state"] {
        let path = directory
            .path()
            .join(format!("wrong-restored-{tamper}.jsonl"));
        let store = opened(|| reopen(&path));
        let prior_id = "4".repeat(64);
        let replacement_id = "5".repeat(64);
        store
            .insert(prior_id, session(None).await, None, 100)
            .await
            .unwrap();
        let replaced = store
            .insert(
                replacement_id.clone(),
                BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
                Some("4".repeat(64)),
                101,
            )
            .await
            .unwrap();
        store
            .abandon_login(replacement_id, replaced, 101)
            .await
            .unwrap();
        drop(store);

        let contents = std::fs::read_to_string(&path).unwrap();
        let mut records: Vec<serde_json::Value> = contents
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let restored = records.last_mut().unwrap()["changes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|change| change["reason"] == "RestoredAfterAbandonedLogin")
            .unwrap();
        match tamper {
            "id" => restored["id"] = serde_json::json!("9".repeat(64)),
            _ => restored["after"]["credential_id"] = serde_json::json!("other-credential"),
        }
        let corrupted = records
            .into_iter()
            .map(|record| serde_json::to_string(&record).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, corrupted).unwrap();
        assert!(reopen(&path).is_err());
    }
}

#[tokio::test]
async fn replay_rejects_replacing_a_session_with_the_same_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("same-id-replacement.jsonl");
    let store = opened(|| reopen(&path));
    let prior_id = "4".repeat(64);
    store
        .insert(prior_id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    store
        .insert(
            "5".repeat(64),
            BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), 101).unwrap(),
            Some(prior_id.clone()),
            101,
        )
        .await
        .unwrap();
    drop(store);

    let contents = std::fs::read_to_string(&path).unwrap();
    let mut records: Vec<serde_json::Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let sign_in = records[1]["changes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|change| change["reason"] == "SignIn")
        .unwrap();
    sign_in["id"] = serde_json::json!(prior_id);
    let corrupted = records
        .into_iter()
        .map(|record| serde_json::to_string(&record).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, corrupted).unwrap();
    assert!(reopen(&path).is_err());
}

#[tokio::test]
async fn competing_replacements_admit_one_owner_and_restore_prior_only_when_it_is_abandoned() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("competing-abandoned-login.jsonl");
    let prior_id = "1".repeat(64);
    let first_id = "2".repeat(64);
    let second_id = "3".repeat(64);
    let prior = session(None).await;
    let store = opened(|| reopen(&path));
    store
        .insert(prior_id.clone(), prior.clone(), None, 100)
        .await
        .unwrap();
    store
        .insert(
            first_id.clone(),
            session(None).await,
            Some(prior_id.clone()),
            100,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .insert(
                second_id.clone(),
                session(None).await,
                Some(prior_id.clone()),
                100,
            )
            .await,
        Err(AccessError::InvalidCredential)
    );
    assert!(store.get(prior_id.clone()).await.unwrap().is_none());
    assert!(store.get(first_id.clone()).await.unwrap().is_some());
    assert!(store.get(second_id.clone()).await.unwrap().is_none());

    store
        .abandon_login(
            first_id.clone(),
            Some((prior_id.clone(), prior.clone())),
            100,
        )
        .await
        .unwrap();
    assert!(store.get(first_id).await.unwrap().is_none());
    assert!(store.get(second_id).await.unwrap().is_none());
    assert_eq!(store.get(prior_id.clone()).await.unwrap(), Some(prior));
    drop(store);
    let reopened = opened(|| reopen(&path));
    assert!(reopened.get(prior_id).await.unwrap().is_some());
}
#[tokio::test]
async fn expiry_and_failed_audit_writes_never_report_success_or_resurrect_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.jsonl");
    let id = "b".repeat(64);
    let store = opened(|| reopen(&path));
    store
        .insert(id.clone(), session(None).await, None, 100)
        .await
        .unwrap();
    assert!((ReadBrowserSession { store: &store })
        .execute(&id, 100 + IDLE_SECONDS)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .renew(id.clone(), 100 + IDLE_SECONDS, credential_id())
        .await
        .is_err());
    // Durable bytes are inspected only after releasing exclusive journal ownership.
    drop(store);
    let record: StoredRecord = serde_json::from_str(
        std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert!(matches!(record.changes[0].reason, Reason::IdleExpired));
    assert!(record.changes[0].initiator.is_none());
    for operation in ["insert", "renew", "remove"] {
        let path = dir.path().join(format!("{operation}.jsonl"));
        let store = opened(|| reopen(&path));
        store
            .insert(id.clone(), session(None).await, None, 100)
            .await
            .unwrap();
        // Replace only this test instance's sink with a real read-only file.
        store.0.lock().unwrap().file = Some(open(&path, OpenMode::Read).unwrap());
        let result = match operation {
            "insert" => store
                .insert("c".repeat(64), session(None).await, Some(id.clone()), 100)
                .await
                .map(|_| ()),
            "renew" => store
                .renew(id.clone(), 4000, credential_id())
                .await
                .map(|_| ()),
            _ => {
                store
                    .remove(
                        id.clone(),
                        4000,
                        RemovalReason::SignOut,
                        Some(credential_id()),
                    )
                    .await
            }
        };
        assert_eq!(result, Err(AccessError::Unavailable));
        assert_eq!(store.get(id.clone()).await, Err(AccessError::Unavailable));
    }
}

#[tokio::test]
async fn invalid_journal_correlations_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    for field in ["sequence", "initiator", "deadline"] {
        let path = dir.path().join(format!("{field}.jsonl"));
        let store = opened(|| reopen(&path));
        store
            .insert("a".repeat(64), session(None).await, None, 100)
            .await
            .unwrap();
        drop(store);
        let mut record: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        match field {
            "sequence" => record["sequence"] = serde_json::json!(2),
            "initiator" => record["changes"][0]["initiator"] = serde_json::json!("other"),
            _ => record["changes"][0]["after"]["idle_expires_at"] = serde_json::json!(99999999),
        }
        std::fs::write(&path, format!("{record}\n")).unwrap();
        assert!(reopen(&path).is_err());
    }
}

#[tokio::test]
async fn journal_total_byte_bound_is_exact_durable_and_checked_before_replay() {
    let directory = tempfile::tempdir().unwrap();
    let sample_path = directory.path().join("sample-bound.jsonl");
    let sample = opened(|| reopen(&sample_path));
    sample
        .insert("a".repeat(64), session(None).await, None, 100)
        .await
        .unwrap();
    drop(sample);
    let exact_bound = std::fs::metadata(&sample_path).unwrap().len();

    let path = directory.path().join("bounded.jsonl");
    let store = opened(|| reopen_bounded(&path, exact_bound));
    let id = "b".repeat(64);
    let original = session(None).await;
    store
        .insert(id.clone(), original.clone(), None, 100)
        .await
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), exact_bound);
    assert_eq!(
        store.renew(id.clone(), 3_700, credential_id()).await,
        Err(AccessError::Unavailable)
    );
    assert_eq!(store.get(id.clone()).await.unwrap(), Some(original.clone()));
    drop(store);

    let reopened = opened(|| reopen_bounded(&path, exact_bound));
    assert_eq!(reopened.get(id).await.unwrap(), Some(original));
    drop(reopened);
    assert!(reopen_bounded(&path, exact_bound - 1).is_err());
}

#[tokio::test]
async fn every_automatic_invalidation_retains_its_typed_cause_without_an_initiator() {
    let dir = tempfile::tempdir().unwrap();
    for (error, expected) in [
        (AccessError::CredentialRevoked, Reason::CredentialRevoked),
        (AccessError::CredentialExpired, Reason::CredentialExpired),
        (AccessError::InactiveMembership, Reason::InactiveMembership),
        (AccessError::IdentityMismatch, Reason::IdentityMismatch),
        (AccessError::InvalidCredential, Reason::InvalidCredential),
    ] {
        let path = dir.path().join(format!("{error:?}.jsonl"));
        let store = opened(|| reopen(&path));
        let id = "d".repeat(64);
        store
            .insert(id.clone(), session(None).await, None, 100)
            .await
            .unwrap();
        store
            .remove(id, 101, invalidation_reason(error).unwrap(), None)
            .await
            .unwrap();
        // Durable bytes are inspected only after releasing exclusive journal ownership.
        drop(store);
        let record: StoredRecord = serde_json::from_str(
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(record.changes[0].reason, expected);
        assert!(record.changes[0].initiator.is_none());
        assert!(record.changes[0].before.is_some());
        assert!(record.changes[0].after.is_none());
    }
    assert!(invalidation_reason(AccessError::Unavailable).is_none());
    assert!(invalidation_reason(AccessError::Unsupported).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_journal_work_does_not_block_the_async_runtime() {
    use std::{
        sync::{mpsc, Arc},
        time::Duration,
    };

    let store = Arc::new(PersistentSessions::memory());
    let locked_state = store.0.clone();
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = locked_state.lock().unwrap();
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    locked_rx.recv().unwrap();

    let blocked_store = store.clone();
    let mut blocked = tokio::spawn(async move { blocked_store.get("e".repeat(64)).await });
    tokio::task::yield_now().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), async { 7_u8 })
            .await
            .unwrap(),
        7
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut blocked)
            .await
            .is_err()
    );

    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(blocked.await.unwrap().unwrap().is_none());
}

fn origin_session(now: u64) -> BrowserSession {
    BrowserSession::new(credential_id(), "https://127.0.0.1:1443".into(), now).unwrap()
}
fn last_record(path: &std::path::Path) -> StoredRecord {
    serde_json::from_str(
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_renewal_no_clock_can_vouch_for_is_retired_at_the_next_open() {
    // The journal never constrains a record's own instant, so a renewal written
    // with a forged `at` is indistinguishable from one written honestly. Passing
    // that instant to `renew` produces the exact bytes the forgery would, and
    // chaining it is what would otherwise keep a session alive indefinitely.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("forged-renewal.jsonl");
    let store = opened(|| reopen(&path));
    let id = "a".repeat(64);
    store
        .insert(id.clone(), origin_session(100), None, 100)
        .await
        .unwrap();
    let forged_at = 100 + IDLE_SECONDS - 1;
    let extended = store
        .renew(id.clone(), forged_at, credential_id())
        .await
        .unwrap();
    assert_eq!(extended.idle_expires_at(), forged_at + IDLE_SECONDS);
    drop(store);

    let store = opened(|| PersistentSessions::open(&path, 200));
    assert!(store.get(id.clone()).await.unwrap().is_none());
    drop(store);
    let record = last_record(&path);
    assert_eq!(record.at, 200);
    assert_eq!(record.changes.len(), 1);
    assert_eq!(record.changes[0].reason, Reason::FutureRenewal);
    assert!(record.changes[0].initiator.is_none());
    assert!(record.changes[0].before.is_some());
    assert!(record.changes[0].after.is_none());

    // The retirement is a fact about the record, not about the clock that wrote
    // it: replaying it under the forged reading reaches the same verdict.
    let store = opened(|| PersistentSessions::open(&path, forged_at + IDLE_SECONDS));
    assert!(store.get(id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_forward_clock_excursion_costs_a_sign_in_and_never_wedges_the_store() {
    // The failure the reverted ordering guard introduced: an instant the clock
    // later disagrees with must not leave the store unable to write. Sign-out
    // in particular has to keep working.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("clock-excursion.jsonl");
    let excursion = 10_000_000;
    let store = opened(|| reopen(&path));
    let stranded = "b".repeat(64);
    store
        .insert(stranded.clone(), origin_session(excursion), None, excursion)
        .await
        .unwrap();
    drop(store);

    let store = opened(|| PersistentSessions::open(&path, 100));
    assert!(store.get(stranded).await.unwrap().is_none());
    let fresh = "c".repeat(64);
    store
        .insert(fresh.clone(), origin_session(100), None, 100)
        .await
        .unwrap();
    store
        .remove(
            fresh.clone(),
            101,
            RemovalReason::SignOut,
            Some(credential_id()),
        )
        .await
        .unwrap();
    assert!(store.get(fresh).await.unwrap().is_none());
}

#[tokio::test]
async fn renewals_within_the_tolerance_survive_and_the_next_second_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let renewed_at = 100_000;
    for (now, survives) in [
        (renewed_at - FUTURE_TOLERANCE_SECONDS, true),
        (renewed_at - FUTURE_TOLERANCE_SECONDS - 1, false),
    ] {
        let path = directory.path().join(format!("tolerance-{now}.jsonl"));
        let store = opened(|| reopen(&path));
        let id = "e".repeat(64);
        store
            .insert(id.clone(), origin_session(renewed_at), None, renewed_at)
            .await
            .unwrap();
        drop(store);
        let store = opened(|| PersistentSessions::open(&path, now));
        assert_eq!(store.get(id).await.unwrap().is_some(), survives);
    }
}

#[tokio::test]
async fn a_login_cannot_replace_a_prior_whose_renewal_the_record_precedes() {
    // Why restoration needs no plausibility rule of its own. A prior can only
    // be remembered by the record that replaced it, and that record carried it
    // as a `before`, so it could not precede the prior's own renewal. Any later
    // record restoring it cannot precede that one either. The sweep at `open`
    // is therefore the only place a clock has to be consulted.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preceded-prior.jsonl");
    let store = opened(|| reopen(&path));
    let prior_id = "f".repeat(64);
    let excursion = 10_000_000;
    store
        .insert(prior_id.clone(), origin_session(excursion), None, excursion)
        .await
        .unwrap();
    assert_eq!(
        store
            .insert(
                "9".repeat(64),
                origin_session(100),
                Some(prior_id.clone()),
                100,
            )
            .await,
        Err(AccessError::Unavailable)
    );
    drop(store);

    // And the prior itself does not survive a clock that disagrees with it.
    let store = opened(|| PersistentSessions::open(&path, 100));
    assert!(store.get(prior_id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_journal_at_its_bound_cannot_retire_implausible_state_and_fails_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounded-sweep.jsonl");
    let excursion = 10_000_000;
    let store = opened(|| reopen(&path));
    store
        .insert("7".repeat(64), origin_session(excursion), None, excursion)
        .await
        .unwrap();
    drop(store);
    let exact_bound = std::fs::metadata(&path).unwrap().len();

    assert!(PersistentSessions::open_bounded(&path, exact_bound, 100).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), exact_bound);
    opened(|| PersistentSessions::open_bounded(&path, exact_bound, excursion));
}
