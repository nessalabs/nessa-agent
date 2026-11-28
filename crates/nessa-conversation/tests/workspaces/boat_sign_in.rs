//! Sign in with Boat and the renewal of its access token, over fakes of
//! Boat's OAuth endpoints, the loopback callback, the vault and the VM
//! provider (`docs/design/boat-sign-in.md`).
use crate::workspaces::{
    application::{
        BoatAuthorization, BoatSignIn, BoatTokens, CallbackPage, CallbackQuery, GrantFailure,
        PendingCallback, Provider, RecoveryAudit, RecoveryEvent, RenewedBoatSettings, RestCause,
        Settings, SignInCallbacks, SignInFailure, SignInState, WorkspaceReceipt, WorkspaceRuntime,
        Workspaces, RENEW_MARGIN_MS,
    },
    domain::{Admitted, BoatConnection, BoatGrant, CloudAccount, CloudConnection, Failure, VmSpec},
};
use async_trait::async_trait;
use base64::Engine as _;
use sha2::Digest as _;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

const HOUR_MS: u64 = 3_600_000;

/// The vault: one saved connection, a generation, and saves that can be
/// made to fail.
#[derive(Default)]
struct Memory {
    saved: Mutex<Option<CloudConnection>>,
    locked: std::sync::atomic::AtomicBool,
    failing_saves: AtomicU64,
    saves: AtomicU64,
    /// Outcomes for the next saves in order, `false` refusing one; past
    /// them, `failing_saves` decides.
    script: Mutex<std::collections::VecDeque<bool>>,
    /// How long a save takes before it lands.
    save_delay_ms: AtomicU64,
}
#[async_trait]
impl Settings for Memory {
    async fn read(&self) -> Result<CloudConnection, Failure> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(Failure::Locked);
        }
        self.saved
            .lock()
            .unwrap()
            .clone()
            .ok_or(Failure::NotConfigured)
    }
    fn generation(&self) -> Result<u64, Failure> {
        if self.locked.load(Ordering::SeqCst) {
            Err(Failure::Locked)
        } else {
            Ok(1)
        }
    }
    async fn save(&self, _generation: u64, connection: CloudConnection) -> Result<(), Failure> {
        tokio::time::sleep(Duration::from_millis(
            self.save_delay_ms.load(Ordering::SeqCst),
        ))
        .await;
        if let Some(ok) = self.script.lock().unwrap().pop_front() {
            if !ok {
                return Err(Failure::Storage);
            }
        } else if self.failing_saves.load(Ordering::SeqCst) > 0 {
            self.failing_saves.fetch_sub(1, Ordering::SeqCst);
            return Err(Failure::Storage);
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.saved.lock().unwrap() = Some(connection);
        Ok(())
    }
    async fn clear(&self) -> Result<(), Failure> {
        *self.saved.lock().unwrap() = None;
        Ok(())
    }
}

/// Boat's token endpoint: each refresh token works once, and each answer
/// carries the next one.
struct FakeBoat {
    client: Option<&'static str>,
    issued: AtomicU64,
    live_refresh: Mutex<String>,
    refreshes: AtomicU64,
    next_refresh: Mutex<Option<GrantFailure>>,
    exchanged: Mutex<Vec<(String, String, String)>>,
    revoked: Mutex<Vec<String>>,
    refresh_delay: Duration,
}
impl FakeBoat {
    fn new() -> Self {
        Self {
            client: Some("boatapp_fixture"),
            issued: AtomicU64::new(0),
            live_refresh: Mutex::new("boatrt_0".into()),
            refreshes: AtomicU64::new(0),
            next_refresh: Mutex::new(None),
            exchanged: Mutex::default(),
            revoked: Mutex::default(),
            refresh_delay: Duration::ZERO,
        }
    }
    fn issue(&self) -> BoatTokens {
        let n = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        let refresh = format!("boatrt_{n}");
        *self.live_refresh.lock().unwrap() = refresh.clone();
        BoatTokens {
            access: Zeroizing::new(format!("boat_{n}")),
            refresh: Zeroizing::new(refresh),
            expires_in: 3600,
        }
    }
}
#[async_trait]
impl BoatAuthorization for FakeBoat {
    fn authorize_url(&self, redirect_uri: &str, state: &str, challenge: &str) -> Option<String> {
        self.client.map(|client| {
            format!("https://boat.test/authorize?client_id={client}&redirect_uri={redirect_uri}&state={state}&code_challenge={challenge}")
        })
    }
    fn issuer(&self) -> String {
        "https://boat.test".into()
    }
    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<BoatTokens, GrantFailure> {
        self.exchanged
            .lock()
            .unwrap()
            .push((code.into(), redirect_uri.into(), verifier.into()));
        if code == "refused" {
            return Err(GrantFailure::Refused);
        }
        Ok(self.issue())
    }
    async fn refresh(&self, refresh: &str) -> Result<BoatTokens, GrantFailure> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.refresh_delay).await;
        if let Some(failure) = self.next_refresh.lock().unwrap().take() {
            return Err(failure);
        }
        if *self.live_refresh.lock().unwrap() != refresh {
            return Err(GrantFailure::Refused);
        }
        Ok(self.issue())
    }
    async fn revoke(&self, refresh: &str) {
        self.revoked.lock().unwrap().push(refresh.into());
    }
    async fn login(&self, access: &str) -> Result<String, GrantFailure> {
        assert!(access.starts_with("boat_"));
        Ok("octo".into())
    }
}

struct Valid;
#[async_trait]
impl Provider for Valid {
    async fn validate(&self, account: &CloudAccount, _spec: &VmSpec) -> Result<(), Failure> {
        let CloudAccount::Boat(account) = account else {
            return Err(Failure::InvalidInput);
        };
        assert!(account.token.as_slice().starts_with(b"boat_"));
        Ok(())
    }
    async fn create(
        &self,
        _account: &CloudAccount,
        _spec: &VmSpec,
        _key: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        unreachable!()
    }
    async fn connect(
        &self,
        _account: &CloudAccount,
        _spec: &VmSpec,
        _key: &str,
        _vm: &str,
        _admission: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        unreachable!()
    }
    async fn remove(
        &self,
        _account: &CloudAccount,
        _vm: &str,
        _admission: &Admitted,
    ) -> Result<(), Failure> {
        unreachable!()
    }
    async fn stop(
        &self,
        _account: &CloudAccount,
        _vm: &str,
        _cause: RestCause,
    ) -> Result<(), Failure> {
        unreachable!()
    }
}
struct NoAudit;
impl RecoveryAudit for NoAudit {
    fn record(&self, _event: &RecoveryEvent) -> Result<(), Failure> {
        Ok(())
    }
}

/// The browser: visits are sent in by the test.
type Visit = (CallbackQuery, oneshot::Sender<CallbackPage>);
struct Browser(Mutex<Option<mpsc::Receiver<Visit>>>);
struct Pending(mpsc::Receiver<Visit>);
#[async_trait]
impl SignInCallbacks for Browser {
    async fn open(&self) -> std::io::Result<Box<dyn PendingCallback>> {
        let arrivals = self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("one sign-in per browser");
        Ok(Box::new(Pending(arrivals)))
    }
}
#[async_trait]
impl PendingCallback for Pending {
    fn redirect_uri(&self) -> String {
        "http://localhost:5555/boat/callback".into()
    }
    async fn arrived(&mut self) -> Option<Visit> {
        self.0.recv().await
    }
}

struct Rig {
    boat: Arc<FakeBoat>,
    vault: Arc<Memory>,
    sign_in: BoatSignIn,
    visits: mpsc::Sender<Visit>,
}
fn rig(boat: FakeBoat) -> Rig {
    let boat = Arc::new(boat);
    let vault = Arc::new(Memory::default());
    let settings = Arc::new(RenewedBoatSettings::new(vault.clone(), boat.clone()));
    let workspaces =
        Workspaces::new(settings, Arc::new(Valid), Arc::new(NoAudit)).with_image(Some("bsh_img"));
    let (visits, arrivals) = mpsc::channel(4);
    let sign_in = BoatSignIn::new(
        boat.clone(),
        Arc::new(Browser(Mutex::new(Some(arrivals)))),
        workspaces,
    );
    Rig {
        boat,
        vault,
        sign_in,
        visits,
    }
}
fn query_of(url: &str, key: &str) -> String {
    url.split(['?', '&'])
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap()
        .to_owned()
}
async fn visit(rig: &Rig, query: CallbackQuery) -> CallbackPage {
    let (reply, page) = oneshot::channel();
    rig.visits.send((query, reply)).await.unwrap();
    page.await.unwrap()
}
async fn settled(sign_in: &BoatSignIn) -> SignInState {
    for _ in 0..200 {
        let state = sign_in.state();
        if !matches!(state, SignInState::Waiting { .. }) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    sign_in.state()
}

#[tokio::test]
async fn a_sign_in_exchanges_its_code_with_the_verifier_and_saves_the_grant() {
    let rig = rig(FakeBoat::new());
    let url = rig.sign_in.start().await.unwrap();
    assert_eq!(
        rig.sign_in.state(),
        SignInState::Waiting { url: url.clone() }
    );
    let state = query_of(&url, "state");
    // Another attempt's tab is answered Failed, and the sign-in waits on.
    let stale = visit(
        &rig,
        CallbackQuery {
            code: Some("other".into()),
            state: Some("not-this-one".into()),
            iss: Some("https://boat.test".into()),
            error: None,
        },
    )
    .await;
    assert_eq!(stale, CallbackPage::Failed);
    assert!(matches!(rig.sign_in.state(), SignInState::Waiting { .. }));
    let page = visit(
        &rig,
        CallbackQuery {
            code: Some("code-1".into()),
            state: Some(state),
            iss: Some("https://boat.test".into()),
            error: None,
        },
    )
    .await;
    assert_eq!(page, CallbackPage::Connected);
    assert_eq!(settled(&rig.sign_in).await, SignInState::Idle);
    let exchanged = rig.boat.exchanged.lock().unwrap().clone();
    assert_eq!(exchanged.len(), 1);
    let (code, redirect, verifier) = &exchanged[0];
    assert_eq!(code, "code-1");
    assert_eq!(redirect, "http://localhost:5555/boat/callback");
    // PKCE S256: the challenge in the URL is the verifier's hash.
    assert_eq!(
        query_of(&url, "code_challenge"),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()))
    );
    assert!(verifier.len() >= 43 && verifier.len() <= 128);
    let Some(CloudConnection::Boat(saved)) = rig.vault.saved.lock().unwrap().clone() else {
        panic!("nothing was saved");
    };
    assert_eq!(saved.account.token.as_slice(), b"boat_1");
    assert_eq!(saved.environment, "nessa");
    assert_eq!(saved.image, "bsh_img");
    let grant = saved.grant.unwrap();
    assert_eq!(grant.refresh.as_slice(), b"boatrt_1");
    assert_eq!(grant.login, "octo");
}

#[tokio::test]
async fn cancel_on_boat_wrong_issuer_and_a_refused_code_each_end_the_sign_in() {
    for (query, cause) in [
        (
            CallbackQuery {
                error: Some("access_denied".into()),
                iss: Some("https://boat.test".into()),
                ..Default::default()
            },
            SignInFailure::Denied,
        ),
        (
            CallbackQuery {
                code: Some("c".into()),
                iss: Some("https://evil.test".into()),
                ..Default::default()
            },
            SignInFailure::Rejected,
        ),
        (
            CallbackQuery {
                code: Some("refused".into()),
                iss: Some("https://boat.test".into()),
                ..Default::default()
            },
            SignInFailure::Unreachable,
        ),
    ] {
        let rig = rig(FakeBoat::new());
        let url = rig.sign_in.start().await.unwrap();
        let page = visit(
            &rig,
            CallbackQuery {
                state: Some(query_of(&url, "state")),
                ..query
            },
        )
        .await;
        assert_eq!(page, CallbackPage::Failed);
        assert_eq!(settled(&rig.sign_in).await, SignInState::Failed(cause));
        assert!(rig.vault.saved.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn a_sign_in_nobody_finishes_expires_and_cancel_stops_one() {
    let mut rig = rig(FakeBoat::new());
    rig.sign_in = rig.sign_in.with_deadline(Duration::from_millis(20));
    rig.sign_in.start().await.unwrap();
    assert_eq!(
        settled(&rig.sign_in).await,
        SignInState::Failed(SignInFailure::Expired)
    );
    let rig = self::rig(FakeBoat::new());
    rig.sign_in.start().await.unwrap();
    rig.sign_in.cancel();
    assert_eq!(rig.sign_in.state(), SignInState::Idle);
}

#[tokio::test]
async fn no_app_or_a_locked_vault_starts_nothing() {
    let rig = rig(FakeBoat {
        client: None,
        ..FakeBoat::new()
    });
    assert!(!rig.sign_in.available());
    assert_eq!(rig.sign_in.start().await, Err(Failure::NotConfigured));
    let rig = self::rig(FakeBoat::new());
    rig.vault.locked.store(true, Ordering::SeqCst);
    assert_eq!(rig.sign_in.start().await, Err(Failure::Locked));
    assert_eq!(rig.sign_in.state(), SignInState::Idle);
}

/// A signed-in connection whose access token ends at `expires_at_ms`.
fn signed_in(refresh: &str, expires_at_ms: u64) -> CloudConnection {
    CloudConnection::Boat(
        BoatConnection::new("boat_0".into(), "nessa".into(), "bsh_img".into())
            .unwrap()
            .with_grant(BoatGrant::new(refresh.into(), expires_at_ms, "octo".into()).unwrap()),
    )
}
fn renewing(boat: &Arc<FakeBoat>, vault: &Arc<Memory>, now: Arc<AtomicU64>) -> RenewedBoatSettings {
    RenewedBoatSettings::with_clock(
        vault.clone(),
        boat.clone(),
        Arc::new(move || now.load(Ordering::SeqCst)),
    )
}
fn token(connection: &CloudConnection) -> String {
    let CloudConnection::Boat(boat) = connection else {
        panic!("not Boat")
    };
    String::from_utf8(boat.account.token.to_vec()).unwrap()
}

#[tokio::test]
async fn the_access_token_is_renewed_once_when_due_however_many_read_it() {
    let boat = Arc::new(FakeBoat {
        refresh_delay: Duration::from_millis(30),
        ..FakeBoat::new()
    });
    let vault = Arc::new(Memory::default());
    *vault.saved.lock().unwrap() = Some(signed_in("boatrt_0", HOUR_MS));
    let now = Arc::new(AtomicU64::new(0));
    let settings = Arc::new(renewing(&boat, &vault, now.clone()));
    // Not due: read as saved, nothing spent.
    assert_eq!(token(&settings.read().await.unwrap()), "boat_0");
    assert_eq!(boat.refreshes.load(Ordering::SeqCst), 0);
    // Within the margin: eight readers at once spend the refresh token once.
    now.store(HOUR_MS - RENEW_MARGIN_MS, Ordering::SeqCst);
    let reads = (0..8).map(|_| {
        let settings = settings.clone();
        tokio::spawn(async move { token(&settings.read().await.unwrap()) })
    });
    for read in reads {
        assert_eq!(read.await.unwrap(), "boat_1");
    }
    assert_eq!(boat.refreshes.load(Ordering::SeqCst), 1);
    let Some(CloudConnection::Boat(saved)) = vault.saved.lock().unwrap().clone() else {
        panic!()
    };
    let grant = saved.grant.unwrap();
    assert_eq!(grant.refresh.as_slice(), b"boatrt_1");
    assert_eq!(grant.expires_at_ms, HOUR_MS - RENEW_MARGIN_MS + HOUR_MS);
}

#[tokio::test]
async fn a_renewal_the_vault_refused_is_kept_and_saved_later_not_spent_again() {
    let boat = Arc::new(FakeBoat::new());
    let vault = Arc::new(Memory::default());
    *vault.saved.lock().unwrap() = Some(signed_in("boatrt_0", HOUR_MS));
    let now = Arc::new(AtomicU64::new(HOUR_MS));
    let settings = renewing(&boat, &vault, now.clone());
    vault.failing_saves.store(1, Ordering::SeqCst);
    assert_eq!(token(&settings.read().await.unwrap()), "boat_1");
    // The vault still holds the spent refresh token; the next read hands
    // out the kept renewal and saves it, without asking Boat again.
    assert_eq!(token(&settings.read().await.unwrap()), "boat_1");
    assert_eq!(boat.refreshes.load(Ordering::SeqCst), 1);
    let Some(CloudConnection::Boat(saved)) = vault.saved.lock().unwrap().clone() else {
        panic!()
    };
    assert_eq!(saved.grant.unwrap().refresh.as_slice(), b"boatrt_1");
}

#[tokio::test]
async fn a_held_renewal_saved_then_renewed_again_unsaved_names_the_grant_it_replaced() {
    let boat = Arc::new(FakeBoat::new());
    let vault = Arc::new(Memory::default());
    *vault.saved.lock().unwrap() = Some(signed_in("boatrt_0", HOUR_MS));
    let now = Arc::new(AtomicU64::new(HOUR_MS));
    let settings = renewing(&boat, &vault, now.clone());
    // 1: renewed to boatrt_1, which the vault refuses; it is held.
    vault.script.lock().unwrap().extend([false]);
    assert_eq!(token(&settings.read().await.unwrap()), "boat_1");
    // 2: the held grant is due too. Saving it succeeds, its renewal to
    // boatrt_2 is refused: the held renewal replaces boatrt_1 now.
    now.store(3 * HOUR_MS, Ordering::SeqCst);
    vault.script.lock().unwrap().extend([true, false]);
    assert_eq!(token(&settings.read().await.unwrap()), "boat_2");
    // 3: the vault holds boatrt_1, so the held boatrt_2 is used and saved,
    // and the spent boatrt_1 is never sent to Boat again.
    assert_eq!(token(&settings.read().await.unwrap()), "boat_2");
    assert_eq!(boat.refreshes.load(Ordering::SeqCst), 2);
    let Some(CloudConnection::Boat(saved)) = vault.saved.lock().unwrap().clone() else {
        panic!("signed out")
    };
    assert_eq!(saved.grant.unwrap().refresh.as_slice(), b"boatrt_2");
}

#[tokio::test]
async fn cancel_while_the_sign_in_is_saving_lets_the_save_finish_and_report() {
    let rig = rig(FakeBoat::new());
    rig.vault.save_delay_ms.store(200, Ordering::SeqCst);
    let url = rig.sign_in.start().await.unwrap();
    let query = CallbackQuery {
        code: Some("code-1".into()),
        state: Some(query_of(&url, "state")),
        iss: Some("https://boat.test".into()),
        error: None,
    };
    let (reply, page) = oneshot::channel();
    rig.visits.send((query, reply)).await.unwrap();
    // The save is under way: Cancel can no longer stop it.
    tokio::time::sleep(Duration::from_millis(80)).await;
    rig.sign_in.cancel();
    assert_eq!(page.await.unwrap(), CallbackPage::Connected);
    assert_eq!(settled(&rig.sign_in).await, SignInState::Idle);
    assert!(rig.vault.saved.lock().unwrap().is_some());
    assert!(rig.boat.revoked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_deadline_passing_while_the_sign_in_is_saving_lets_the_save_finish_and_report() {
    let mut rig = rig(FakeBoat::new());
    rig.sign_in = rig.sign_in.with_deadline(Duration::from_millis(100));
    rig.vault.save_delay_ms.store(300, Ordering::SeqCst);
    let url = rig.sign_in.start().await.unwrap();
    let query = CallbackQuery {
        code: Some("code-1".into()),
        state: Some(query_of(&url, "state")),
        iss: Some("https://boat.test".into()),
        error: None,
    };
    let (reply, page) = oneshot::channel();
    rig.visits.send((query, reply)).await.unwrap();
    // The browser came back in time; the save outlives the deadline.
    assert_eq!(page.await.unwrap(), CallbackPage::Connected);
    assert_eq!(settled(&rig.sign_in).await, SignInState::Idle);
    assert!(rig.vault.saved.lock().unwrap().is_some());
    assert!(rig.boat.revoked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_refused_renewal_signs_out_and_an_unreachable_boat_keeps_a_live_token() {
    let boat = Arc::new(FakeBoat::new());
    let vault = Arc::new(Memory::default());
    *vault.saved.lock().unwrap() = Some(signed_in("boatrt_0", HOUR_MS));
    let now = Arc::new(AtomicU64::new(HOUR_MS - 1000));
    let settings = renewing(&boat, &vault, now.clone());
    *boat.next_refresh.lock().unwrap() = Some(GrantFailure::Unreachable);
    assert_eq!(token(&settings.read().await.unwrap()), "boat_0");
    now.store(HOUR_MS, Ordering::SeqCst);
    *boat.next_refresh.lock().unwrap() = Some(GrantFailure::Unreachable);
    assert!(matches!(settings.read().await, Err(Failure::Provider)));
    *boat.next_refresh.lock().unwrap() = Some(GrantFailure::Refused);
    assert!(matches!(settings.read().await, Err(Failure::NotConfigured)));
    assert!(vault.saved.lock().unwrap().is_none());
}

#[tokio::test]
async fn disconnecting_a_sign_in_revokes_it_and_a_pasted_token_is_never_renewed() {
    let boat = Arc::new(FakeBoat::new());
    let vault = Arc::new(Memory::default());
    *vault.saved.lock().unwrap() = Some(signed_in("boatrt_0", HOUR_MS));
    let settings = renewing(&boat, &vault, Arc::new(AtomicU64::new(0)));
    settings.clear().await.unwrap();
    assert_eq!(*boat.revoked.lock().unwrap(), vec!["boatrt_0".to_owned()]);
    let pasted = CloudConnection::Boat(
        BoatConnection::new("boat_pasted".into(), "nessa".into(), "bsh_img".into()).unwrap(),
    );
    *vault.saved.lock().unwrap() = Some(pasted);
    let settings = renewing(&boat, &vault, Arc::new(AtomicU64::new(u64::MAX / 2)));
    assert_eq!(token(&settings.read().await.unwrap()), "boat_pasted");
    settings.clear().await.unwrap();
    assert_eq!(boat.refreshes.load(Ordering::SeqCst), 0);
    assert_eq!(boat.revoked.lock().unwrap().len(), 1);
}
