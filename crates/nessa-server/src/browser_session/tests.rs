use super::*;
use crate::browser_session::{
    adapters::{MemorySessions, PersistentSessions},
    entrypoint,
};
use crate::browser_session::{
    application::{BrowserSession, SessionStore},
    domain::value_objects::RemovalReason,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    Json,
};
use tokio::sync::{Notify, Semaphore};

fn headers() -> HeaderMap {
    let mut value = HeaderMap::new();
    value.insert(header::ORIGIN, "https://127.0.0.1:1443".parse().unwrap());
    value.insert("x-nessa-browser", "1".parse().unwrap());
    value
}
fn with_cookie(value: &str) -> HeaderMap {
    let mut headers = headers();
    headers.insert(
        header::COOKIE,
        value.split(';').next().unwrap().parse().unwrap(),
    );
    headers
}

fn set_browser_sessions(
    state: &mut ProductRouteState,
    store: Arc<dyn SessionStore>,
) {
    state.browser_sessions = Some(store);
}

#[test]
fn browser_session_id_encoding_preserves_all_256_random_bits() {
    assert_eq!(entrypoint::encode_session_id([0; 32]), "0".repeat(64));
    assert_eq!(entrypoint::encode_session_id([u8::MAX; 32]), "f".repeat(64));
}

#[tokio::test]
async fn browser_login_cookie_restore_origin_binding_logout_and_expiry() {
    let (state, clock) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let login = || Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
    let response = entrypoint::login(State(state.clone()), headers(), login()).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    for flag in [
        "HttpOnly",
        "Secure",
        "SameSite=Strict",
        "Path=/",
        "Max-Age=2592000",
    ] {
        assert!(cookie.contains(flag));
    }
    assert!(!cookie.contains("secret"));
    let mut h = headers();
    h.insert(
        header::COOKIE,
        cookie.split(';').next().unwrap().parse().unwrap(),
    );
    assert_eq!(
        entrypoint::check(State(state.clone()), h.clone())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let mut foreign = h.clone();
    foreign.insert(header::ORIGIN, "https://127.0.0.1:1555".parse().unwrap());
    assert_eq!(
        entrypoint::check(State(state.clone()), foreign)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let identity = authenticate(&state).await;
    let mut socket_state = state.clone();
    socket_state.browser_session_id = entrypoint::cookie(&h).map(str::to_owned);
    assert!(current_snapshot(&socket_state, &identity).await.is_ok());
    assert_eq!(
        entrypoint::logout(State(state.clone()), h.clone())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(current_snapshot(&socket_state, &identity).await.is_err());
    assert_eq!(
        entrypoint::check(State(state.clone()), h.clone())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = entrypoint::login(State(state.clone()), headers(), login()).await;
    h.insert(
        header::COOKIE,
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .parse()
            .unwrap(),
    );
    clock.now.store(200, Ordering::SeqCst);
    assert_eq!(
        entrypoint::check(State(state), h).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn browser_session_reads_current_membership_role_grants_and_revision() {
    let (state, authority) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let response = entrypoint::login(
        State(state.clone()),
        headers(),
        Json(serde_json::from_value(json!({"token":"secret"})).unwrap()),
    )
    .await;
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let identity = authenticate(&state).await;
    let mut socket_state = state;
    socket_state.browser_session_id = entrypoint::cookie(&with_cookie(cookie)).map(str::to_owned);

    let mut updated = snapshot(MembershipRole::Member, MembershipStatus::Active);
    updated.revision = 7;
    updated.credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        Some(200),
        vec![Grant::new(
            Action::new("server.read").unwrap(),
            Resource::new(
                OrganizationId::new("organization").unwrap(),
                ResourceId::new("gateway-resource").unwrap(),
            ),
        )],
    )
    .unwrap();
    updated.membership = Membership::new(
        MembershipId::new("replacement-membership").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        MembershipRole::Member,
        MembershipStatus::Active,
    );
    *authority.snapshot.lock().unwrap() = updated;

    let (current_identity, current) = current_identity(&socket_state, &identity).await.unwrap();
    assert_eq!(current.revision, 7);
    assert_eq!(current.membership.role(), MembershipRole::Member);
    assert_eq!(
        current_identity.context().membership_id().as_str(),
        "replacement-membership"
    );
    assert_eq!(current.credential.grants().len(), 1);
    let OutgoingMessage::Response(response) =
        dispatch(&socket_state, &identity, request("current", "auth.session")).await
    else {
        panic!("response expected")
    };
    assert_eq!(
        response.payload.unwrap()["membershipId"],
        "replacement-membership"
    );
    assert!(dispatch(
        &socket_state,
        &identity,
        request("authorized", "server.health"),
    )
    .await
    .is_success());
}

#[tokio::test]
async fn minimal_session_survives_restart_and_rejects_malformed_credential_ids() {
    let directory = tempfile::tempdir().unwrap();
    let sessions_path = directory.path().join("browser-sessions.jsonl");
    let (state, _) = fixture(MembershipRole::Admin);
    let credential_id = authenticate(&state).await.context().credential_id().clone();
    let id = "8".repeat(64);
    let store = PersistentSessions::open(&sessions_path).unwrap();
    store
        .insert(
            id.clone(),
            BrowserSession::new(credential_id.clone(), "https://127.0.0.1:1443".into(), 100)
                .unwrap(),
            None,
            100,
        )
        .await
        .unwrap();
    drop(store);

    let store = PersistentSessions::open(&sessions_path).unwrap();
    let restored = store.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(restored.credential_id(), &credential_id);
    assert!((ReadCurrentSession {
        access: state.access.as_ref(),
        clock: state.clock.as_ref(),
    })
    .resolve(restored.credential_id(), &state.audience)
    .await
    .is_ok());
    drop(store);

    let mut record: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&sessions_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let stored = &record["changes"][0]["after"];
    assert_eq!(stored.as_object().unwrap().len(), 5);
    for copied in ["principal_id", "organization_id", "membership_id", "audience_id", "auth_revision"] {
        assert!(stored.get(copied).is_none());
    }
    record["changes"][0]["after"]["credential_id"] = json!(" credential");
    std::fs::write(&sessions_path, format!("{record}\n")).unwrap();
    assert!(PersistentSessions::open(&sessions_path).is_err());
}

#[tokio::test]
async fn concurrent_renewals_return_the_authoritative_current_session() {
    let (state, _) = fixture(MembershipRole::Admin);
    let credential_id = authenticate(&state).await.context().credential_id().clone();
    let store = MemorySessions::default();
    let id = "7".repeat(64);
    store
        .insert(
            id.clone(),
            BrowserSession::new(
                credential_id.clone(),
                "https://127.0.0.1:1443".into(),
                100,
            )
            .unwrap(),
            None,
            100,
        )
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        store.renew(id.clone(), 3_700, credential_id.clone()),
        store.renew(id.clone(), 3_701, credential_id),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    let current = store.get(id).await.unwrap().unwrap();
    assert!([3_700, 3_701].contains(&current.renewed_at()));
    assert!([3_700, 3_701].contains(&first.renewed_at()));
    assert!([3_700, 3_701].contains(&second.renewed_at()));
}

#[tokio::test]
async fn browser_rejects_csrf_http_and_invalid_credentials() {
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    for origin in ["http://127.0.0.1:1443", "https://evil.example", "null"] {
        let mut h = headers();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
        assert_eq!(
            entrypoint::login(State(state.clone()), h, body)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut h = headers();
    h.remove("x-nessa-browser");
    assert_eq!(
        entrypoint::check(State(state.clone()), h).await.status(),
        StatusCode::FORBIDDEN
    );
    let body = Json(serde_json::from_value(json!({"token":"wrong"})).unwrap());
    assert_eq!(
        entrypoint::login(State(state), headers(), body)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn full_store_atomically_rotates_the_presented_same_origin_session() {
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let mut first_cookie = None;
    for index in 0..128 {
        let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
        let response = entrypoint::login(State(state.clone()), headers(), body).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "login {index}");
        if first_cookie.is_none() {
            first_cookie = Some(
                response.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
    }
    let old = first_cookie.unwrap();
    let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
    let rotated = entrypoint::login(State(state.clone()), with_cookie(&old), body).await;
    assert_eq!(rotated.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        entrypoint::check(State(state.clone()), with_cookie(&old))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let fresh = rotated.headers()[header::SET_COOKIE].to_str().unwrap();
    assert_eq!(
        entrypoint::check(State(state), with_cookie(fresh))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

struct FailingAccess(AccessError);
impl AccessReader for FailingAccess {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        let error = self.0;
        Box::pin(async move { Err(error) })
    }
}
struct PendingAccess;
impl AccessReader for PendingAccess {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(std::future::pending())
    }
}
struct PendingStore;
impl SessionStore for PendingStore {
    fn insert<'a>(
        &'a self,
        _: String,
        _: BrowserSession,
        _: Option<String>,
        _: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSession)>> {
        Box::pin(std::future::pending())
    }
    fn get<'a>(&'a self, _: String) -> PortFuture<'a, Option<BrowserSession>> {
        Box::pin(std::future::pending())
    }
    fn remove<'a>(
        &'a self,
        _: String,
        _: u64,
        _: RemovalReason,
        _: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        Box::pin(std::future::pending())
    }
    fn renew<'a>(
        &'a self,
        _: String,
        _: u64,
        _: CredentialId,
    ) -> PortFuture<'a, BrowserSession> {
        Box::pin(std::future::pending())
    }
    fn abandon_login<'a>(
        &'a self,
        _: String,
        _: Option<(String, BrowserSession)>,
        _: u64,
    ) -> PortFuture<'a, ()> {
        Box::pin(std::future::pending())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GatedMutation {
    Insert,
    Renew,
    Remove,
}

struct GatedStore {
    inner: MemorySessions,
    mutation: GatedMutation,
    committed: Notify,
    release: Semaphore,
    inserted_id: Mutex<Option<String>>,
    removals: Mutex<Vec<RemovalReason>>,
}
impl GatedStore {
    fn new(mutation: GatedMutation) -> Self {
        Self {
            inner: MemorySessions::default(),
            mutation,
            committed: Notify::new(),
            release: Semaphore::new(0),
            inserted_id: Mutex::new(None),
            removals: Mutex::new(Vec::new()),
        }
    }
    async fn hold_after_commit(&self, mutation: GatedMutation) {
        if self.mutation == mutation {
            self.committed.notify_one();
            let _permit = self.release.acquire().await.unwrap();
        }
    }
}
impl SessionStore for GatedStore {
    fn insert<'a>(
        &'a self,
        id: String,
        session: BrowserSession,
        prior: Option<String>,
        now: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSession)>> {
        Box::pin(async move {
            let replaced = self
                .inner
                .insert(id.clone(), session, prior, now)
                .await?;
            *self.inserted_id.lock().unwrap() = Some(id);
            self.hold_after_commit(GatedMutation::Insert).await;
            Ok(replaced)
        })
    }
    fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSession>> {
        self.inner.get(id)
    }
    fn remove<'a>(
        &'a self,
        id: String,
        now: u64,
        reason: RemovalReason,
        initiator: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.inner.remove(id, now, reason, initiator).await?;
            self.removals.lock().unwrap().push(reason);
            self.hold_after_commit(GatedMutation::Remove).await;
            Ok(())
        })
    }
    fn renew<'a>(
        &'a self,
        id: String,
        now: u64,
        initiator: CredentialId,
    ) -> PortFuture<'a, BrowserSession> {
        Box::pin(async move {
            let renewed = self.inner.renew(id, now, initiator).await?;
            self.hold_after_commit(GatedMutation::Renew).await;
            Ok(renewed)
        })
    }
    fn abandon_login<'a>(
        &'a self,
        id: String,
        prior: Option<(String, BrowserSession)>,
        now: u64,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.inner.abandon_login(id, prior, now).await?;
            self.removals
                .lock()
                .unwrap()
                .push(RemovalReason::AbandonedLogin);
            Ok(())
        })
    }
}

async fn wait_for_removal(store: &GatedStore, reason: RemovalReason) {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if store.removals.lock().unwrap().contains(&reason) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn timed_out_login_is_reclaimed_after_the_committed_insert_finishes() {
    let (mut state, _) = fixture(MembershipRole::Admin);
    state.settings.handshake_timeout = std::time::Duration::from_millis(1);
    let store = Arc::new(GatedStore::new(GatedMutation::Insert));
    set_browser_sessions(&mut state, store.clone());
    let login = tokio::spawn(entrypoint::login(
        State(state),
        headers(),
        Json(serde_json::from_value(json!({"token":"secret"})).unwrap()),
    ));
    store.committed.notified().await;
    assert_eq!(login.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    let id = store.inserted_id.lock().unwrap().clone().unwrap();
    assert!(store.inner.get(id.clone()).await.unwrap().is_some());
    store.release.add_permits(1);
    wait_for_removal(&store, RemovalReason::AbandonedLogin).await;
    assert!(store.inner.get(id).await.unwrap().is_none());
}

#[tokio::test]
async fn timed_out_replacement_login_atomically_restores_the_prior_cookie() {
    let (mut state, _) = fixture(MembershipRole::Admin);
    state.settings.handshake_timeout = std::time::Duration::from_millis(1);
    let store = Arc::new(GatedStore::new(GatedMutation::Insert));
    let prior_id = "6".repeat(64);
    let prior = BrowserSession::new(
        authenticate(&state).await.context().credential_id().clone(),
        "https://127.0.0.1:1443".into(),
        100,
    )
    .unwrap();
    store
        .inner
        .insert(prior_id.clone(), prior, None, 100)
        .await
        .unwrap();
    set_browser_sessions(&mut state, store.clone());
    let prior_cookie = format!("__Host-nessa-session={prior_id}");
    let login = tokio::spawn(entrypoint::login(
        State(state.clone()),
        with_cookie(&prior_cookie),
        Json(serde_json::from_value(json!({"token":"secret"})).unwrap()),
    ));
    store.committed.notified().await;
    assert_eq!(login.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(store.inner.get(prior_id.clone()).await.unwrap().is_none());
    let replacement = store.inserted_id.lock().unwrap().clone().unwrap();
    store.release.add_permits(1);
    wait_for_removal(&store, RemovalReason::AbandonedLogin).await;
    assert!(store.inner.get(replacement).await.unwrap().is_none());
    assert!(store.inner.get(prior_id).await.unwrap().is_some());
    let mut state = state;
    state.settings.handshake_timeout = std::time::Duration::from_secs(1);
    assert_eq!(
        entrypoint::check(State(state), with_cookie(&prior_cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn timed_out_logout_finishes_the_exact_signout_transition() {
    let (state, _) = fixture(MembershipRole::Admin);
    let store = Arc::new(GatedStore::new(GatedMutation::Remove));
    let state = state.with_browser_sessions(store.clone());
    let login = entrypoint::login(
        State(state.clone()),
        headers(),
        Json(serde_json::from_value(json!({"token":"secret"})).unwrap()),
    )
    .await;
    let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap().to_owned();
    let id = entrypoint::cookie(&with_cookie(&cookie)).unwrap().to_owned();
    let mut timed = state;
    timed.settings.handshake_timeout = std::time::Duration::from_millis(1);
    let logout = tokio::spawn(entrypoint::logout(State(timed), with_cookie(&cookie)));
    store.committed.notified().await;
    assert_eq!(logout.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(store.inner.get(id).await.unwrap().is_none());
    store.release.add_permits(1);
    wait_for_removal(&store, RemovalReason::SignOut).await;
}

#[tokio::test]
async fn timed_out_check_finishes_the_exact_renewal_transition() {
    let (mut state, authority) = fixture(MembershipRole::Admin);
    let store = Arc::new(GatedStore::new(GatedMutation::Renew));
    let id = "7".repeat(64);
    *authority.proof_expires_at.lock().unwrap() = None;
    let membership = authority.snapshot.lock().unwrap().membership.clone();
    authority.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        None,
        vec![],
    )
    .unwrap();
    authority.snapshot.lock().unwrap().membership = membership;
    let credential_id = authenticate(&state).await.context().credential_id().clone();
    store
        .inner
        .insert(
            id.clone(),
            BrowserSession::new(
                credential_id,
                "https://127.0.0.1:1443".into(),
                100,
            )
            .unwrap(),
            None,
            100,
        )
        .await
        .unwrap();
    set_browser_sessions(&mut state, store.clone());
    state.settings.handshake_timeout = std::time::Duration::from_millis(1);
    authority.now.store(3_700, Ordering::SeqCst);
    let cookie = format!("__Host-nessa-session={id}");
    let check = tokio::spawn(entrypoint::check(State(state), with_cookie(&cookie)));
    store.committed.notified().await;
    assert_eq!(check.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        store.inner.get(id).await.unwrap().unwrap().renewed_at(),
        3_700
    );
    store.release.add_permits(1);
}

#[tokio::test]
async fn check_preserves_session_on_unavailable_and_reclaims_confirmed_invalid() {
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
    let login = entrypoint::login(State(state.clone()), headers(), body).await;
    let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();

    let mut unavailable = state.clone();
    unavailable.access = Arc::new(FailingAccess(AccessError::Unavailable));
    assert_eq!(
        entrypoint::check(State(unavailable), with_cookie(cookie))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        entrypoint::check(State(state.clone()), with_cookie(cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let mut revoked = state.clone();
    revoked.access = Arc::new(FailingAccess(AccessError::CredentialRevoked));
    assert_eq!(
        entrypoint::check(State(revoked), with_cookie(cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        entrypoint::check(State(state), with_cookie(cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn logout_resolves_current_authority_before_attributing_sign_out() {
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let login = entrypoint::login(
        State(state.clone()),
        headers(),
        Json(serde_json::from_value(json!({"token":"secret"})).unwrap()),
    )
    .await;
    let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();

    let mut unavailable = state.clone();
    unavailable.access = Arc::new(FailingAccess(AccessError::Unavailable));
    assert_eq!(
        entrypoint::logout(State(unavailable), with_cookie(cookie))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        entrypoint::check(State(state.clone()), with_cookie(cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let mut revoked = state.clone();
    revoked.access = Arc::new(FailingAccess(AccessError::CredentialRevoked));
    assert_eq!(
        entrypoint::logout(State(revoked), with_cookie(cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        entrypoint::check(State(state), with_cookie(cookie))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn check_returns_service_unavailable_when_admission_or_deadline_is_exhausted() {
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
    let login = entrypoint::login(State(state.clone()), headers(), body).await;
    let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
    let permits = state.requests.clone().acquire_many_owned(128).await.unwrap();
    assert_eq!(
        entrypoint::check(State(state.clone()), with_cookie(cookie))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(permits);
    let mut stalled = state.clone();
    stalled.settings.handshake_timeout = std::time::Duration::from_millis(10);
    stalled.browser_sessions = Some(Arc::new(PendingStore));
    let stalled_check = tokio::spawn(entrypoint::check(
        State(stalled.clone()),
        with_cookie(cookie),
    ));
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            entrypoint::check(State(state.clone()), with_cookie(cookie)),
        )
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(stalled_check.await.unwrap().status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
    assert_eq!(
        entrypoint::login(State(stalled.clone()), headers(), body)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        entrypoint::logout(State(stalled), with_cookie(cookie))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut elapsed = state;
    elapsed.settings.handshake_timeout = std::time::Duration::ZERO;
    elapsed.access = Arc::new(PendingAccess);
    assert_eq!(
        entrypoint::check(State(elapsed), with_cookie(cookie))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn development_http_cookie_is_separate_and_origin_bound() {
    let (mut state, _) = fixture(MembershipRole::Admin);
    state.browser_http_allowed = true;
    let state = state.with_browser_sessions(Arc::new(MemorySessions::default()));
    for origin in ["http://127.0.0.1:1420", "http://[::1]:1420"] {
        let mut h = headers();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
        let login = entrypoint::login(State(state.clone()), h.clone(), body).await;
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let cookie = login.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with("nessa-local-session="));
        assert!(!cookie.contains("Secure"));
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        h.insert(header::COOKIE, cookie.split(';').next().unwrap().parse().unwrap());
        assert_eq!(entrypoint::check(State(state.clone()), h.clone()).await.status(), StatusCode::NO_CONTENT);
        let mut secure = h.clone();
        secure.insert(header::ORIGIN, origin.replacen("http:", "https:", 1).parse().unwrap());
        assert_eq!(entrypoint::check(State(state.clone()), secure).await.status(), StatusCode::UNAUTHORIZED);
        let mut disabled = state.clone();
        disabled.browser_http_allowed = false;
        assert_eq!(entrypoint::check(State(disabled), h.clone()).await.status(), StatusCode::FORBIDDEN);
        let logout = entrypoint::logout(State(state.clone()), h.clone()).await;
        assert_eq!(logout.status(), StatusCode::NO_CONTENT);
        assert!(logout.headers()[header::SET_COOKIE].to_str().unwrap().starts_with("nessa-local-session=;"));
        assert_eq!(entrypoint::check(State(state.clone()), h).await.status(), StatusCode::UNAUTHORIZED);
    }
    for origin in ["http://localhost:1420", "http://192.168.1.2:1420", "http://127.0.0.1.evil.example", "http://127.0.0.1:1420@evil.example", "null"] {
        let mut h = headers();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        let body = Json(serde_json::from_value(json!({"token":"secret"})).unwrap());
        assert_eq!(entrypoint::login(State(state.clone()), h, body).await.status(), StatusCode::FORBIDDEN);
    }
}

struct HostileStore {
    session: BrowserSession,
    removals: Mutex<Vec<RemovalReason>>,
}

struct RenewingStore {
    session: BrowserSession,
    renewals: Mutex<Vec<u64>>,
}
impl SessionStore for RenewingStore {
    fn insert<'a>(
        &'a self,
        _: String,
        _: BrowserSession,
        _: Option<String>,
        _: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSession)>> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
    fn get<'a>(&'a self, _: String) -> PortFuture<'a, Option<BrowserSession>> {
        Box::pin(async { Ok(Some(self.session.clone())) })
    }
    fn remove<'a>(
        &'a self,
        _: String,
        _: u64,
        _: RemovalReason,
        _: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
    fn renew<'a>(
        &'a self,
        _: String,
        now: u64,
        _: CredentialId,
    ) -> PortFuture<'a, BrowserSession> {
        Box::pin(async move {
            self.renewals.lock().unwrap().push(now);
            self.session
                .renewed_for_check(now)
                .ok_or(AccessError::InvalidCredential)
        })
    }
    fn abandon_login<'a>(
        &'a self,
        _: String,
        _: Option<(String, BrowserSession)>,
        _: u64,
    ) -> PortFuture<'a, ()> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
}

struct AdvancingClock(AtomicU64);
impl Clock for AdvancingClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) * 1000
    }
}

#[tokio::test]
async fn check_uses_one_time_sample_for_expected_and_persisted_renewal() {
    let (mut state, authority) = fixture(MembershipRole::Admin);
    *authority.proof_expires_at.lock().unwrap() = None;
    let current = snapshot(MembershipRole::Admin, MembershipStatus::Active);
    let context = current.membership.clone();
    authority.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        None,
        vec![],
    )
    .unwrap();
    authority.snapshot.lock().unwrap().membership = context;
    let credential_id = authenticate(&state).await.context().credential_id().clone();
    let store = Arc::new(RenewingStore {
        session: BrowserSession::new(
            credential_id,
            "https://127.0.0.1:1443".into(),
            100,
        )
        .unwrap(),
        renewals: Mutex::new(vec![]),
    });
    state.clock = Arc::new(AdvancingClock(AtomicU64::new(3_700)));
    set_browser_sessions(&mut state, store.clone());
    let cookie = format!("__Host-nessa-session={}", "a".repeat(64));

    assert_eq!(
        entrypoint::check(State(state), with_cookie(&cookie))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(store.renewals.lock().unwrap().len(), 1);
}
impl SessionStore for HostileStore {
    fn insert<'a>(
        &'a self,
        _: String,
        _: BrowserSession,
        _: Option<String>,
        _: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSession)>> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
    fn get<'a>(&'a self, _: String) -> PortFuture<'a, Option<BrowserSession>> {
        Box::pin(async { Ok(Some(self.session.clone())) })
    }
    fn remove<'a>(
        &'a self,
        _: String,
        _: u64,
        reason: RemovalReason,
        _: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.removals.lock().unwrap().push(reason);
            Ok(())
        })
    }
    fn renew<'a>(
        &'a self,
        _: String,
        _: u64,
        _: CredentialId,
    ) -> PortFuture<'a, BrowserSession> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
    fn abandon_login<'a>(
        &'a self,
        _: String,
        _: Option<(String, BrowserSession)>,
        _: u64,
    ) -> PortFuture<'a, ()> {
        Box::pin(async { Err(AccessError::Unsupported) })
    }
}

#[tokio::test]
async fn current_identity_failures_keep_distinct_automatic_removal_causes() {
    for (error, expected) in [
        (AccessError::CredentialRevoked, RemovalReason::CredentialRevoked),
        (AccessError::CredentialExpired, RemovalReason::CredentialExpired),
        (AccessError::InactiveMembership, RemovalReason::InactiveMembership),
        (AccessError::IdentityMismatch, RemovalReason::IdentityMismatch),
        (AccessError::InvalidCredential, RemovalReason::InvalidCredential),
    ] {
        let (mut state, _) = fixture(MembershipRole::Admin);
        let identity = authenticate(&state).await;
        let store = Arc::new(HostileStore {
            session: BrowserSession::new(
                identity.context().credential_id().clone(),
                "https://127.0.0.1:1443".into(),
                100,
            )
            .unwrap(),
            removals: Mutex::new(vec![]),
        });
        set_browser_sessions(&mut state, store.clone());
        state.access = Arc::new(FailingAccess(error));
        let cookie = format!("__Host-nessa-session={}", "a".repeat(64));

        assert_eq!(
            entrypoint::check(State(state), with_cookie(&cookie))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(*store.removals.lock().unwrap(), vec![expected]);
    }
}

#[tokio::test]
async fn logout_attributes_only_current_verified_authority_as_explicit() {
    for (error, expected, status) in [
        (None, Some(RemovalReason::SignOut), StatusCode::NO_CONTENT),
        (
            Some(AccessError::CredentialRevoked),
            Some(RemovalReason::CredentialRevoked),
            StatusCode::NO_CONTENT,
        ),
        (
            Some(AccessError::CredentialExpired),
            Some(RemovalReason::CredentialExpired),
            StatusCode::NO_CONTENT,
        ),
        (
            Some(AccessError::InactiveMembership),
            Some(RemovalReason::InactiveMembership),
            StatusCode::NO_CONTENT,
        ),
        (
            Some(AccessError::IdentityMismatch),
            Some(RemovalReason::IdentityMismatch),
            StatusCode::NO_CONTENT,
        ),
        (
            Some(AccessError::InvalidCredential),
            Some(RemovalReason::InvalidCredential),
            StatusCode::NO_CONTENT,
        ),
        (
            Some(AccessError::Unavailable),
            None,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (mut state, _) = fixture(MembershipRole::Admin);
        let identity = authenticate(&state).await;
        let store = Arc::new(HostileStore {
            session: BrowserSession::new(
                identity.context().credential_id().clone(),
                "https://127.0.0.1:1443".into(),
                100,
            )
            .unwrap(),
            removals: Mutex::new(vec![]),
        });
        set_browser_sessions(&mut state, store.clone());
        if let Some(error) = error {
            state.access = Arc::new(FailingAccess(error));
        }
        let cookie = format!("__Host-nessa-session={}", "a".repeat(64));

        assert_eq!(
            entrypoint::logout(State(state), with_cookie(&cookie))
                .await
                .status(),
            status
        );
        assert_eq!(store.removals.lock().unwrap().first().copied(), expected);
    }
}

#[tokio::test]
async fn hostile_store_cannot_restore_an_expired_domain_session() {
    let (mut state, clock) = fixture(MembershipRole::Admin);
    *clock.proof_expires_at.lock().unwrap() = None;
    clock.snapshot.lock().unwrap().credential = Credential::new(
        CredentialId::new("credential").unwrap(),
        PrincipalId::new("principal").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        100,
        None,
        vec![],
    )
    .unwrap();
    let identity = authenticate(&state).await;
    let expired = BrowserSession::restore(
        identity.context().credential_id().clone(),
        "https://127.0.0.1:1443".into(),
        100,
        100,
        100 + crate::browser_session::domain::value_objects::IDLE_SECONDS,
    )
    .unwrap();
    let store = Arc::new(HostileStore {
        session: expired,
        removals: Mutex::new(vec![]),
    });
    set_browser_sessions(&mut state, store.clone());
    state.browser_session_id = Some("f".repeat(64));
    clock.now.store(
        100 + crate::browser_session::domain::value_objects::IDLE_SECONDS,
        Ordering::SeqCst,
    );

    assert!(matches!(
        current_snapshot(&state, &identity).await,
        Err(AccessError::InvalidCredential)
    ));
    assert_eq!(
        *store.removals.lock().unwrap(),
        vec![RemovalReason::IdleExpired]
    );
}
