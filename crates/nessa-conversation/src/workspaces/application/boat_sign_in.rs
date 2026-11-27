//! Sign in with Boat (`docs/design/boat-sign-in.md`): the browser sign-in
//! that saves a Boat connection without a pasted token, and the one place
//! its short-lived access token is renewed.
//!
//! ```text
//! start:  open a loopback callback ─► authorize URL (PKCE S256, state)
//! wait:   callback ─► state and issuer checked ─► code exchanged
//!         ─► account read ─► Workspaces::save_signed_in (validate, vault)
//! renew:  every read of the Boat settings whose access token is due
//!         ─► one refresh at a time ─► rotated grant saved ─► fresh token
//! ```
//!
//! Boat rotates the refresh token on every renewal and disconnects the app
//! when an old one is used again, so renewals never run side by side, and
//! the newest grant is kept in memory until the vault has it.
use super::workspaces::{Settings, Workspaces};
use crate::workspaces::domain::{BoatConnection, BoatGrant, Cloud, CloudConnection, Failure};
use async_trait::async_trait;
use base64::Engine as _;
use sha2::Digest as _;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

/// How long a sign-in waits for the browser to come back.
pub const SIGN_IN_DEADLINE: Duration = Duration::from_secs(10 * 60);
/// An access token is renewed once it has less than this left, so a call
/// that starts with it never meets its end.
pub const RENEW_MARGIN_MS: u64 = 5 * 60 * 1000;

/// What Boat's token endpoint answered.
pub struct BoatTokens {
    pub access: Zeroizing<String>,
    pub refresh: Zeroizing<String>,
    /// Seconds the access token lasts.
    pub expires_in: u64,
}

/// Why Boat's token endpoint gave no tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantFailure {
    /// `invalid_grant`: the code or refresh token is used, expired or
    /// revoked. A refused refresh token never works again.
    Refused,
    /// Boat refused the app itself, or answered something this does not read.
    Rejected,
    /// Boat could not be reached, or did not answer in time.
    Unreachable,
}

/// Boat's OAuth endpoints, as one public (secretless) app uses them.
#[async_trait]
pub trait BoatAuthorization: Send + Sync {
    /// Where the person approves Nessa, or `None` when this build names no
    /// Boat app to sign in through.
    fn authorize_url(&self, redirect_uri: &str, state: &str, challenge: &str) -> Option<String>;
    /// The `iss` Boat's redirect must carry.
    fn issuer(&self) -> String;
    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<BoatTokens, GrantFailure>;
    async fn refresh(&self, refresh: &str) -> Result<BoatTokens, GrantFailure>;
    /// Best effort: tell Boat a refresh token is no longer wanted.
    async fn revoke(&self, refresh: &str);
    /// The login of the account `access` belongs to.
    async fn login(&self, access: &str) -> Result<String, GrantFailure>;
}

/// What the browser brought back to the callback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub iss: Option<String>,
    pub error: Option<String>,
}

/// The page the browser is answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackPage {
    Connected,
    Failed,
}

/// Where the browser comes back to: a listener on this machine.
#[async_trait]
pub trait SignInCallbacks: Send + Sync {
    async fn open(&self) -> std::io::Result<Box<dyn PendingCallback>>;
}
#[async_trait]
pub trait PendingCallback: Send {
    /// The `redirect_uri` that reaches this listener.
    fn redirect_uri(&self) -> String;
    /// The next visit to the callback path, and where to send its page.
    /// `None` once the listener can take no more.
    async fn arrived(&mut self) -> Option<(CallbackQuery, oneshot::Sender<CallbackPage>)>;
}

/// Why a sign-in ended without a connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInFailure {
    /// The person chose Cancel on Boat's page.
    Denied,
    /// Nobody came back before [`SIGN_IN_DEADLINE`].
    Expired,
    /// The redirect named another issuer, or an error Boat documents
    /// otherwise.
    Rejected,
    /// Boat refused the code, or could not be reached for it.
    Unreachable,
    /// The account signed in, but its connection could not be saved.
    Saving(Failure),
}

/// Where the gateway's sign-in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInState {
    Idle,
    Waiting { url: String },
    Failed(SignInFailure),
}

struct Attempt {
    id: u64,
    state: SignInState,
    task: Option<tokio::task::AbortHandle>,
}

/// The one sign-in at a time this gateway runs.
pub struct BoatSignIn {
    oauth: Arc<dyn BoatAuthorization>,
    callbacks: Arc<dyn SignInCallbacks>,
    workspaces: Workspaces,
    deadline: Duration,
    attempt: Arc<StdMutex<Attempt>>,
}

impl BoatSignIn {
    pub fn new(
        oauth: Arc<dyn BoatAuthorization>,
        callbacks: Arc<dyn SignInCallbacks>,
        workspaces: Workspaces,
    ) -> Self {
        Self {
            oauth,
            callbacks,
            workspaces,
            deadline: SIGN_IN_DEADLINE,
            attempt: Arc::new(StdMutex::new(Attempt {
                id: 0,
                state: SignInState::Idle,
                task: None,
            })),
        }
    }
    /// The same sign-in, waiting `deadline` for the browser instead.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }
    /// Whether this build names a Boat app to sign in through.
    pub fn available(&self) -> bool {
        self.oauth
            .authorize_url("http://localhost/", "s", "c")
            .is_some()
    }
    pub fn state(&self) -> SignInState {
        lock(&self.attempt).state.clone()
    }
    /// Stop waiting for the browser. A sign-in already exchanging its code
    /// is stopped too; nothing is saved.
    pub fn cancel(&self) {
        let mut attempt = lock(&self.attempt);
        if let Some(task) = attempt.task.take() {
            task.abort();
        }
        attempt.id += 1;
        attempt.state = SignInState::Idle;
    }

    /// Begin a sign-in, ending any earlier one, and answer the URL to open.
    ///
    /// # Errors
    /// [`Failure::NotConfigured`] when no Boat app is named;
    /// [`Failure::Locked`] while the vault cannot be written, since a
    /// sign-in that could not be saved would only spend the person's time;
    /// [`Failure::Provider`] when no callback listener can be opened.
    pub async fn start(&self) -> Result<String, Failure> {
        if !self.available() {
            return Err(Failure::NotConfigured);
        }
        self.workspaces
            .settings(Cloud::Boat)
            .ok_or(Failure::NotConfigured)?
            .generation()?;
        let mut pending = self.callbacks.open().await.map_err(|_| Failure::Provider)?;
        let redirect = pending.redirect_uri();
        let state = random_text(24)?;
        let verifier = Zeroizing::new(random_text(48)?);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        let url = self
            .oauth
            .authorize_url(&redirect, &state, &challenge)
            .ok_or(Failure::NotConfigured)?;
        let mut attempt = lock(&self.attempt);
        if let Some(task) = attempt.task.take() {
            task.abort();
        }
        attempt.id += 1;
        let id = attempt.id;
        attempt.state = SignInState::Waiting { url: url.clone() };
        let shared = self.attempt.clone();
        let oauth = self.oauth.clone();
        let workspaces = self.workspaces.clone();
        let deadline = self.deadline;
        let task = tokio::spawn(async move {
            let outcome = match tokio::time::timeout(
                deadline,
                wait(
                    &mut *pending,
                    &*oauth,
                    &workspaces,
                    &redirect,
                    &state,
                    &verifier,
                ),
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => Err(SignInFailure::Expired),
            };
            let mut attempt = lock(&shared);
            if attempt.id == id {
                attempt.task = None;
                attempt.state = match outcome {
                    Ok(()) => SignInState::Idle,
                    Err(failure) => SignInState::Failed(failure),
                };
            }
        });
        attempt.task = Some(task.abort_handle());
        Ok(url)
    }
}

/// Wait for the browser, then exchange and save. A visit that carries
/// another attempt's state is answered Failed and waited past: it is a
/// stale tab, or someone else's page, never this sign-in.
async fn wait(
    pending: &mut dyn PendingCallback,
    oauth: &dyn BoatAuthorization,
    workspaces: &Workspaces,
    redirect: &str,
    state: &str,
    verifier: &str,
) -> Result<(), SignInFailure> {
    loop {
        let Some((query, page)) = pending.arrived().await else {
            return Err(SignInFailure::Unreachable);
        };
        if query.state.as_deref() != Some(state) {
            let _ = page.send(CallbackPage::Failed);
            continue;
        }
        let outcome = finish(query, oauth, workspaces, redirect, verifier).await;
        let _ = page.send(if outcome.is_ok() {
            CallbackPage::Connected
        } else {
            CallbackPage::Failed
        });
        return outcome;
    }
}

async fn finish(
    query: CallbackQuery,
    oauth: &dyn BoatAuthorization,
    workspaces: &Workspaces,
    redirect: &str,
    verifier: &str,
) -> Result<(), SignInFailure> {
    // RFC 9207: a redirect that names another issuer is not this sign-in.
    if query.iss.as_deref() != Some(oauth.issuer().as_str()) {
        return Err(SignInFailure::Rejected);
    }
    match (query.error.as_deref(), query.code) {
        (Some("access_denied"), _) => Err(SignInFailure::Denied),
        (None, Some(code)) if !code.is_empty() => {
            let tokens = oauth
                .exchange(&code, redirect, verifier)
                .await
                .map_err(|_| SignInFailure::Unreachable)?;
            let saved = async {
                let login = oauth
                    .login(&tokens.access)
                    .await
                    .map_err(|_| SignInFailure::Unreachable)?;
                workspaces
                    .save_signed_in(&tokens, login, now_ms())
                    .await
                    .map_err(SignInFailure::Saving)
            }
            .await;
            if saved.is_err() {
                // Nothing keeps this grant: tell Boat so.
                oauth.revoke(&tokens.refresh).await;
            }
            saved
        }
        _ => Err(SignInFailure::Rejected),
    }
}

/// The Boat settings, with the access token a grant renews kept current.
/// Every reader of Boat's connection reads through this, so it is the only
/// place a refresh token is ever spent.
pub struct RenewedBoatSettings {
    inner: Arc<dyn Settings>,
    oauth: Arc<dyn BoatAuthorization>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Held across a renewal; holds a renewed connection the vault did not
    /// take yet, with the refresh token it replaced.
    renewing: tokio::sync::Mutex<Option<Unsaved>>,
}
struct Unsaved {
    replaced: Zeroizing<Vec<u8>>,
    connection: BoatConnection,
}

impl RenewedBoatSettings {
    pub fn new(inner: Arc<dyn Settings>, oauth: Arc<dyn BoatAuthorization>) -> Self {
        Self::with_clock(inner, oauth, Arc::new(now_ms))
    }
    pub fn with_clock(
        inner: Arc<dyn Settings>,
        oauth: Arc<dyn BoatAuthorization>,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            oauth,
            now_ms,
            renewing: tokio::sync::Mutex::new(None),
        }
    }

    /// The saved connection, renewed first when its access token is due.
    /// Readers wait for a renewal in progress: Boat ends the old access
    /// token the moment it issues the new one.
    async fn current(&self) -> Result<CloudConnection, Failure> {
        let mut unsaved = self.renewing.lock().await;
        let generation = self.inner.generation()?;
        let stored = self.inner.read().await?;
        let CloudConnection::Boat(stored) = stored else {
            return Ok(stored);
        };
        let Some(grant) = stored.grant.clone() else {
            *unsaved = None;
            return Ok(CloudConnection::Boat(stored));
        };
        // The refresh token the vault holds: an unsaved renewal answers it.
        let in_vault = Zeroizing::new(grant.refresh.as_slice().to_vec());
        // A renewal the vault did not take is the newest grant there is.
        let mut current = stored;
        if let Some(held) = unsaved.take() {
            if held.replaced.as_slice() == grant.refresh.as_slice() {
                current = held.connection.clone();
                if self
                    .inner
                    .save(generation, CloudConnection::Boat(current.clone()))
                    .await
                    .is_err()
                {
                    *unsaved = Some(held);
                }
            }
        }
        let grant = current.grant.clone().ok_or(Failure::Storage)?;
        let now = (self.now_ms)();
        if !grant.due(now, RENEW_MARGIN_MS) {
            return Ok(CloudConnection::Boat(current));
        }
        let refresh =
            std::str::from_utf8(grant.refresh.as_slice()).map_err(|_| Failure::Storage)?;
        match self.oauth.refresh(refresh).await {
            Ok(tokens) => {
                let renewed = signed_in(&current, &tokens, grant.login.clone(), now)?;
                *unsaved = match self
                    .inner
                    .save(generation, CloudConnection::Boat(renewed.clone()))
                    .await
                {
                    Ok(()) => None,
                    // Keep it: the refresh token it replaced is spent.
                    Err(_) => Some(Unsaved {
                        replaced: in_vault,
                        connection: renewed.clone(),
                    }),
                };
                Ok(CloudConnection::Boat(renewed))
            }
            Err(GrantFailure::Refused) => {
                // Revoked in Boat, or disconnected there: this sign-in is
                // over, and Settings offers it again.
                tracing::warn!(
                    target: "nessa_server::workspaces::boat",
                    "Boat refused the saved sign-in; it was removed"
                );
                *unsaved = None;
                self.inner.clear().await?;
                Err(Failure::NotConfigured)
            }
            Err(_) if !grant.due(now, 0) => Ok(CloudConnection::Boat(current)),
            Err(_) => Err(Failure::Provider),
        }
    }
}

#[async_trait]
impl Settings for RenewedBoatSettings {
    fn generation(&self) -> Result<u64, Failure> {
        self.inner.generation()
    }
    async fn read(&self) -> Result<CloudConnection, Failure> {
        self.current().await
    }
    async fn save(&self, generation: u64, connection: CloudConnection) -> Result<(), Failure> {
        let mut unsaved = self.renewing.lock().await;
        self.inner.save(generation, connection).await?;
        *unsaved = None;
        Ok(())
    }
    async fn clear(&self) -> Result<(), Failure> {
        let mut unsaved = self.renewing.lock().await;
        let grant = match self.inner.read().await {
            Ok(CloudConnection::Boat(boat)) => boat.grant,
            _ => None,
        };
        self.inner.clear().await?;
        *unsaved = None;
        if let Some(grant) = grant {
            // Disconnecting ends the sign-in in Boat too, as far as Boat
            // can be told; the vault no longer holds it either way.
            if let Ok(refresh) = std::str::from_utf8(grant.refresh.as_slice()) {
                self.oauth.revoke(refresh).await;
            }
        }
        Ok(())
    }
}

/// `base`'s connection, signed in with `tokens` at `now`.
pub(crate) fn signed_in(
    base: &BoatConnection,
    tokens: &BoatTokens,
    login: String,
    now: u64,
) -> Result<BoatConnection, Failure> {
    let expires_at_ms = now.saturating_add(tokens.expires_in.saturating_mul(1000));
    Ok(BoatConnection::new(
        tokens.access.to_string(),
        base.environment.clone(),
        base.image.clone(),
    )?
    .with_grant(BoatGrant::new(
        tokens.refresh.to_string(),
        expires_at_ms,
        login,
    )?))
}

impl Workspaces {
    /// Save the connection Sign in with Boat made, once it validates the way
    /// a pasted token does.
    pub async fn save_signed_in(
        &self,
        tokens: &BoatTokens,
        login: String,
        now: u64,
    ) -> Result<(), Failure> {
        let image = self.image().ok_or(Failure::WorkerUnavailable)?;
        let base = BoatConnection::new(tokens.access.to_string(), "nessa".into(), image)?;
        self.save(CloudConnection::Boat(signed_in(&base, tokens, login, now)?))
            .await
    }
}

/// `bytes` random bytes as URL-safe text: PKCE's verifier alphabet.
fn random_text(bytes: usize) -> Result<String, Failure> {
    let mut buffer = Zeroizing::new(vec![0u8; bytes]);
    getrandom::fill(&mut buffer).map_err(|_| Failure::Provider)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer.as_slice()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
