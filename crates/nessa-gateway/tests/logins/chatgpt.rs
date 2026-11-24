//! Codex's ChatGPT accounts: the sign-in (rows S*), the refreshes a lease's
//! hand-off makes (R*), choosing and removing accounts (A*), and the pure
//! pieces of OpenAI's adapter and the callback listener
//! (`docs/design/chatgpt-sign-in.md`).
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nessa_auth::application::ports::Clock;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::application::{AccessRefused, ChatgptLogins, SignInStatus};
use super::domain::{
    AccountId, ChatgptAccount, ChatgptAccounts, RefreshProblem, Secret, SignInEnd, Tokens,
    MAX_ACCOUNTS, PLAN_SCOPE, REGISTRATION_CLIENT,
};
use super::infrastructure::authority::{
    authorize_url, refresh_failure, token_answer, verify_id_token, Answer,
};
use super::infrastructure::callback::{callback_params, request_line_target};
use super::ports::{
    AuthorityFailure, Authorize, Callback, CallbackListener, CallbackReply, CallbackServer,
    ChatgptAuthority, Identity, ListenFailed, RefreshFailure, TokenAnswer,
};
use crate::logins::application::{
    AuditUnavailable, LoginAudit, LoginCaller, LoginEvent, LoginRecord, LoginsError,
};
use crate::secrets::{SecretStore, SecretUnavailable};

const MINUTE: u64 = 60_000;
const START: u64 = 1_800_000_000_000;

// ---- fakes ----

#[derive(Default)]
struct Store {
    value: Mutex<Option<Vec<u8>>>,
    writes_fail: Mutex<bool>,
    locked: Mutex<bool>,
}

impl SecretStore for Store {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        if *self.locked.lock().unwrap() {
            return Err(SecretUnavailable::Locked);
        }
        Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        if *self.writes_fail.lock().unwrap() {
            return Err(SecretUnavailable::Unavailable);
        }
        *self.value.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}

impl Store {
    fn accounts(&self) -> ChatgptAccounts {
        super::vault_entry::decode(self.value.lock().unwrap().as_deref().unwrap()).unwrap()
    }
    fn put(&self, accounts: &ChatgptAccounts) {
        *self.value.lock().unwrap() = Some(super::vault_entry::encode(accounts));
    }
}

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<LoginRecord>>,
    refuses: Mutex<bool>,
}

impl LoginAudit for Audit {
    fn record(&self, record: &LoginRecord) -> Result<(), AuditUnavailable> {
        if *self.refuses.lock().unwrap() {
            return Err(AuditUnavailable);
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}

impl Audit {
    fn events(&self) -> Vec<LoginEvent> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.event.clone())
            .collect()
    }
}

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// OpenAI as a test scripts it.
struct Authority {
    issued: Mutex<String>,
    subject: Mutex<String>,
    scopes: Mutex<Vec<String>>,
    exchange_fails: Mutex<bool>,
    identity_fails: Mutex<bool>,
    refreshes: Mutex<VecDeque<Result<TokenAnswer, RefreshFailure>>>,
    refreshed: AtomicUsize,
    revoked: Mutex<Vec<String>>,
    /// Holds each refresh until a permit is added, to race two leases.
    hold: Option<Arc<tokio::sync::Semaphore>>,
    /// Holds each exchange until a permit is added, counting those held, to
    /// race a cancel or a new sign-in with the end of one.
    exchange_hold: Option<Arc<tokio::sync::Semaphore>>,
    exchanging: AtomicUsize,
    last_authorize: Mutex<Option<String>>,
}

impl Default for Authority {
    fn default() -> Self {
        Self {
            issued: Mutex::new("oaiapp_one".into()),
            subject: Mutex::new("user-1".into()),
            scopes: Mutex::new(vec!["openid".into(), PLAN_SCOPE.into()]),
            exchange_fails: Mutex::new(false),
            identity_fails: Mutex::new(false),
            refreshes: Mutex::new(VecDeque::new()),
            refreshed: AtomicUsize::new(0),
            revoked: Mutex::new(Vec::new()),
            hold: None,
            exchange_hold: None,
            exchanging: AtomicUsize::new(0),
            last_authorize: Mutex::new(None),
        }
    }
}

fn answer(access: &str, refresh: Option<&str>, scopes: &[String]) -> TokenAnswer {
    TokenAnswer {
        access: Secret::new(access.into()),
        refresh: refresh.map(|refresh| Secret::new(refresh.into())),
        id_token: Some(Secret::new("id.token.x".into())),
        expires_in: Duration::from_secs(3600),
        earliest_refresh_at_ms: None,
        scopes: scopes.to_vec(),
    }
}

#[async_trait]
impl ChatgptAuthority for Authority {
    fn authorize_url(&self, request: &Authorize<'_>) -> String {
        let url = format!(
            "https://auth.example/authorize?client_id={}&state={}&register={}",
            request.client_id, request.state, request.register
        );
        *self.last_authorize.lock().unwrap() = Some(url.clone());
        url
    }
    async fn exchange(
        &self,
        _client_id: &str,
        _code: &str,
        _verifier: &str,
        _redirect_uri: &str,
    ) -> Result<TokenAnswer, AuthorityFailure> {
        if let Some(hold) = &self.exchange_hold {
            self.exchanging.fetch_add(1, Ordering::SeqCst);
            hold.acquire().await.unwrap().forget();
        }
        if *self.exchange_fails.lock().unwrap() {
            return Err(AuthorityFailure::Refused);
        }
        Ok(answer(
            "eyJa.eyJb.access-1",
            Some("refresh-1"),
            &self.scopes.lock().unwrap(),
        ))
    }
    async fn refresh(
        &self,
        _client_id: &str,
        _refresh: &Secret,
    ) -> Result<TokenAnswer, RefreshFailure> {
        if let Some(hold) = &self.hold {
            hold.acquire().await.unwrap().forget();
        }
        self.refreshed.fetch_add(1, Ordering::SeqCst);
        self.refreshes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(RefreshFailure::Unreachable))
    }
    async fn revoke(&self, _client_id: &str, refresh: &Secret) -> Result<(), AuthorityFailure> {
        self.revoked.lock().unwrap().push(refresh.expose().into());
        Ok(())
    }
    async fn identity(
        &self,
        _id_token: &Secret,
        _client_id: &str,
        _nonce: &str,
        _now_ms: u64,
    ) -> Result<Identity, AuthorityFailure> {
        if *self.identity_fails.lock().unwrap() {
            return Err(AuthorityFailure::Malformed);
        }
        let subject = self.subject.lock().unwrap().clone();
        Ok(Identity {
            email: Some(format!("{subject}@example.com")),
            subject,
        })
    }
}

/// A callback address the test drives: each `listen` hands its sender out.
#[derive(Default)]
struct Listener {
    servers: Mutex<Vec<mpsc::Sender<Callback>>>,
    fails: Mutex<bool>,
}

struct Server(mpsc::Receiver<Callback>);

#[async_trait]
impl CallbackServer for Server {
    fn redirect_uri(&self) -> String {
        "http://127.0.0.1:1455/auth/callback".into()
    }
    async fn next(&mut self) -> Option<Callback> {
        self.0.recv().await
    }
}

#[async_trait]
impl CallbackListener for Listener {
    async fn listen(&self) -> Result<Box<dyn CallbackServer>, ListenFailed> {
        if *self.fails.lock().unwrap() {
            return Err(ListenFailed);
        }
        let (sender, receiver) = mpsc::channel(4);
        self.servers.lock().unwrap().push(sender);
        Ok(Box::new(Server(receiver)))
    }
}

struct Fixture {
    logins: Arc<ChatgptLogins>,
    store: Arc<Store>,
    audit: Arc<Audit>,
    authority: Arc<Authority>,
    listener: Arc<Listener>,
    time: Arc<Time>,
}

fn fixture_with(authority: Authority) -> Fixture {
    let store = Arc::new(Store::default());
    let audit = Arc::new(Audit::default());
    let authority = Arc::new(authority);
    let listener = Arc::new(Listener::default());
    let time = Arc::new(Time(AtomicU64::new(START)));
    let logins = Arc::new(ChatgptLogins::new(
        store.clone(),
        authority.clone(),
        listener.clone(),
        audit.clone(),
        time.clone(),
    ));
    Fixture {
        logins,
        store,
        audit,
        authority,
        listener,
        time,
    }
}

fn fixture() -> Fixture {
    fixture_with(Authority::default())
}

fn caller() -> LoginCaller {
    LoginCaller {
        organization_id: "org".into(),
        principal_id: "me".into(),
        credential_id: "window".into(),
    }
}

impl Fixture {
    fn state(&self) -> String {
        let url = self
            .authority
            .last_authorize
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        url.split("state=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_owned()
    }

    /// The browser comes back with `params`; answers what it was shown.
    async fn callback(&self, params: &[(&str, &str)]) -> CallbackReply {
        let sender = self
            .listener
            .servers
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let (reply, answer) = oneshot::channel();
        let params: BTreeMap<String, String> = params
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        sender.send(Callback { params, reply }).await.unwrap();
        answer.await.unwrap()
    }

    async fn ended(&self) -> Option<SignInEnd> {
        for _ in 0..200 {
            if let SignInStatus::Idle { last, .. } = self.logins.status().unwrap().sign_in {
                return last;
            }
            tokio::task::yield_now().await;
        }
        panic!("the sign-in did not end");
    }

    /// Sign a new account in as `subject`, all the way.
    async fn sign_in(&self, subject: &str) -> SignInEnd {
        *self.authority.subject.lock().unwrap() = subject.into();
        self.logins.begin(caller(), None).await.unwrap();
        let state = self.state();
        let issued = self.authority.issued.lock().unwrap().clone();
        self.callback(&[("code", "c"), ("state", &state), ("client_id", &issued)])
            .await;
        self.ended().await.unwrap()
    }
}

fn kept_account(id: &str, subject: &str, expires_at_ms: u64) -> ChatgptAccount {
    ChatgptAccount {
        id: AccountId::parse(id).unwrap(),
        email: format!("{subject}@example.com"),
        subject: subject.into(),
        client_id: "oaiapp_one".into(),
        tokens: Some(Tokens {
            access: Secret::new("eyJa.eyJb.old".into()),
            refresh: Secret::new("refresh-old".into()),
            expires_at_ms,
            earliest_refresh_at_ms: None,
        }),
    }
}

const ONE: &str = "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e";

fn with_one(expires_at_ms: u64) -> ChatgptAccounts {
    ChatgptAccounts::new()
        .with_signed_in(kept_account(ONE, "user-1", expires_at_ms))
        .unwrap()
}

// ---- domain ----

#[test]
fn tokens_are_refreshed_below_fifty_minutes_once_openai_allows_it() {
    let tokens = |left: u64, earliest: Option<u64>| Tokens {
        access: Secret::new("a".into()),
        refresh: Secret::new("r".into()),
        expires_at_ms: START + left,
        earliest_refresh_at_ms: earliest,
    };
    assert!(!tokens(50 * MINUTE, None).need_refresh(START));
    assert!(tokens(49 * MINUTE, None).need_refresh(START));
    // R2: not before earliest_refresh_at, unless expired.
    assert!(!tokens(49 * MINUTE, Some(START + 1)).need_refresh(START));
    assert!(tokens(0, Some(START + MINUTE)).need_refresh(START));
    assert_eq!(tokens(60 * MINUTE, None).renew_by_ms(), START + 50 * MINUTE);
}

#[test]
fn accounts_keep_at_most_eight_and_the_first_is_put_in_use() {
    let mut accounts = ChatgptAccounts::new();
    for index in 0..MAX_ACCOUNTS {
        let id = format!("0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d{index:02x}");
        accounts = accounts
            .with_signed_in(kept_account(&id, "u", START))
            .unwrap();
    }
    assert_eq!(accounts.accounts().len(), MAX_ACCOUNTS);
    assert_eq!(
        accounts.in_use().unwrap().id.as_str(),
        "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d00"
    );
    assert!(accounts
        .with_signed_in(kept_account(ONE, "u", START))
        .is_err());
    // With the key chosen, another sign-in leaves the key chosen.
    let keyed = accounts.using(None).unwrap();
    let again = keyed
        .with_signed_in(kept_account(
            "0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d00",
            "u",
            START,
        ))
        .unwrap();
    assert!(again.in_use().is_none());
    // The encoded entry reads back as it was, and a token is not in Debug.
    assert_eq!(
        super::vault_entry::decode(&super::vault_entry::encode(&accounts)).unwrap(),
        accounts
    );
    assert!(!format!("{accounts:?}").contains("eyJa.eyJb.old"));
    assert!(super::vault_entry::decode(b"{}").is_err());
}

// ---- sign-in ----

#[tokio::test]
async fn a_new_account_signs_in_with_its_issued_client_and_is_put_in_use() {
    let f = fixture();
    // S1: requested, and waiting with an authorize URL that registers.
    f.logins.begin(caller(), None).await.unwrap();
    let SignInStatus::Waiting {
        attempt,
        authorize_url,
        account,
    } = f.logins.status().unwrap().sign_in
    else {
        panic!("not waiting");
    };
    // Its end names the same attempt, so the window that started it knows.
    let started = attempt;
    assert!(authorize_url.contains(&format!("client_id={REGISTRATION_CLIENT}")));
    assert!(authorize_url.contains("register=true"));
    assert_eq!(account, None);
    // S3: another attempt's state is answered NotThis, still waiting.
    assert_eq!(
        f.callback(&[
            ("code", "c"),
            ("state", "other"),
            ("client_id", "oaiapp_one")
        ])
        .await,
        CallbackReply::NotThis
    );
    let state = f.state();
    // S12.
    assert_eq!(
        f.callback(&[
            ("code", "c"),
            ("state", &state),
            ("client_id", "oaiapp_one")
        ])
        .await,
        CallbackReply::SignedIn
    );
    assert_eq!(f.ended().await, Some(SignInEnd::SignedIn));
    assert!(matches!(
        f.logins.status().unwrap().sign_in,
        SignInStatus::Idle { attempt: Some(ended), .. } if ended == started
    ));
    let accounts = f.store.accounts();
    let kept = accounts.in_use().unwrap();
    assert_eq!(kept.client_id, "oaiapp_one");
    assert_eq!(kept.email, "user-1@example.com");
    let status = f.logins.status().unwrap();
    assert!(status.accounts[0].in_use && !status.accounts[0].needs_sign_in);
    let events = f.audit.events();
    assert!(matches!(
        events[0],
        LoginEvent::ChatgptSignInRequested { account: None }
    ));
    assert!(matches!(
        events.last(),
        Some(LoginEvent::ChatgptSignInEnded {
            end: SignInEnd::SignedIn,
            ..
        })
    ));
    // No record carries a token.
    let records = format!("{:?}", f.audit.records.lock().unwrap());
    assert!(!records.contains("access-1") && !records.contains("refresh-1"));
}

#[tokio::test]
async fn a_sign_in_that_does_not_pass_every_check_keeps_nothing() {
    // (end expected, how the browser comes back, how OpenAI is scripted)
    /// A case: how it ends, how the fake authority is set up, and what the
    /// browser comes back with.
    type Case = (
        SignInEnd,
        Box<dyn Fn(&Authority)>,
        Vec<(&'static str, &'static str)>,
    );
    let cases: Vec<Case> = vec![
        (
            SignInEnd::Declined,
            Box::new(|_| {}),
            vec![("error", "access_denied")],
        ),
        (SignInEnd::Incomplete, Box::new(|_| {}), vec![("code", "c")]),
        (
            SignInEnd::ExchangeFailed,
            Box::new(|a| *a.exchange_fails.lock().unwrap() = true),
            vec![("code", "c"), ("client_id", "oaiapp_one")],
        ),
        (
            SignInEnd::InvalidIdentity,
            Box::new(|a| *a.identity_fails.lock().unwrap() = true),
            vec![("code", "c"), ("client_id", "oaiapp_one")],
        ),
        (
            SignInEnd::PlanNotEnabled,
            Box::new(|a| *a.scopes.lock().unwrap() = vec!["openid".into()]),
            vec![("code", "c"), ("client_id", "oaiapp_one")],
        ),
    ];
    for (end, script, params) in cases {
        let f = fixture();
        script(&f.authority);
        f.logins.begin(caller(), None).await.unwrap();
        let state = f.state();
        let mut params = params.clone();
        params.push(("state", &state));
        assert_eq!(f.callback(&params).await, CallbackReply::Failed, "{end:?}");
        assert_eq!(f.ended().await, Some(end));
        assert!(f.store.accounts().accounts().is_empty(), "{end:?}");
    }
}

#[tokio::test]
async fn signing_in_again_must_be_the_same_account() {
    let f = fixture();
    f.store.put(&with_one(START + 60 * MINUTE));
    let one = AccountId::parse(ONE).unwrap();
    // S6: another client.
    f.logins.begin(caller(), Some(one.clone())).await.unwrap();
    assert!(f
        .authority
        .last_authorize
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .contains("register=false"));
    let state = f.state();
    f.callback(&[
        ("code", "c"),
        ("state", &state),
        ("client_id", "oaiapp_two"),
    ])
    .await;
    assert_eq!(f.ended().await, Some(SignInEnd::WrongAccount));
    // S9: another subject.
    *f.authority.subject.lock().unwrap() = "someone-else".into();
    f.logins.begin(caller(), Some(one.clone())).await.unwrap();
    let state = f.state();
    f.callback(&[("code", "c"), ("state", &state)]).await;
    assert_eq!(f.ended().await, Some(SignInEnd::WrongAccount));
    // The same subject: its tokens are replaced.
    *f.authority.subject.lock().unwrap() = "user-1".into();
    f.logins.begin(caller(), Some(one.clone())).await.unwrap();
    let state = f.state();
    f.callback(&[("code", "c"), ("state", &state)]).await;
    assert_eq!(f.ended().await, Some(SignInEnd::SignedIn));
    let kept = f.store.accounts();
    assert_eq!(kept.accounts().len(), 1);
    assert_eq!(
        kept.account(&one)
            .unwrap()
            .tokens
            .as_ref()
            .unwrap()
            .access
            .expose(),
        "eyJa.eyJb.access-1"
    );
    // An unknown account is refused before anything starts.
    let unknown = AccountId::parse("1b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e").unwrap();
    assert_eq!(
        f.logins.begin(caller(), Some(unknown)).await,
        Err(LoginsError::UnknownAccount)
    );
}

#[tokio::test]
async fn a_new_sign_in_of_an_account_already_kept_signs_that_one_in_again() {
    let f = fixture();
    assert_eq!(f.sign_in("user-1").await, SignInEnd::SignedIn);
    assert!(f.authority.revoked.lock().unwrap().is_empty());
    assert_eq!(f.sign_in("user-1").await, SignInEnd::SignedIn);
    assert_eq!(f.store.accounts().accounts().len(), 1);
    // The grant it replaced is revoked.
    assert_eq!(*f.authority.revoked.lock().unwrap(), ["refresh-1"]);
    assert_eq!(f.sign_in("user-2").await, SignInEnd::SignedIn);
    let accounts = f.store.accounts();
    assert_eq!(accounts.accounts().len(), 2);
    // The first stays in use.
    assert_eq!(accounts.in_use().unwrap().subject, "user-1");
}

#[tokio::test]
async fn a_sign_in_is_replaced_cancelled_or_never_starts() {
    let f = fixture();
    // S2.
    f.logins.begin(caller(), None).await.unwrap();
    f.logins.begin(caller(), None).await.unwrap();
    let ends: Vec<SignInEnd> = f
        .audit
        .events()
        .into_iter()
        .filter_map(|event| match event {
            LoginEvent::ChatgptSignInEnded { end, .. } => Some(end),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [SignInEnd::Replaced]);
    // S14.
    f.logins.cancel(&caller()).unwrap();
    assert_eq!(f.ended().await, Some(SignInEnd::Cancelled));
    // Nothing listens: ends at once.
    *f.listener.fails.lock().unwrap() = true;
    f.logins.begin(caller(), None).await.unwrap();
    assert_eq!(f.ended().await, Some(SignInEnd::ListenFailed));
    // S1: an audit that refuses starts nothing.
    *f.listener.fails.lock().unwrap() = false;
    *f.audit.refuses.lock().unwrap() = true;
    assert_eq!(
        f.logins.begin(caller(), None).await,
        Err(LoginsError::AuditUnavailable)
    );
    assert!(matches!(
        f.logins.status().unwrap().sign_in,
        SignInStatus::Idle { .. }
    ));
}

#[tokio::test]
async fn too_many_accounts_or_a_vault_that_cannot_keep_it_keeps_nothing() {
    let f = fixture();
    let mut accounts = ChatgptAccounts::new();
    for index in 0..MAX_ACCOUNTS {
        let id = format!("0b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d{index:02x}");
        accounts = accounts
            .with_signed_in(kept_account(&id, &format!("u{index}"), START))
            .unwrap();
    }
    f.store.put(&accounts);
    assert_eq!(f.sign_in("someone-new").await, SignInEnd::TooManyAccounts);
    let f = fixture();
    f.logins.begin(caller(), None).await.unwrap();
    *f.store.writes_fail.lock().unwrap() = true;
    let state = f.state();
    f.callback(&[
        ("code", "c"),
        ("state", &state),
        ("client_id", "oaiapp_one"),
    ])
    .await;
    assert_eq!(f.ended().await, Some(SignInEnd::NotSaved));
}

// ---- refreshes ----

#[tokio::test]
async fn a_token_with_time_left_is_handed_as_it_is() {
    let f = fixture();
    f.store.put(&with_one(START + 55 * MINUTE));
    let access = f.logins.access().await.unwrap().unwrap();
    assert_eq!(access.login.expose(), "eyJa.eyJb.old");
    assert_eq!(access.renew_by_ms, START + 45 * MINUTE);
    assert_eq!(f.authority.refreshed.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_token_near_its_end_is_refreshed_and_saved_before_it_is_handed() {
    let f = fixture();
    f.store.put(&with_one(START + 20 * MINUTE));
    f.authority.refreshes.lock().unwrap().push_back(Ok(answer(
        "eyJa.eyJb.new",
        Some("refresh-new"),
        &[],
    )));
    let access = f.logins.access().await.unwrap().unwrap();
    assert_eq!(access.login.expose(), "eyJa.eyJb.new");
    assert_eq!(access.renew_by_ms, START + 50 * MINUTE);
    let kept = f.store.accounts();
    let tokens = kept.in_use().unwrap().tokens.clone().unwrap();
    assert_eq!(tokens.refresh.expose(), "refresh-new");
    assert!(f
        .audit
        .events()
        .iter()
        .any(|event| matches!(event, LoginEvent::ChatgptRefreshed { .. })));
}

#[tokio::test]
async fn two_leases_at_once_make_one_refresh() {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    let f = fixture_with(Authority {
        hold: Some(hold.clone()),
        ..Authority::default()
    });
    f.store.put(&with_one(START + 20 * MINUTE));
    f.authority.refreshes.lock().unwrap().push_back(Ok(answer(
        "eyJa.eyJb.new",
        Some("refresh-new"),
        &[],
    )));
    let first = tokio::spawn({
        let logins = f.logins.clone();
        async move { logins.access().await }
    });
    let second = tokio::spawn({
        let logins = f.logins.clone();
        async move { logins.access().await }
    });
    tokio::task::yield_now().await;
    hold.add_permits(2);
    let first = first.await.unwrap().unwrap().unwrap();
    let second = second.await.unwrap().unwrap().unwrap();
    assert_eq!(f.authority.refreshed.load(Ordering::SeqCst), 1);
    assert_eq!(first.login.expose(), "eyJa.eyJb.new");
    assert_eq!(second.login.expose(), "eyJa.eyJb.new");
}

#[tokio::test]
async fn a_refused_refresh_token_needs_a_sign_in_and_an_unreachable_one_keeps_its_tokens() {
    let f = fixture();
    f.store.put(&with_one(START + 20 * MINUTE));
    // R6.
    f.authority
        .refreshes
        .lock()
        .unwrap()
        .push_back(Err(RefreshFailure::Unreachable));
    assert_eq!(
        f.logins.access().await,
        Err(AccessRefused::Refresh(RefreshProblem::Unreachable))
    );
    assert!(f.store.accounts().in_use().unwrap().tokens.is_some());
    // R5.
    f.authority
        .refreshes
        .lock()
        .unwrap()
        .push_back(Err(RefreshFailure::Unusable));
    assert_eq!(f.logins.access().await, Err(AccessRefused::SignInNeeded));
    assert!(f.store.accounts().in_use().unwrap().tokens.is_none());
    assert!(f.logins.status().unwrap().accounts[0].needs_sign_in);
    // R9: nothing more is asked of OpenAI for it.
    assert_eq!(f.logins.access().await, Err(AccessRefused::SignInNeeded));
    assert_eq!(f.authority.refreshed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn refreshed_tokens_that_cannot_be_saved_are_not_handed() {
    let f = fixture();
    f.store.put(&with_one(START + 20 * MINUTE));
    f.authority.refreshes.lock().unwrap().push_back(Ok(answer(
        "eyJa.eyJb.new",
        Some("refresh-new"),
        &[],
    )));
    *f.store.writes_fail.lock().unwrap() = true;
    assert_eq!(
        f.logins.access().await,
        Err(AccessRefused::Refresh(RefreshProblem::NotSaved))
    );
}

#[tokio::test]
async fn no_account_in_use_hands_none_and_a_locked_vault_hands_nothing() {
    let f = fixture();
    assert_eq!(f.logins.access().await, Ok(None));
    f.store
        .put(&with_one(START + 55 * MINUTE).using(None).unwrap());
    assert_eq!(f.logins.access().await, Ok(None));
    *f.store.locked.lock().unwrap() = true;
    assert_eq!(f.logins.access().await, Err(AccessRefused::Locked));
}

// ---- choosing and removing ----

#[tokio::test]
async fn an_account_is_chosen_set_aside_and_removed_with_its_token_revoked() {
    let f = fixture();
    f.store.put(&with_one(START + 55 * MINUTE));
    let one = AccountId::parse(ONE).unwrap();
    // A2.
    f.logins.use_account(caller(), None).await.unwrap();
    assert!(!f.logins.in_use().unwrap());
    f.logins
        .use_account(caller(), Some(one.clone()))
        .await
        .unwrap();
    assert!(f.logins.in_use().unwrap());
    let unknown = AccountId::parse("1b7f6c1e-2d3a-4b5c-8d9e-0f1a2b3c4d5e").unwrap();
    assert_eq!(
        f.logins.use_account(caller(), Some(unknown.clone())).await,
        Err(LoginsError::UnknownAccount)
    );
    // A1.
    f.logins.remove(caller(), one.clone()).await.unwrap();
    assert_eq!(*f.authority.revoked.lock().unwrap(), ["refresh-old"]);
    assert!(f.store.accounts().accounts().is_empty());
    assert!(!f.logins.in_use().unwrap());
    assert!(f
        .audit
        .events()
        .iter()
        .any(|event| matches!(event, LoginEvent::ChatgptRemoved { revoked: true, .. })));
    assert_eq!(
        f.logins.remove(caller(), unknown).await,
        Err(LoginsError::UnknownAccount)
    );
    let _ = f.time.0.load(Ordering::SeqCst);
}

// ---- OpenAI's adapter, pure pieces ----

#[test]
fn the_authorize_url_carries_every_parameter_encoded() {
    let base = url::Url::parse("https://auth.openai.com/api/accounts/authorize").unwrap();
    let url = authorize_url(
        &base,
        &Authorize {
            client_id: REGISTRATION_CLIENT,
            register: true,
            host_id: "host one",
            redirect_uri: "http://127.0.0.1:1455/auth/callback",
            state: "s",
            nonce: "n",
            challenge: "c",
            login_hint: Some("me@example.com"),
        },
    );
    let parsed = url::Url::parse(&url).unwrap();
    let query: BTreeMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], REGISTRATION_CLIENT);
    assert_eq!(query["agent_name_hint"], "Nessa");
    assert_eq!(query["ext_agent_host_id"], "host one");
    assert_eq!(query["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["resource"], "https://api.openai.com/v1");
    assert!(query["scope"].contains(PLAN_SCOPE) && query["scope"].contains("offline_access"));
    // The address is shown to every status reader: it carries no token.
    assert!(!query.contains_key("id_token_hint"));
    assert_eq!(query["login_hint"], "me@example.com");
}

#[test]
fn a_token_answer_and_a_refused_refresh_are_read() {
    let read = token_answer(
        json!({
            "access_token": "a", "refresh_token": "r", "id_token": "i",
            "expires_in": 3600, "scope": "openid chatgpt.tokens.use.direct",
            "earliest_refresh_at": "2027-01-15T08:00:00Z"
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(read.expires_in, Duration::from_secs(3600));
    assert_eq!(read.scopes, ["openid", PLAN_SCOPE]);
    assert_eq!(read.earliest_refresh_at_ms, Some(1_800_000_000_000));
    let seconds =
        token_answer(br#"{"access_token":"a","expires_in":60,"earliest_refresh_at":1700000000}"#)
            .unwrap();
    assert_eq!(seconds.earliest_refresh_at_ms, Some(1_700_000_000_000));
    assert!(seconds.refresh.is_none());
    assert!(token_answer(br#"{"access_token":"","expires_in":60}"#).is_none());
    assert!(token_answer(b"not json").is_none());
    let refused = |status: u16, body: &str| {
        refresh_failure(&Answer {
            status,
            body: body.as_bytes().to_vec(),
        })
    };
    assert_eq!(
        refused(400, r#"{"error":"refresh_token_reused"}"#),
        RefreshFailure::Unusable
    );
    assert_eq!(
        refused(401, r#"{"error":{"code":"invalid_client"}}"#),
        RefreshFailure::Unusable
    );
    assert_eq!(
        refused(400, r#"{"error":"slow_down"}"#),
        RefreshFailure::Unreachable
    );
    assert_eq!(
        refused(500, r#"{"error":"invalid_grant"}"#),
        RefreshFailure::Unreachable
    );
}

/// A test signing key: RSA 2048, PKCS#1 DER, and its modulus as a JWK's `n`.
const TEST_KEY: &str = "MIIEowIBAAKCAQEA+zTh12UO417X3b29SoV52IiuFtXZXuAYzqy+cDzoJa37JJkQX1GAksvRrNa9tFF6vw5y341W+O7TOZwWk7OxD+HVz1TVaTdfaLXijpw7Yc7cGrDHXtAOJsShcX1N5cle4m/88wHiHIOFCWeaJeh0c4nhmxTat+GIPA6ypTCBOh++iWC7O8OfEVc0bZAucASXnvc+SG9y1RbDtOI1EeacLeGzat0IsiVbL5sNy6cj3eArbnR/n255OjPD+t16cvoN0zdgjc8JtzROQSUWhNUM0Y7f3GKTpv6H4J1Tpr7CZSzPIJv9TDOOS4n06kv0KZUK00yFOaM7CYBZ+aZeSMkqCQIDAQABAoIBABEM5JRSXWmU3ymDjqTPfPbzxf8ySiP8qWpjwrRLCgW+sphoFDqoBUZRc7ybwboKur5bjHP8e63/RdJ38/dX/7lgfnrpWufelIj+qW89DvHDUOC15X5buE4s0/loRCCH47sSV1SmKNL/IUlDYSzYA4lxDyFPz82hzvWmmL0KFwdbDE6ZZeITOQfkH96JJ0UjWDXsrIZq0hwsuK0zQQ32gFtdouEcbOV/iyqZ+D7XrHyuw3RlH2WJwzKDHU6CDl29NiAPdoZekU27laD4PfsvxEElGouo1RR1zTrdAUR/VQrNbrsJ3fwQP+IoG51ry/heaQkDAFxrid7Cd1THx3/hhsUCgYEA/muZ4XpyV7VW/VoQEVfwpoTC31dcwQzXk+oqyx/Gm84dmbmPfGgzv6NT9ylsBti86qmHt0Ih30O9a338crPRvjjQghODgvmKtBEcGxv/7TKoi79sPyMxEXzd+yZb+zRi3EPag3mHr6Ddsp48v+WluPKrToDqiicekUqwO2mmAG0CgYEA/MQsQYj4IGfGgWwn8Va3G1fvMqMDrBIQuRkXE8emPAh2Q2cS3+ZpyvYeubM6N3y+EiiOH6yNQq+HCAoLYWTMYaiN4i9b0U8vmkyz6RkcSdMp9+41lDiXMhFDhI5zhefqrcKM2Cq9QM96IYyQf4SOihg4vNzIYraYZPK/9wI95o0CgYAWgiJ9Bj4zcEGf7GXbgvIxaTFsXOhB1AcGeQMMMWV6x0NrJDWDhhx96c814J5EBMzB3fm5xsoEXJPXfK/m3e3CbHhV6bAK2FTjFVhtNY7wxziOFINZGp3utCNXYs9kbAALCJ2+X/QXH549fS5Ws9RzhlSNf25AhrF3nSn2N6SOCQKBgQDxs3TORfdepV1ALPgU4uK9Ftz4AsFkLPIozUEv/N2M0lqkIU//IglPtwGyeRvWK0ZlfrpIt2tstQPeQByZyRVW3SKvgrG9+IaAeZAf+Y7/WRjsp7A8R9DsY224t67oANh7x2l+23+VFdCnfA5AobTbgEXg8w36trgK78V4oYAnkQKBgEMs22noUHkbuBOxpTqLnDWdLPueRObRgXyPL1wzxYmWhlGvUVPTu+GUlbYMhXUn2H/He+XTSDBApMK0Hun1WqSPZwbPAGaBg8dIH3ZZotSIEiB9gYYqE+fQanOu4m0B3fZQ0VyDgZrl8KxoGSTg4AXcRjmlTnxrAUAQb7AvqvZz";
const TEST_N: &str = "-zTh12UO417X3b29SoV52IiuFtXZXuAYzqy-cDzoJa37JJkQX1GAksvRrNa9tFF6vw5y341W-O7TOZwWk7OxD-HVz1TVaTdfaLXijpw7Yc7cGrDHXtAOJsShcX1N5cle4m_88wHiHIOFCWeaJeh0c4nhmxTat-GIPA6ypTCBOh--iWC7O8OfEVc0bZAucASXnvc-SG9y1RbDtOI1EeacLeGzat0IsiVbL5sNy6cj3eArbnR_n255OjPD-t16cvoN0zdgjc8JtzROQSUWhNUM0Y7f3GKTpv6H4J1Tpr7CZSzPIJv9TDOOS4n06kv0KZUK00yFOaM7CYBZ-aZeSMkqCQ";

fn id_token(claims: Value, kid: &str) -> String {
    use base64::engine::general_purpose::STANDARD;
    let der = STANDARD.decode(TEST_KEY).unwrap();
    let key = ring::signature::RsaKeyPair::from_der(&der).unwrap();
    let header = URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "kid": kid}).to_string());
    let body = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signed = format!("{header}.{body}");
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        signed.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[test]
fn an_id_token_verifies_only_when_every_claim_and_the_signature_hold() {
    let keys = json!({"keys": [{"kty": "RSA", "kid": "k1", "n": TEST_N, "e": "AQAB"}]});
    let now = 1_800_000_000_000_u64;
    let claims = json!({
        "iss": "https://auth.openai.com", "aud": ["oaiapp_one"], "nonce": "n",
        "exp": now / 1000 + 3600, "sub": "user-1", "email": "me@example.com"
    });
    let verify = |token: &str| {
        verify_id_token(
            token,
            &keys,
            "https://auth.openai.com",
            "oaiapp_one",
            "n",
            now,
        )
    };
    assert_eq!(
        verify(&id_token(claims.clone(), "k1")),
        Some(Identity {
            subject: "user-1".into(),
            email: Some("me@example.com".into())
        })
    );
    for (name, value) in [
        ("iss", json!("https://evil.example")),
        ("aud", json!("oaiapp_two")),
        ("nonce", json!("other")),
        ("exp", json!(now / 1000 - 3600)),
        ("sub", json!("")),
    ] {
        let mut changed = claims.clone();
        changed[name] = value;
        assert_eq!(verify(&id_token(changed, "k1")), None, "{name}");
    }
    // An unknown key, or a body changed after signing.
    assert_eq!(verify(&id_token(claims.clone(), "k2")), None);
    let token = id_token(claims, "k1");
    let mut parts: Vec<&str> = token.split('.').collect();
    let forged = URL_SAFE_NO_PAD.encode(json!({"sub": "admin"}).to_string());
    parts[1] = &forged;
    assert_eq!(verify(&parts.join(".")), None);
}

// ---- the callback listener ----

#[test]
fn only_a_get_of_the_callback_path_is_read() {
    assert_eq!(
        request_line_target(b"GET /auth/callback?code=c&state=s HTTP/1.1\r\nHost: x\r\n\r\n"),
        Some("/auth/callback?code=c&state=s".into())
    );
    assert_eq!(
        request_line_target(b"POST /auth/callback HTTP/1.1\r\n\r\n"),
        None
    );
    assert_eq!(
        request_line_target(b"GET /auth/callback HTTP/2\r\n\r\n"),
        None
    );
    let params = callback_params("/auth/callback?code=a%20b&state=s&state=t").unwrap();
    assert_eq!(params["code"], "a b");
    assert_eq!(params["state"], "s");
    assert_eq!(callback_params("/other?code=c"), None);
    assert_eq!(callback_params("/auth/callback").unwrap().len(), 0);
}

/// The browser comes back to the attempt that listened `nth`, its answer
/// awaited in a task of its own: `None` when the attempt was stopped before
/// it answered.
fn come_back(
    f: &Fixture,
    nth: usize,
    state: String,
) -> tokio::task::JoinHandle<Option<CallbackReply>> {
    let sender = f.listener.servers.lock().unwrap()[nth].clone();
    tokio::spawn(async move {
        let (reply, answer) = oneshot::channel();
        let params: BTreeMap<String, String> = [
            ("state".to_owned(), state),
            ("code".to_owned(), "code-1".to_owned()),
            ("client_id".to_owned(), "oaiapp_one".to_owned()),
        ]
        .into();
        sender.send(Callback { params, reply }).await.unwrap();
        answer.await.ok()
    })
}

/// Wait until `count` exchanges are held.
async fn exchanging(f: &Fixture, count: usize) {
    for _ in 0..500 {
        if f.authority.exchanging.load(Ordering::SeqCst) >= count {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("no exchange started");
}

/// Rows S2 and S14 at the end of a sign-in: one cancelled, or replaced by
/// another, while it talks to OpenAI keeps nothing, and its end is recorded
/// once, as the cancel or the replacement.
#[tokio::test]
async fn a_sign_in_ended_while_it_finishes_keeps_nothing() {
    let hold = Arc::new(tokio::sync::Semaphore::new(0));
    // Cancelled.
    let f = fixture_with(Authority {
        exchange_hold: Some(hold.clone()),
        ..Authority::default()
    });
    f.logins.begin(caller(), None).await.unwrap();
    let back = come_back(&f, 0, f.state());
    exchanging(&f, 1).await;
    f.logins.cancel(&caller()).unwrap();
    hold.add_permits(1);
    assert_ne!(back.await.unwrap(), Some(CallbackReply::SignedIn));
    assert_eq!(f.ended().await, Some(SignInEnd::Cancelled));
    assert!(f.logins.status().unwrap().accounts.is_empty());
    // The permit the stopped exchange never took is not the next one's.
    if let Ok(permit) = hold.try_acquire() {
        permit.forget();
    }
    // Replaced by a sign-in begun meanwhile, which then waits.
    f.logins.begin(caller(), None).await.unwrap();
    let back = come_back(&f, 1, f.state());
    exchanging(&f, 2).await;
    f.logins.begin(caller(), None).await.unwrap();
    hold.add_permits(1);
    assert_ne!(back.await.unwrap(), Some(CallbackReply::SignedIn));
    assert!(f.logins.status().unwrap().accounts.is_empty());
    assert!(matches!(
        f.logins.status().unwrap().sign_in,
        SignInStatus::Waiting { .. }
    ));
    let ends: Vec<SignInEnd> = f
        .audit
        .events()
        .into_iter()
        .filter_map(|event| match event {
            LoginEvent::ChatgptSignInEnded { end, .. } => Some(end),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [SignInEnd::Cancelled, SignInEnd::Replaced]);
}

/// The saved-logins service over this fixture's accounts, with `key` saved
/// for Codex, the checked-in policy, and this fixture's audit.
async fn handing(
    f: &Fixture,
    key: Option<&str>,
) -> Arc<crate::logins::application::SavedLoginsService> {
    use nessa_protocol::agents::AgentId;
    let service = Arc::new(
        crate::logins::application::SavedLoginsService::new(
            AgentId::ALL
                .iter()
                .map(|agent| (*agent, Arc::new(Store::default()) as Arc<dyn SecretStore>))
                .collect(),
            Arc::new(Store::default()),
            Arc::new(
                crate::logins::infrastructure::cedar::CedarLoginPolicyEvaluator::new().unwrap(),
            ),
            f.audit.clone(),
        )
        .with_chatgpt(f.logins.clone()),
    );
    if let Some(key) = key {
        service
            .save(
                caller(),
                AgentId::Codex,
                nessa_protocol::lease::Login::new(key.into()).unwrap(),
            )
            .await
            .unwrap();
    }
    service
}

/// What `service` hands Codex on an SSH host for `lease`.
async fn hand(
    service: &Arc<crate::logins::application::SavedLoginsService>,
    lease: &str,
) -> Option<crate::logins::application::HandedLogin> {
    service
        .clone()
        .hand(
            nessa_sdk::domain::agent_execution::leases::EnvironmentRef::Ssh(
                nessa_sdk::domain::agent_execution::leases::SshDestination::new("studio").unwrap(),
            ),
            nessa_protocol::agents::AgentId::Codex,
            lease.into(),
        )
        .await
}

/// The last hand-off record for `lease`: what was sent, or why not.
fn handoff(f: &Fixture, lease: &str) -> Option<LoginEvent> {
    f.audit
        .events()
        .into_iter()
        .rev()
        .find(|event| match event {
            LoginEvent::Sent { lease: each, .. } | LoginEvent::NotSent { lease: each, .. } => {
                each == lease
            }
            _ => false,
        })
}

/// Rows R8, R9, R5/R6 and the policy at the hand-off: with no account in
/// use Codex is handed its key; with one, that account's token and when to
/// renew it, recorded with the account; one that needs signing in again, or
/// whose refresh fails now, hands nothing (never the key in its place) and
/// leaves the lease due so it is asked again; and a machine the policy
/// refuses costs no refresh.
#[tokio::test]
async fn codex_is_handed_the_account_in_use_and_never_the_key_in_its_place() {
    use crate::logins::application::Withheld;
    let f = fixture();
    let service = handing(&f, Some("sk-codex-key")).await;
    // R8: no account in use.
    let handed = hand(&service, "lease-key").await.unwrap();
    assert_eq!(handed.login.unwrap().expose(), "sk-codex-key");
    assert_eq!((handed.renew_by_ms, handed.account), (None, None));
    // An account signed in is in use: its token.
    assert_eq!(f.sign_in("user-1").await, SignInEnd::SignedIn);
    let handed = hand(&service, "lease-token").await.unwrap();
    let account = f.logins.status().unwrap().accounts[0].id.clone();
    assert_eq!(handed.login.unwrap().expose(), "eyJa.eyJb.access-1");
    assert_eq!(handed.account, Some(account.clone()));
    assert!(handed.renew_by_ms.is_some());
    assert!(matches!(
        handoff(&f, "lease-token"),
        Some(LoginEvent::Sent { chatgpt: Some(sent), .. }) if sent == account
    ));
    // The policy refuses this gateway's own machine before any refresh.
    let refreshed = f.authority.refreshed.load(Ordering::SeqCst);
    f.time.0.fetch_add(15 * MINUTE, Ordering::SeqCst);
    let here = service
        .clone()
        .hand(
            nessa_sdk::domain::agent_execution::leases::EnvironmentRef::Here,
            nessa_protocol::agents::AgentId::Codex,
            "lease-here".into(),
        )
        .await;
    assert_eq!(here, None);
    assert_eq!(f.authority.refreshed.load(Ordering::SeqCst), refreshed);
    // R5/R6: due for a refresh that fails now.
    f.authority
        .refreshes
        .lock()
        .unwrap()
        .push_back(Err(RefreshFailure::Unreachable));
    let handed = hand(&service, "lease-unreached").await.unwrap();
    assert_eq!((handed.login, handed.renew_by_ms), (None, Some(0)));
    assert!(matches!(
        handoff(&f, "lease-unreached"),
        Some(LoginEvent::NotSent {
            why: Withheld::RefreshFailed,
            ..
        })
    ));
    // R9: refused for good, so it needs signing in again: still not the key.
    f.authority
        .refreshes
        .lock()
        .unwrap()
        .push_back(Err(RefreshFailure::Unusable));
    let handed = hand(&service, "lease-refused").await.unwrap();
    assert_eq!((handed.login, handed.renew_by_ms), (None, Some(0)));
    assert!(matches!(
        handoff(&f, "lease-refused"),
        Some(LoginEvent::NotSent {
            why: Withheld::SignInNeeded,
            ..
        })
    ));
    let handed = hand(&service, "lease-again").await.unwrap();
    assert!(
        handed.login.is_none(),
        "the key is never handed in its place"
    );
    // Set aside: the key again.
    f.logins.use_account(caller(), None).await.unwrap();
    let handed = hand(&service, "lease-set-aside").await.unwrap();
    assert_eq!(handed.login.unwrap().expose(), "sk-codex-key");
    // A locked vault cannot say whether an account is in use: nothing, never
    // the key, and asked again once idle.
    *f.store.locked.lock().unwrap() = true;
    let handed = hand(&service, "lease-locked").await.unwrap();
    assert_eq!((handed.login, handed.renew_by_ms), (None, Some(0)));
    assert!(matches!(
        handoff(&f, "lease-locked"),
        Some(LoginEvent::NotSent {
            why: Withheld::Locked,
            ..
        })
    ));
}

/// Row S13: a browser that never comes back ends the sign-in after its
/// wait, recorded, and keeps nothing.
#[tokio::test(start_paused = true)]
async fn a_sign_in_the_browser_never_finishes_times_out() {
    let f = fixture();
    f.logins.begin(caller(), None).await.unwrap();
    tokio::time::sleep(super::domain::SIGN_IN_WAIT + Duration::from_secs(1)).await;
    assert_eq!(f.ended().await, Some(SignInEnd::TimedOut));
    assert!(f.logins.status().unwrap().accounts.is_empty());
    assert!(f.audit.events().iter().any(|event| matches!(
        event,
        LoginEvent::ChatgptSignInEnded {
            end: SignInEnd::TimedOut,
            ..
        }
    )));
}

/// Row S12b: a sign-in whose end cannot be recorded keeps nothing and
/// revokes the grant it got; a cancel that cannot be recorded still stops
/// the sign-in, and says it was not recorded.
#[tokio::test]
async fn a_sign_in_or_cancel_that_cannot_be_recorded_says_so() {
    let f = fixture();
    *f.authority.subject.lock().unwrap() = "user-1".into();
    f.logins.begin(caller(), None).await.unwrap();
    let state = f.state();
    let issued = f.authority.issued.lock().unwrap().clone();
    *f.audit.refuses.lock().unwrap() = true;
    let reply = f
        .callback(&[("code", "c"), ("state", &state), ("client_id", &issued)])
        .await;
    assert!(matches!(reply, CallbackReply::Failed));
    assert_eq!(f.ended().await, Some(SignInEnd::NotSaved));
    assert!(f.store.accounts().accounts().is_empty());
    assert_eq!(*f.authority.revoked.lock().unwrap(), ["refresh-1"]);
    *f.audit.refuses.lock().unwrap() = false;
    f.logins.begin(caller(), None).await.unwrap();
    *f.audit.refuses.lock().unwrap() = true;
    assert_eq!(
        f.logins.cancel(&caller()),
        Err(LoginsError::ChangeUnrecorded)
    );
    assert_eq!(f.ended().await, Some(SignInEnd::Cancelled));
}

/// Row A1b: removing an account while it signs in again ends that sign-in,
/// and the account stays removed.
#[tokio::test]
async fn an_account_removed_while_it_signs_in_again_stays_removed() {
    let f = fixture();
    f.store.put(&with_one(START + 55 * MINUTE));
    let one = AccountId::parse(ONE).unwrap();
    f.logins.begin(caller(), Some(one.clone())).await.unwrap();
    f.logins.remove(caller(), one).await.unwrap();
    assert_eq!(f.ended().await, Some(SignInEnd::Cancelled));
    assert!(f.store.accounts().accounts().is_empty());
    assert!(f.store.accounts().in_use().is_none());
}
