//! The per-server owner. One step is chosen under the server's lock; the
//! network, the store and the audit run after that lock is released.
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use nessa_sdk::infrastructure::mcp::{Bearer, McpError, RemoteAuthorization};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use super::discovery::{self, DiscoverFailure, Discovered};
use super::ports::{
    AuthAuditRecord, AuthClock, AuthorizationAudit, AuthorizationRecords, AuthorizeAnswer,
    BindingChange, CallbackQuery, ConsentCallback, Entropy, FenceRefusal, ListedAuthorization,
    OAuthCallFailure, OAuthHttp, RecordFailure, ResourceLookup, RevokeAnswer, SessionDrain,
    TokenMaterial,
};
use crate::mcp_authorization::domain::{
    AcceptedDiscovery, Admission, Command, Deletion, Phase, Publication, RefreshActivity, Refusal,
    RemoteObservation, ServerAuth, TokenAvailability, CONSENT_DEADLINE_MS,
};

struct Slot {
    auth: ServerAuth,
    verifier: Option<String>,
    redirect_uri: Option<String>,
    token_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    /// The generation last returned from `bearer`. A rejection of an older
    /// generation reuses a replacement that was published since.
    handed: u64,
}

struct RefreshFlight {
    notify: Notify,
    result: Mutex<Option<Result<Option<Bearer>, McpError>>>,
}

/// Gateway-owned authorization for every remote server.
pub struct AuthorizationOwner {
    slots: Mutex<HashMap<Uuid, Arc<Mutex<Slot>>>>,
    flights: Mutex<HashMap<Uuid, Arc<RefreshFlight>>>,
    records: Arc<dyn AuthorizationRecords>,
    audit: Arc<dyn AuthorizationAudit>,
    http: Arc<dyn OAuthHttp>,
    callback: Arc<dyn ConsentCallback>,
    clock: Arc<dyn AuthClock>,
    entropy: Arc<dyn Entropy>,
    sessions: Arc<dyn SessionDrain>,
    resources: Arc<dyn ResourceLookup>,
    writer: bool,
}

impl AuthorizationOwner {
    pub fn new(
        records: Arc<dyn AuthorizationRecords>,
        audit: Arc<dyn AuthorizationAudit>,
        http: Arc<dyn OAuthHttp>,
        callback: Arc<dyn ConsentCallback>,
        clock: Arc<dyn AuthClock>,
        entropy: Arc<dyn Entropy>,
        sessions: Arc<dyn SessionDrain>,
        resources: Arc<dyn ResourceLookup>,
        writer: bool,
    ) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            records,
            audit,
            http,
            callback,
            clock,
            entropy,
            sessions,
            resources,
            writer,
        }
    }

    pub async fn authorize(
        self: &Arc<Self>,
        server: Uuid,
        name: &str,
        resource: &str,
    ) -> AuthorizeAnswer {
        if !self.writer {
            return AuthorizeAnswer::StoreUnavailable;
        }
        let slot = self.slot(server, name, resource).await;
        let decision = {
            let mut guard = slot.lock().await;
            if guard.auth.resource != resource {
                guard.auth.resource = resource.to_owned();
            }
            guard.auth.name = name.to_owned();
            let decision = guard.auth.step(Command::Authorize {
                store_available: true,
            });
            guard.auth = decision.auth.clone();
            decision
        };
        if let Some(refusal) = decision.refusal {
            return answer_of(refusal);
        }
        if self.persist(&slot).await.is_err() {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        if self.audit_slot(&slot, "authorize", true).await.is_err() {
            self.revert_consent(&slot).await;
            return AuthorizeAnswer::AuditUnavailable;
        }
        let probe = self.http.get(resource).await;
        let challenge = match probe {
            Ok(response) if (200..300).contains(&response.status) => {
                return self.finish_no_auth(&slot).await;
            }
            Ok(response) if response.status == 401 => response.www_authenticate,
            Ok(_) => return self.fail_discovery(&slot, false).await,
            Err(OAuthCallFailure::NotSent) | Err(OAuthCallFailure::Lost) => {
                return self.fail_discovery(&slot, false).await;
            }
        };
        let scope = challenge.as_deref().and_then(discovery::challenge_scope);
        let discovered = match discovery::discover(&self.http, resource, challenge.as_deref()).await
        {
            Ok(discovered) => discovered,
            Err(DiscoverFailure::UnsupportedRegistration) => {
                return self.fail_discovery(&slot, true).await
            }
            Err(_) => return self.fail_discovery(&slot, false).await,
        };
        let accepted = self
            .accept_discovery(&slot, discovered.clone(), scope)
            .await;
        let Some(accepted) = accepted else {
            return self.refusal_now(&slot).await;
        };
        let redirect = match self
            .callback
            .listen(std::time::Duration::from_millis(CONSENT_DEADLINE_MS))
            .await
        {
            Ok(bound) => bound,
            Err(()) => return self.fail_discovery(&slot, false).await,
        };
        let verifier = match self.random() {
            Ok(value) => value,
            Err(()) => return self.fail_discovery(&slot, false).await,
        };
        let client_id = match self.register(&accepted, &redirect.redirect_uri).await {
            Ok(client_id) => client_id,
            Err(AuthorizeAnswer::RegistrationUnsupported) => {
                return self.fail_discovery(&slot, true).await
            }
            Err(other) => return other,
        };
        {
            let mut guard = slot.lock().await;
            let decision = guard.auth.step(Command::Registered { client_id });
            guard.auth = decision.auth;
            guard.verifier = Some(verifier.clone());
            guard.redirect_uri = Some(redirect.redirect_uri.clone());
            guard.token_endpoint = Some(accepted.token_endpoint.clone());
            guard.revocation_endpoint = accepted.revocation_endpoint.clone();
            guard.auth.token_endpoint = Some(accepted.token_endpoint.clone());
            guard.auth.revocation_endpoint = accepted.revocation_endpoint.clone();
            guard.auth.authorization_endpoint = Some(accepted.authorization_endpoint.clone());
        }
        let state = match self.random() {
            Ok(value) => value,
            Err(()) => return self.fail_discovery(&slot, false).await,
        };
        let consent_url = consent_url(
            &accepted,
            &client_from(&slot).await,
            &redirect.redirect_uri,
            &verifier,
            &state,
            resource,
        );
        let deadline = self.deadline();
        {
            let mut guard = slot.lock().await;
            let decision = guard.auth.step(Command::ConsentReady {
                state: state.clone(),
                deadline_ms: deadline,
            });
            guard.auth = decision.auth;
        }
        if self.persist(&slot).await.is_err() {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let attempt_id = {
            let guard = slot.lock().await;
            guard
                .auth
                .attempt
                .as_ref()
                .map(|attempt| attempt.id.to_string())
                .unwrap_or_default()
        };
        let owner = Arc::clone(self);
        tokio::spawn(async move {
            if let Ok(query) = redirect.accepted.await {
                let _ = owner.complete_callback(server, query).await;
            }
        });
        AuthorizeAnswer::PendingConsent {
            attempt_id,
            consent_url,
            deadline_ms: deadline,
        }
    }

    pub async fn complete_callback(&self, server: Uuid, query: CallbackQuery) -> AuthorizeAnswer {
        let Some(slot) = self.existing(server).await else {
            return AuthorizeAnswer::DiscoveryFailed;
        };
        let now = self.clock.now_ms();
        let decision = {
            let mut guard = slot.lock().await;
            let decision = guard.auth.step(Command::Callback {
                state: query.state,
                now_ms: now,
                denied: query.denied || query.code.is_none(),
            });
            guard.auth = decision.auth.clone();
            decision
        };
        if let Some(refusal) = decision.refusal {
            let _ = self.persist(&slot).await;
            return match refusal {
                Refusal::StaleCallback | Refusal::Expired | Refusal::Denied => {
                    AuthorizeAnswer::DiscoveryFailed
                }
                Refusal::AlreadyReady => {
                    let generation = slot.lock().await.auth.generation;
                    AuthorizeAnswer::Ready { generation }
                }
                _ => self.refusal_now(&slot).await,
            };
        }
        if self.persist(&slot).await.is_err() {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        if self.audit_slot(&slot, "exchange", true).await.is_err() {
            let _ = self.apply(&slot, Command::ExchangeUncertain).await;
            let _ = self.persist(&slot).await;
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let Some(code) = query.code else {
            return self.refusal_now(&slot).await;
        };
        self.exchange(&slot, &code).await
    }

    pub async fn revoke(&self, server: Uuid) -> RevokeAnswer {
        let Some(slot) = self.existing(server).await else {
            return RevokeAnswer {
                settled: true,
                local_drained: true,
                secret_deleted: true,
                remote: Some(RemoteObservation::Unsupported),
                evidence_acknowledged: true,
            };
        };
        self.revoke_slot(&slot, Command::Revoke).await
    }

    /// Fence a configured remote whose stored resource is not the URL it
    /// would be called on. A matching binding is left alone. Called at
    /// startup, before the process accepts a session, so a URL written
    /// while this process was down is not called with the previous token.
    pub async fn revalidate(&self, remotes: &[(Uuid, String)]) {
        for (id, url) in remotes {
            let Some(slot) = self.existing(*id).await else {
                continue;
            };
            let bound = slot.lock().await.auth.resource.clone();
            if bound == *url {
                continue;
            }
            let _ = self
                .fence(&[BindingChange::ResourceChanged {
                    id: *id,
                    previous_url: bound,
                    url: url.clone(),
                }])
                .await;
        }
    }

    pub async fn fence(&self, changes: &[BindingChange]) -> Result<(), FenceRefusal> {
        for change in changes {
            let (id, command) = match change {
                BindingChange::ResourceChanged { id, .. } => {
                    (*id, Command::ResourceChanged { url: String::new() })
                }
                BindingChange::Removed { id, .. } => (*id, Command::Removed),
            };
            let Some(slot) = self.existing(id).await else {
                continue;
            };
            let answer = self.revoke_slot(&slot, command).await;
            if !answer.settled {
                return Err(FenceRefusal);
            }
        }
        Ok(())
    }

    pub async fn facts(&self, server: Uuid) -> Option<ListedAuthorization> {
        let slot = self.existing(server).await?;
        let guard = slot.lock().await;
        Some(listed(&guard.auth, self.clock.now_ms()))
    }

    pub async fn acknowledge_domains(&self, server: Uuid, digest: &str) -> Result<(), ()> {
        let Some(slot) = self.existing(server).await else {
            return Err(());
        };
        self.apply(
            &slot,
            Command::AcknowledgeDomains {
                digest: digest.to_owned(),
            },
        )
        .await;
        self.persist(&slot).await.map_err(|_| ())?;
        self.audit_slot(&slot, "domains", false)
            .await
            .map_err(|_| ())
    }

    async fn exchange(&self, slot: &Arc<Mutex<Slot>>, code: &str) -> AuthorizeAnswer {
        let (endpoint, verifier, redirect, client_id, resource) = {
            let guard = slot.lock().await;
            (
                guard.token_endpoint.clone(),
                guard.verifier.clone(),
                guard.redirect_uri.clone(),
                guard.auth.client_id.clone(),
                guard.auth.resource.clone(),
            )
        };
        let (Some(endpoint), Some(verifier), Some(redirect), Some(client_id)) =
            (endpoint, verifier, redirect, client_id)
        else {
            let _ = self.apply(slot, Command::ExchangeUncertain).await;
            let _ = self.persist(slot).await;
            return AuthorizeAnswer::AuthorizationIncomplete;
        };
        if !discovery::https_url(&endpoint) {
            let _ = self.apply(slot, Command::ExchangeRefused).await;
            return AuthorizeAnswer::DiscoveryFailed;
        }
        let body = discovery::form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
            ("resource", &resource),
        ]);
        let response = match self.http.post_form(&endpoint, &body).await {
            Ok(response) => response,
            Err(OAuthCallFailure::NotSent) => {
                let _ = self.apply(slot, Command::ExchangeRefused).await;
                let _ = self.persist(slot).await;
                return AuthorizeAnswer::DiscoveryFailed;
            }
            Err(OAuthCallFailure::Lost) => {
                let _ = self.apply(slot, Command::ExchangeUncertain).await;
                let _ = self.persist(slot).await;
                return AuthorizeAnswer::AuthorizationIncomplete;
            }
        };
        self.publish_token(slot, &response.body, false).await
    }

    async fn publish_token(
        &self,
        slot: &Arc<Mutex<Slot>>,
        body: &str,
        refresh: bool,
    ) -> AuthorizeAnswer {
        let parsed = parse_token(body);
        if parsed.error.as_deref() == Some("invalid_grant") {
            if refresh {
                let answer = self.revoke_slot(slot, Command::InvalidGrant).await;
                return if answer.settled {
                    AuthorizeAnswer::DiscoveryFailed
                } else {
                    AuthorizeAnswer::AuthorizationIncomplete
                };
            }
            let _ = self.apply(slot, Command::ExchangeRefused).await;
            let _ = self.persist(slot).await;
            return AuthorizeAnswer::DiscoveryFailed;
        }
        let Some(access) = parsed.access else {
            let command = if refresh {
                Command::RefreshLost
            } else {
                Command::ExchangeUncertain
            };
            let _ = self.apply(slot, command).await;
            let _ = self.persist(slot).await;
            return AuthorizeAnswer::AuthorizationIncomplete;
        };
        let generation = {
            let guard = slot.lock().await;
            if refresh {
                guard.auth.generation + 1
            } else {
                guard.auth.generation.max(1)
            }
        };
        let expires_at_ms = parsed.expires_in.map(|seconds| {
            self.clock
                .now_ms()
                .saturating_add(seconds.saturating_mul(1000))
        });
        let material = TokenMaterial {
            access_token: access,
            refresh_token: parsed.refresh,
            generation,
        };
        let publication = match self
            .records
            .store_secret(server_of(slot).await, &material)
            .await
        {
            Ok(publication) => publication,
            Err(RecordFailure::Unavailable) => Publication::Unknown,
        };
        let from = {
            let guard = slot.lock().await;
            guard.auth.generation
        };
        let command = if refresh {
            Command::RefreshPublication {
                from_generation: from,
                publication,
                expires_at_ms,
            }
        } else {
            Command::SecretPublication {
                publication,
                generation,
                expires_at_ms,
            }
        };
        self.apply(slot, command).await;
        let acked = self.audit_slot(slot, "store", false).await.is_ok();
        self.apply(slot, Command::Evidence { acked }).await;
        if self.persist(slot).await.is_err() {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let guard = slot.lock().await;
        match guard.auth.phase {
            Phase::Ready {
                availability: TokenAvailability::Usable,
                ..
            } => AuthorizeAnswer::Ready {
                generation: guard.auth.generation,
            },
            Phase::AuthorizationIncomplete { .. } => AuthorizeAnswer::AuthorizationIncomplete,
            Phase::ConsentNeeded => AuthorizeAnswer::DiscoveryFailed,
            _ => AuthorizeAnswer::AuthorizationIncomplete,
        }
    }

    async fn revoke_slot(&self, slot: &Arc<Mutex<Slot>>, command: Command) -> RevokeAnswer {
        let decision = self.apply(slot, command).await;
        if decision.refusal == Some(Refusal::JoinRevoke) {
            return snapshot(slot).await;
        }
        let _ = self.persist(slot).await;
        let _ = self.audit_slot(slot, "revoke", true).await;
        let name = {
            let guard = slot.lock().await;
            guard.auth.name.clone()
        };
        self.sessions.drain(&name).await;
        self.apply(slot, Command::LocalDrained).await;
        let observation = self.revoke_remote(slot).await;
        self.apply(slot, Command::RemoteObserved(observation)).await;
        let deletion = match self.records.delete_secret(server_of(slot).await).await {
            Ok(deletion) => deletion,
            Err(RecordFailure::Unavailable) => Deletion::Unknown,
        };
        self.apply(slot, Command::SecretDeletion { deletion }).await;
        let acked = self.audit_slot(slot, "revoke", false).await.is_ok();
        self.apply(slot, Command::Evidence { acked }).await;
        let _ = self.persist(slot).await;
        snapshot(slot).await
    }

    async fn revoke_remote(&self, slot: &Arc<Mutex<Slot>>) -> RemoteObservation {
        let (endpoint, server) = {
            let guard = slot.lock().await;
            (guard.revocation_endpoint.clone(), guard.auth.server)
        };
        let Some(endpoint) = endpoint.filter(|url| discovery::https_url(url)) else {
            return RemoteObservation::Unsupported;
        };
        let Some(secret) = self.records.load_secret(server).await.ok().flatten() else {
            return RemoteObservation::Unconfirmed;
        };
        let token = secret
            .refresh_token
            .as_deref()
            .unwrap_or(&secret.access_token);
        let body = discovery::form(&[("token", token), ("token_type_hint", "refresh_token")]);
        match self.http.post_form(&endpoint, &body).await {
            Ok(response) if (200..300).contains(&response.status) => {
                RemoteObservation::Acknowledged
            }
            Ok(_) | Err(_) => RemoteObservation::Unconfirmed,
        }
    }

    async fn refresh(&self, server: Uuid, rejected: bool) -> Result<Option<Bearer>, McpError> {
        let Some(slot) = self.existing(server).await else {
            return Err(McpError::Unauthorized);
        };
        let flight = {
            let mut guard = slot.lock().await;
            if !rejected {
                if let Admission::Bearer { generation } = self.live_admission(&guard, server) {
                    guard.handed = generation;
                    drop(guard);
                    return self.release_bearer(&slot, server, generation).await;
                }
            } else if guard.auth.generation > guard.handed {
                if let Admission::Bearer { generation } = self.live_admission(&guard, server) {
                    drop(guard);
                    return self.release_bearer(&slot, server, generation).await;
                }
            }
            let decision = guard.auth.step(Command::RefreshRequested {
                now_ms: self.clock.now_ms(),
                rejected,
            });
            if decision.refusal == Some(Refusal::JoinRefresh) {
                drop(guard);
                let flight = self.flights.lock().await.get(&server).cloned();
                if let Some(flight) = flight {
                    let notified = flight.notify.notified();
                    if let Some(result) = flight.result.lock().await.clone() {
                        return result;
                    }
                    notified.await;
                    return flight
                        .result
                        .lock()
                        .await
                        .clone()
                        .unwrap_or(Err(McpError::Unauthorized));
                }
                return Err(McpError::Unauthorized);
            }
            if decision.refusal.is_some() {
                guard.auth = decision.auth;
                return Err(McpError::Unauthorized);
            }
            guard.auth = decision.auth;
            let flight = Arc::new(RefreshFlight {
                notify: Notify::new(),
                result: Mutex::new(None),
            });
            self.flights.lock().await.insert(server, flight.clone());
            flight
        };
        if self.audit_slot(&slot, "refresh", true).await.is_err() {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched)
                .await;
            return Err(McpError::Unauthorized);
        }
        let (endpoint, client_id, resource, generation) = {
            let guard = slot.lock().await;
            (
                guard.token_endpoint.clone(),
                guard.auth.client_id.clone(),
                guard.auth.resource.clone(),
                guard.auth.generation,
            )
        };
        let refresh_token = self
            .records
            .load_secret(server)
            .await
            .ok()
            .flatten()
            .and_then(|secret| secret.refresh_token);
        let Some(endpoint) = endpoint.filter(|url| discovery::https_url(url)) else {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched)
                .await;
            return Err(McpError::Unauthorized);
        };
        let Some(refresh_token) = refresh_token else {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched)
                .await;
            return Err(McpError::Unauthorized);
        };
        {
            let mut guard = slot.lock().await;
            guard.auth.refresh_dispatched = true;
        }
        if self.persist(&slot).await.is_err() {
            self.finish_refresh(&slot, &flight, Command::RefreshLost)
                .await;
            return Err(McpError::Unauthorized);
        }
        let body = discovery::form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", client_id.as_deref().unwrap_or("")),
            ("resource", &resource),
        ]);
        let _ = generation;
        let outcome = match self.http.post_form(&endpoint, &body).await {
            Err(OAuthCallFailure::NotSent) => {
                self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched)
                    .await;
                return Err(McpError::Unauthorized);
            }
            Err(OAuthCallFailure::Lost) => {
                self.finish_refresh(&slot, &flight, Command::RefreshLost)
                    .await;
                return Err(McpError::Unauthorized);
            }
            Ok(response) => response,
        };
        let answer = self.publish_token(&slot, &outcome.body, true).await;
        let result = match answer {
            AuthorizeAnswer::Ready { generation } => {
                self.release_bearer(&slot, server, generation).await
            }
            _ => Err(McpError::Unauthorized),
        };
        *flight.result.lock().await = Some(result.clone());
        self.flights.lock().await.remove(&server);
        flight.notify.notify_waiters();
        result
    }

    async fn finish_refresh(
        &self,
        slot: &Arc<Mutex<Slot>>,
        flight: &RefreshFlight,
        command: Command,
    ) {
        self.apply(slot, command).await;
        let _ = self.persist(slot).await;
        *flight.result.lock().await = Some(Err(McpError::Unauthorized));
        let server = server_of(slot).await;
        self.flights.lock().await.remove(&server);
        flight.notify.notify_waiters();
    }

    /// What `bearer` may do against the URL configured now. No configured
    /// URL refuses a token.
    fn live_admission(&self, guard: &Slot, server: Uuid) -> Admission {
        if matches!(guard.auth.phase, Phase::Unauthenticated) {
            return Admission::NoneRequired;
        }
        match self.resources.resource(server) {
            Some(resource) => guard.auth.admission(self.clock.now_ms(), &resource),
            None => Admission::Unauthorized,
        }
    }

    /// Load the secret after the slot lock is released, then return it only
    /// when the same generation is still admitted for the live URL.
    async fn release_bearer(
        &self,
        slot: &Arc<Mutex<Slot>>,
        server: Uuid,
        generation: u64,
    ) -> Result<Option<Bearer>, McpError> {
        let loaded = self.records.load_secret(server).await;
        let guard = slot.lock().await;
        match self.live_admission(&guard, server) {
            Admission::Bearer {
                generation: current,
            } if current == generation => match loaded {
                Ok(Some(secret)) if secret.generation == generation => Ok(Some(secret.bearer())),
                Ok(Some(_)) | Ok(None) => Err(McpError::Unauthorized),
                Err(RecordFailure::Unavailable) => Err(McpError::Unreachable),
            },
            Admission::Bearer { .. } => Err(McpError::Unauthorized),
            Admission::InsufficientScope => Err(McpError::InsufficientScope),
            Admission::NoneRequired => Ok(None),
            Admission::NeedsRefresh | Admission::Unauthorized => Err(McpError::Unauthorized),
        }
    }

    async fn accept_discovery(
        &self,
        slot: &Arc<Mutex<Slot>>,
        discovered: Discovered,
        scope: Option<String>,
    ) -> Option<Discovered> {
        let scopes = match scope {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => discovered.scopes.clone(),
        };
        let decision = self
            .apply(
                slot,
                Command::DiscoveryAccepted(AcceptedDiscovery {
                    issuer: discovered.issuer.clone(),
                    scopes,
                    pkce_s256: discovered.pkce_s256,
                    registration_offered: discovered.registration_endpoint.is_some(),
                    revocation_offered: discovered.revocation_endpoint.is_some(),
                    resource_matches: discovered.resource_matches,
                    issuer_matches: discovered.issuer_matches,
                }),
            )
            .await;
        decision.refusal.is_none().then_some(discovered)
    }

    async fn register(
        &self,
        discovered: &Discovered,
        redirect: &str,
    ) -> Result<String, AuthorizeAnswer> {
        let Some(endpoint) = &discovered.registration_endpoint else {
            return Err(AuthorizeAnswer::RegistrationUnsupported);
        };
        if !discovery::https_url(endpoint) {
            return Err(AuthorizeAnswer::DiscoveryFailed);
        }
        let body = serde_json::json!({
            "redirect_uris": [redirect],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "client_name": "Nessa",
        })
        .to_string();
        match self.http.post_form(endpoint, &body).await {
            Ok(response) if response.status == 201 || response.status == 200 => {
                let client_id = serde_json::from_str::<serde_json::Value>(&response.body)
                    .ok()
                    .and_then(|value| value.get("client_id")?.as_str().map(str::to_owned));
                client_id.ok_or(AuthorizeAnswer::DiscoveryFailed)
            }
            Ok(_) => Err(AuthorizeAnswer::RegistrationUnsupported),
            Err(_) => Err(AuthorizeAnswer::DiscoveryFailed),
        }
    }

    async fn finish_no_auth(&self, slot: &Arc<Mutex<Slot>>) -> AuthorizeAnswer {
        self.apply(slot, Command::ProbedNoAuth).await;
        let _ = self.persist(slot).await;
        let _ = self.audit_slot(slot, "authorize", false).await;
        AuthorizeAnswer::NotRequired
    }

    async fn fail_discovery(&self, slot: &Arc<Mutex<Slot>>, unsupported: bool) -> AuthorizeAnswer {
        self.apply(
            slot,
            Command::DiscoveryFailed {
                unsupported_registration: unsupported,
            },
        )
        .await;
        let _ = self.persist(slot).await;
        if unsupported {
            AuthorizeAnswer::RegistrationUnsupported
        } else {
            AuthorizeAnswer::DiscoveryFailed
        }
    }

    async fn revert_consent(&self, slot: &Arc<Mutex<Slot>>) {
        let mut guard = slot.lock().await;
        guard.auth.phase = Phase::ConsentNeeded;
        guard.auth.attempt = None;
    }

    async fn apply(
        &self,
        slot: &Arc<Mutex<Slot>>,
        command: Command,
    ) -> crate::mcp_authorization::domain::Decision {
        let mut guard = slot.lock().await;
        let decision = guard.auth.step(command);
        guard.auth = decision.auth.clone();
        decision
    }

    async fn persist(&self, slot: &Arc<Mutex<Slot>>) -> Result<(), RecordFailure> {
        let auth = slot.lock().await.auth.clone();
        self.records.store(&auth).await
    }

    async fn audit_slot(
        &self,
        slot: &Arc<Mutex<Slot>>,
        action: &'static str,
        intent: bool,
    ) -> Result<(), super::ports::AuditFailure> {
        let guard = slot.lock().await;
        self.audit
            .record(&AuthAuditRecord {
                server: guard.auth.server,
                action,
                intent,
                generation: guard.auth.generation,
                resource: guard.auth.resource.clone(),
                phase: phase_name(&guard.auth.phase).to_owned(),
            })
            .await
    }

    async fn slot(&self, server: Uuid, name: &str, resource: &str) -> Arc<Mutex<Slot>> {
        let mut slots = self.slots.lock().await;
        if let Some(slot) = slots.get(&server) {
            return slot.clone();
        }
        let auth = match self.records.load(server).await {
            Ok(Some(mut loaded)) => {
                let secret_present = self
                    .records
                    .load_secret(server)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
                let restarted = loaded.step(Command::Restart {
                    now_ms: self.clock.now_ms(),
                    secret_present,
                    resource_now: Some(resource.to_owned()),
                });
                loaded = restarted.auth;
                loaded.name = name.to_owned();
                loaded
            }
            _ => ServerAuth::consent_needed(server, name, resource),
        };
        let token_endpoint = auth.token_endpoint.clone();
        let revocation_endpoint = auth.revocation_endpoint.clone();
        let slot = Arc::new(Mutex::new(Slot {
            auth,
            verifier: None,
            redirect_uri: None,
            token_endpoint,
            revocation_endpoint,
            handed: 0,
        }));
        slots.insert(server, slot.clone());
        slot
    }

    async fn existing(&self, server: Uuid) -> Option<Arc<Mutex<Slot>>> {
        if let Some(slot) = self.slots.lock().await.get(&server).cloned() {
            return Some(slot);
        }
        let loaded = self.records.load(server).await.ok()??;
        let secret_present = self
            .records
            .load_secret(server)
            .await
            .ok()
            .flatten()
            .is_some();
        let resource = self.resources.resource(server);
        let restarted = loaded.step(Command::Restart {
            now_ms: self.clock.now_ms(),
            secret_present,
            resource_now: resource,
        });
        let token_endpoint = restarted.auth.token_endpoint.clone();
        let revocation_endpoint = restarted.auth.revocation_endpoint.clone();
        let slot = Arc::new(Mutex::new(Slot {
            auth: restarted.auth,
            verifier: None,
            redirect_uri: None,
            token_endpoint,
            revocation_endpoint,
            handed: 0,
        }));
        self.slots.lock().await.insert(server, slot.clone());
        Some(slot)
    }

    async fn refusal_now(&self, slot: &Arc<Mutex<Slot>>) -> AuthorizeAnswer {
        let guard = slot.lock().await;
        match guard.auth.phase {
            Phase::Ready { .. } => AuthorizeAnswer::Ready {
                generation: guard.auth.generation,
            },
            Phase::AuthorizationIncomplete { .. } => AuthorizeAnswer::AuthorizationIncomplete,
            Phase::PendingConsent { .. } => AuthorizeAnswer::Busy,
            _ => AuthorizeAnswer::DiscoveryFailed,
        }
    }

    fn deadline(&self) -> u64 {
        self.clock.now_ms().saturating_add(CONSENT_DEADLINE_MS)
    }

    fn random(&self) -> Result<String, ()> {
        Ok(URL_SAFE_NO_PAD.encode(self.entropy.bytes(32)?))
    }
}

fn consent_url(
    accepted: &Discovered,
    client_id: &str,
    redirect: &str,
    verifier: &str,
    state: &str,
    resource: &str,
) -> String {
    build_consent(accepted, client_id, redirect, verifier, state, resource)
}

fn build_consent(
    accepted: &Discovered,
    client_id: &str,
    redirect: &str,
    verifier: &str,
    state: &str,
    resource: &str,
) -> String {
    let challenge = discovery::s256(verifier);
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("resource", resource),
    ];
    let scope = accepted.scopes.join(" ");
    if !scope.is_empty() {
        pairs.push(("scope", scope.as_str()));
    }
    format!(
        "{}?{}",
        accepted.authorization_endpoint,
        discovery::form(&pairs)
    )
}

async fn client_from(slot: &Arc<Mutex<Slot>>) -> String {
    slot.lock().await.auth.client_id.clone().unwrap_or_default()
}

async fn server_of(slot: &Arc<Mutex<Slot>>) -> Uuid {
    slot.lock().await.auth.server
}

async fn snapshot(slot: &Arc<Mutex<Slot>>) -> RevokeAnswer {
    let guard = slot.lock().await;
    let (settled, local_drained, secret_deleted, remote, evidence_acknowledged) = match &guard
        .auth
        .phase
    {
        Phase::ConsentNeeded => (true, true, true, guard_remote(&guard.auth), true),
        Phase::RevocationIncomplete { settlement, .. } | Phase::Revoking { settlement, .. } => (
            false,
            settlement.local_drained,
            settlement.secret_deleted,
            settlement.remote,
            settlement.evidence_acked,
        ),
        _ => (false, false, !guard.auth.secret_present, None, false),
    };
    let _ = secret_deleted;
    RevokeAnswer {
        settled,
        local_drained,
        secret_deleted: !guard.auth.secret_present,
        remote: remote.or(guard_remote(&guard.auth)),
        evidence_acknowledged,
    }
}

fn guard_remote(auth: &ServerAuth) -> Option<RemoteObservation> {
    match auth.phase {
        Phase::Revoking { settlement, .. } | Phase::RevocationIncomplete { settlement, .. } => {
            settlement.remote
        }
        _ => None,
    }
}

fn listed(auth: &ServerAuth, now_ms: u64) -> ListedAuthorization {
    let (phase, token_expired, refresh_failing, scope_required, remote) = match &auth.phase {
        Phase::Unauthenticated => ("unauthenticated", false, false, false, None),
        Phase::ConsentNeeded => ("consent_needed", false, false, false, None),
        Phase::Discovering { .. } => ("consent_needed", false, false, false, None),
        Phase::PendingConsent { .. } => ("pending_consent", false, false, false, None),
        Phase::Exchanging { .. } => ("authorization_incomplete", false, false, false, None),
        Phase::Ready {
            availability,
            refresh,
            scope_required,
        } => (
            if *scope_required {
                "scope_required"
            } else {
                "ready"
            },
            matches!(availability, TokenAvailability::Unavailable)
                || auth.expires_at_ms.is_some_and(|expires| now_ms >= expires),
            matches!(refresh, RefreshActivity::Refreshing),
            *scope_required,
            None,
        ),
        Phase::AuthorizationIncomplete { obligation } => (
            "authorization_incomplete",
            false,
            matches!(
                obligation,
                crate::mcp_authorization::domain::Obligation::RefreshUnknown
            ),
            false,
            None,
        ),
        Phase::Revoking { settlement, .. } => ("revoking", false, false, false, settlement.remote),
        Phase::RevocationIncomplete { settlement, .. } => (
            "revocation_incomplete",
            false,
            false,
            false,
            settlement.remote,
        ),
    };
    ListedAuthorization {
        phase,
        generation: auth.generation,
        token_expired,
        refresh_failing,
        scope_required,
        remote,
        domains_digest: auth.acknowledged_domains.clone(),
    }
}

fn phase_name(phase: &Phase) -> &'static str {
    match phase {
        Phase::Unauthenticated => "unauthenticated",
        Phase::ConsentNeeded => "consent_needed",
        Phase::Discovering { .. } => "discovering",
        Phase::PendingConsent { .. } => "pending_consent",
        Phase::Exchanging { .. } => "exchanging",
        Phase::Ready { .. } => "ready",
        Phase::AuthorizationIncomplete { .. } => "authorization_incomplete",
        Phase::Revoking { .. } => "revoking",
        Phase::RevocationIncomplete { .. } => "revocation_incomplete",
    }
}

fn answer_of(refusal: Refusal) -> AuthorizeAnswer {
    match refusal {
        Refusal::StoreUnavailable => AuthorizeAnswer::StoreUnavailable,
        Refusal::AlreadyReady => AuthorizeAnswer::Ready { generation: 0 },
        Refusal::RegistrationUnsupported => AuthorizeAnswer::RegistrationUnsupported,
        Refusal::DiscoveryFailed | Refusal::BindingRejected | Refusal::ExchangeRefused => {
            AuthorizeAnswer::DiscoveryFailed
        }
        Refusal::ReconciliationRequired | Refusal::Busy => AuthorizeAnswer::Busy,
        Refusal::AuditUnavailable => AuthorizeAnswer::AuditUnavailable,
        _ => AuthorizeAnswer::DiscoveryFailed,
    }
}

struct ParsedToken {
    access: Option<String>,
    refresh: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

fn parse_token(body: &str) -> ParsedToken {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return ParsedToken {
            access: None,
            refresh: None,
            expires_in: None,
            error: None,
        };
    };
    ParsedToken {
        access: value
            .get("access_token")
            .and_then(|token| token.as_str())
            .map(str::to_owned),
        refresh: value
            .get("refresh_token")
            .and_then(|token| token.as_str())
            .map(str::to_owned),
        expires_in: value.get("expires_in").and_then(|seconds| seconds.as_u64()),
        error: value
            .get("error")
            .and_then(|error| error.as_str())
            .map(str::to_owned),
    }
}

#[async_trait]
impl crate::mcp_authorization::application::AuthorizationHandoff for AuthorizationOwner {
    async fn fence(
        &self,
        changes: &[crate::mcp_authorization::application::BindingChange],
    ) -> Result<(), crate::mcp_authorization::application::FenceRefusal> {
        AuthorizationOwner::fence(self, changes).await
    }
}

#[async_trait]
impl RemoteAuthorization for AuthorizationOwner {
    async fn bearer(&self, server: Uuid) -> Result<Option<Bearer>, McpError> {
        let Some(slot) = self.existing(server).await else {
            return Ok(None);
        };
        let admission = {
            let guard = slot.lock().await;
            self.live_admission(&guard, server)
        };
        match admission {
            Admission::NoneRequired => Ok(None),
            Admission::Bearer { generation } => {
                self.release_bearer(&slot, server, generation).await
            }
            Admission::InsufficientScope => Err(McpError::InsufficientScope),
            Admission::NeedsRefresh => self.refresh(server, false).await,
            Admission::Unauthorized => Err(McpError::Unauthorized),
        }
    }

    async fn rejected(
        &self,
        server: Uuid,
        _www_authenticate: &str,
    ) -> Result<Option<Bearer>, McpError> {
        self.refresh(server, true).await
    }

    async fn insufficient_scope(&self, server: Uuid, _www_authenticate: &str) {
        if let Some(slot) = self.existing(server).await {
            self.apply(&slot, Command::InsufficientScope).await;
            let _ = self.persist(&slot).await;
            let _ = self.audit_slot(&slot, "scope", false).await;
        }
    }
}
