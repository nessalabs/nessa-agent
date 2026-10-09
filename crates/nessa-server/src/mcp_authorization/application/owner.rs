//! The per-server owner. One step is chosen under the server's lock; the
//! network, the store and the audit run after that lock is released.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use tokio::sync::{mpsc::Receiver, Mutex, Notify};
use uuid::Uuid;

use super::discovery::{self, DiscoverFailure, Discovered};
use super::ports::{
    AdmissionRefusal, AdmittedToken, AuthAuditRecord, AuthClock, AuthorizationAudit,
    AuthorizationRecords, AuthorizeAnswer, BindingChange, CallbackQuery, ConsentCallback, Entropy,
    FenceRefusal, ListedAuthorization, OAuthCallFailure, OAuthHttp, RecordFailure, ResourceLookup,
    RevokeAnswer, SessionDrain, TokenMaterial,
};
use crate::mcp_authorization::domain::{
    AcceptedDiscovery, Admission, Command, Deletion, Effect, Phase, Publication, RefreshActivity,
    Refusal, RemoteObservation, ServerAuth, TokenAvailability, CONSENT_DEADLINE_MS,
};

/// What a load found. Absent is a missing record. Unavailable is a record
/// that could not be read, and must not be treated as absent.
enum Recalled {
    Present(Arc<Mutex<Slot>>),
    Absent,
    Unavailable,
}

/// Attempt and resource captured before the token POST. The reply is
/// stored only when this pair is still the live one.
struct Reply {
    attempt: u64,
    resource: String,
    refresh: Option<u64>,
}

/// Token-endpoint inputs captured with the callback, so a later URL
/// replace cannot send this code to the replacement authorization server.
struct PendingExchange {
    reply: Reply,
    endpoint: Option<String>,
    verifier: Option<String>,
    redirect: Option<String>,
    client_id: Option<String>,
}

/// Admission is decided once under the domain lock. The stream can release
/// its listener before an accepted callback starts any external effect.
enum AuthCallbackOutcome {
    PendingStale(Arc<Mutex<Slot>>),
    Refused {
        slot: Arc<Mutex<Slot>>,
        refusal: Refusal,
    },
    Answer(AuthorizeAnswer),
    Exchange {
        slot: Arc<Mutex<Slot>>,
        code: String,
        prepared: PendingExchange,
    },
}

impl Reply {
    fn exchange(attempt: u64, resource: String) -> Self {
        Self {
            attempt,
            resource,
            refresh: None,
        }
    }

    fn refresh(generation: u64, resource: String) -> Self {
        Self {
            attempt: 0,
            resource,
            refresh: Some(generation),
        }
    }

    fn current(&self, auth: &ServerAuth) -> bool {
        match self.refresh {
            Some(generation) => auth.refresh_reply_current(generation, &self.resource),
            None => auth.exchange_reply_current(self.attempt, &self.resource),
        }
    }
}

struct Slot {
    auth: ServerAuth,
    verifier: Option<String>,
    redirect_uri: Option<String>,
    token_endpoint: Option<String>,
    revocation_endpoint: Option<String>,
    /// The generation last returned from `bearer`. A rejection of an older
    /// generation reuses a replacement that was published since.
    handed: u64,
    /// Set while this process is driving revoke cleanup. A saved `Revoking`
    /// record with this clear has no worker, so the next revoke resumes it.
    revoke_running: Arc<AtomicBool>,
}

struct RefreshFlight {
    notify: Notify,
    result: Mutex<Option<Result<Option<AdmittedToken>, AdmissionRefusal>>>,
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
    #[allow(clippy::too_many_arguments)]
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
        let slot = match self.slot(server, name, resource).await {
            Ok(slot) => slot,
            Err(RecordFailure::Unavailable) => return AuthorizeAnswer::AuthorizationIncomplete,
        };
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
            Ok(response) if response.status == 401 => response.www_authenticate,
            // A definite answer other than 401 did not ask for a token. A
            // streamable server often answers 405 to this GET.
            Ok(_) => return self.finish_no_auth(&slot).await,
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
            owner.receive_callbacks(server, redirect.candidates).await;
        });
        AuthorizeAnswer::PendingConsent {
            attempt_id,
            consent_url,
            deadline_ms: deadline,
        }
    }

    pub async fn complete_callback(&self, server: Uuid, query: CallbackQuery) -> AuthorizeAnswer {
        let admission = self.admit_callback(server, query).await;
        self.finish_callback(admission).await
    }

    async fn receive_callbacks(&self, server: Uuid, mut candidates: Receiver<CallbackQuery>) {
        while let Some(query) = candidates.recv().await {
            let admission = self.admit_callback(server, query).await;
            if matches!(admission, AuthCallbackOutcome::PendingStale(_)) {
                let _ = self.finish_callback(admission).await;
                continue;
            }
            drop(candidates);
            let _ = self.finish_callback(admission).await;
            return;
        }
    }

    async fn admit_callback(&self, server: Uuid, query: CallbackQuery) -> AuthCallbackOutcome {
        let slot = match self.existing(server).await {
            Recalled::Present(slot) => slot,
            Recalled::Absent => {
                return AuthCallbackOutcome::Answer(AuthorizeAnswer::DiscoveryFailed)
            }
            Recalled::Unavailable => {
                return AuthCallbackOutcome::Answer(AuthorizeAnswer::AuthorizationIncomplete)
            }
        };
        let now = self.clock.now_ms();
        let configured = self.resources.resource(server);
        let outcome = {
            let mut guard = slot.lock().await;
            let resource = configured.unwrap_or_else(|| guard.auth.resource.clone());
            let decision = guard.auth.step(Command::Callback {
                state: query.state,
                now_ms: now,
                denied: query.denied || query.code.is_none(),
                resource,
            });
            guard.auth = decision.auth.clone();
            match decision.refusal {
                Some(Refusal::StaleCallback)
                    if matches!(decision.auth.phase, Phase::PendingConsent { .. }) =>
                {
                    AuthCallbackOutcome::PendingStale(Arc::clone(&slot))
                }
                Some(refusal) => AuthCallbackOutcome::Refused {
                    slot: Arc::clone(&slot),
                    refusal,
                },
                None => match (decision.auth.phase, query.code) {
                    (Phase::Exchanging { attempt }, Some(code)) => AuthCallbackOutcome::Exchange {
                        slot: Arc::clone(&slot),
                        code,
                        prepared: PendingExchange {
                            reply: Reply::exchange(attempt, decision.auth.resource.clone()),
                            endpoint: guard.token_endpoint.clone(),
                            verifier: guard.verifier.clone(),
                            redirect: guard.redirect_uri.clone(),
                            client_id: guard.auth.client_id.clone(),
                        },
                    },
                    _ => AuthCallbackOutcome::Refused {
                        slot: Arc::clone(&slot),
                        refusal: Refusal::StaleCallback,
                    },
                },
            }
        };
        outcome
    }

    async fn finish_callback(&self, outcome: AuthCallbackOutcome) -> AuthorizeAnswer {
        let (slot, code, prepared) = match outcome {
            AuthCallbackOutcome::PendingStale(slot) => {
                let _ = self.persist(&slot).await;
                return AuthorizeAnswer::DiscoveryFailed;
            }
            AuthCallbackOutcome::Refused { slot, refusal } => {
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
            AuthCallbackOutcome::Exchange {
                slot,
                code,
                prepared,
            } => (slot, code, prepared),
            AuthCallbackOutcome::Answer(answer) => return answer,
        };
        if self.persist(&slot).await.is_err() {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        if self.audit_slot(&slot, "exchange", true).await.is_err() {
            return self
                .exchange_stopped(&slot, &prepared.reply, Command::ExchangeUncertain, false)
                .await;
        }
        self.exchange(&slot, &code, prepared).await
    }

    pub async fn revoke(&self, server: Uuid) -> RevokeAnswer {
        let slot = match self.existing(server).await {
            Recalled::Present(slot) => slot,
            Recalled::Absent => return absent_revoke(),
            Recalled::Unavailable => return held_revoke(),
        };
        self.revoke_slot(&slot, Command::Revoke).await
    }

    /// Fence a configured remote whose stored resource is not the URL it
    /// would be called on, and resume a revoke that was still unfinished.
    /// Called at startup, before the process accepts a session, so a URL
    /// written while this process was down is not called with the previous
    /// token.
    pub async fn revalidate(&self, remotes: &[(Uuid, String)]) -> Result<(), FenceRefusal> {
        for (id, url) in remotes {
            let slot = match self.existing(*id).await {
                Recalled::Present(slot) => slot,
                Recalled::Absent => continue,
                Recalled::Unavailable => return Err(FenceRefusal),
            };
            let (bound, resume) = {
                let guard = slot.lock().await;
                let resume = matches!(
                    guard.auth.phase,
                    Phase::Revoking { .. } | Phase::RevocationIncomplete { .. }
                );
                (guard.auth.resource.clone(), resume)
            };
            if bound != *url {
                self.fence(&[BindingChange::ResourceChanged {
                    id: *id,
                    previous_url: bound,
                    url: url.clone(),
                }])
                .await?;
                continue;
            }
            if resume {
                let answer = self.revoke(*id).await;
                if !answer.settled {
                    return Err(FenceRefusal);
                }
            }
        }
        Ok(())
    }

    pub async fn fence(&self, changes: &[BindingChange]) -> Result<(), FenceRefusal> {
        for change in changes {
            let (id, command) = match change {
                BindingChange::ResourceChanged { id, .. } => {
                    (*id, Command::ResourceChanged { url: String::new() })
                }
                BindingChange::Removed { id, .. } => (*id, Command::Removed),
            };
            let slot = match self.existing(id).await {
                Recalled::Present(slot) => slot,
                Recalled::Absent => continue,
                Recalled::Unavailable => return Err(FenceRefusal),
            };
            let answer = self.revoke_slot(&slot, command).await;
            if !answer.settled {
                return Err(FenceRefusal);
            }
        }
        Ok(())
    }

    pub async fn facts(&self, server: Uuid) -> Option<ListedAuthorization> {
        match self.existing(server).await {
            Recalled::Present(slot) => {
                let guard = slot.lock().await;
                Some(listed(&guard.auth, self.clock.now_ms()))
            }
            Recalled::Absent => None,
            Recalled::Unavailable => Some(ListedAuthorization {
                phase: "revocation_incomplete",
                generation: 0,
                token_expired: false,
                refresh_failing: false,
                scope_required: false,
                remote: None,
                domains_digest: None,
            }),
        }
    }

    #[allow(clippy::result_unit_err)]
    pub async fn acknowledge_domains(&self, server: Uuid, digest: &str) -> Result<(), ()> {
        let Recalled::Present(slot) = self.existing(server).await else {
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

    async fn exchange(
        &self,
        slot: &Arc<Mutex<Slot>>,
        code: &str,
        prepared: PendingExchange,
    ) -> AuthorizeAnswer {
        let reply = prepared.reply;
        let (endpoint, verifier, redirect, client_id) = (
            prepared.endpoint,
            prepared.verifier,
            prepared.redirect,
            prepared.client_id,
        );
        let (Some(endpoint), Some(verifier), Some(redirect), Some(client_id)) =
            (endpoint, verifier, redirect, client_id)
        else {
            return self
                .exchange_stopped(slot, &reply, Command::ExchangeUncertain, false)
                .await;
        };
        if !discovery::https_url(&endpoint) {
            return self
                .exchange_stopped(slot, &reply, Command::ExchangeRefused, true)
                .await;
        }
        let body = discovery::form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &redirect),
            ("client_id", &client_id),
            ("code_verifier", &verifier),
            ("resource", &reply.resource),
        ]);
        let response = match self.http.post_form(&endpoint, &body).await {
            Ok(response) => response,
            Err(OAuthCallFailure::NotSent) => {
                return self
                    .exchange_stopped(slot, &reply, Command::ExchangeRefused, true)
                    .await;
            }
            Err(OAuthCallFailure::Lost) => {
                return self
                    .exchange_stopped(slot, &reply, Command::ExchangeUncertain, false)
                    .await;
            }
        };
        self.publish_token(slot, &response.body, &reply, None).await
    }

    /// Apply `command` only while `reply` is still the live attempt. A
    /// replacement authorize keeps its own phase.
    async fn exchange_stopped(
        &self,
        slot: &Arc<Mutex<Slot>>,
        reply: &Reply,
        command: Command,
        refused: bool,
    ) -> AuthorizeAnswer {
        if self.reply_current(slot, reply).await {
            let _ = self.apply(slot, command).await;
            let _ = self.persist(slot).await;
        }
        if refused {
            AuthorizeAnswer::DiscoveryFailed
        } else {
            AuthorizeAnswer::AuthorizationIncomplete
        }
    }

    async fn publish_token(
        &self,
        slot: &Arc<Mutex<Slot>>,
        body: &str,
        reply: &Reply,
        previous_refresh: Option<String>,
    ) -> AuthorizeAnswer {
        if !self.reply_current(slot, reply).await {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let parsed = parse_token(body);
        if parsed.error.as_deref() == Some("invalid_grant") {
            if !self.reply_current(slot, reply).await {
                return AuthorizeAnswer::AuthorizationIncomplete;
            }
            if reply.refresh.is_some() {
                let answer = self.revoke_slot(slot, Command::InvalidGrant).await;
                return if answer.settled {
                    AuthorizeAnswer::DiscoveryFailed
                } else {
                    AuthorizeAnswer::AuthorizationIncomplete
                };
            }
            return self
                .exchange_stopped(slot, reply, Command::ExchangeRefused, true)
                .await;
        }
        let Some(access) = parsed.access else {
            if !self.reply_current(slot, reply).await {
                return AuthorizeAnswer::AuthorizationIncomplete;
            }
            let command = if reply.refresh.is_some() {
                Command::RefreshLost
            } else {
                Command::ExchangeUncertain
            };
            let _ = self.apply(slot, command).await;
            let _ = self.persist(slot).await;
            return AuthorizeAnswer::AuthorizationIncomplete;
        };
        if !self.reply_current(slot, reply).await {
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let generation = match reply.refresh {
            Some(generation) => generation + 1,
            None => {
                let guard = slot.lock().await;
                guard.auth.generation.max(1)
            }
        };
        let expires_at_ms = parsed.expires_in.map(|seconds| {
            self.clock
                .now_ms()
                .saturating_add(seconds.saturating_mul(1000))
        });
        let material = TokenMaterial {
            access_token: access.clone(),
            // RFC 6749 §6: a successful refresh need not replace its token.
            refresh_token: parsed.refresh.or(previous_refresh),
            generation,
        };
        let server = server_of(slot).await;
        let publication = match self.records.store_secret(server, &material).await {
            Ok(publication) => publication,
            Err(RecordFailure::Unavailable) => Publication::Unknown,
        };
        let wrote = publication == Publication::Acknowledged;
        if !self.reply_current(slot, reply).await {
            if wrote {
                self.discard_written(server, &access, generation).await;
            }
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
        let command = match reply.refresh {
            Some(from_generation) => Command::RefreshPublication {
                from_generation,
                publication,
                expires_at_ms,
                resource: reply.resource.clone(),
            },
            None => Command::SecretPublication {
                publication,
                generation,
                expires_at_ms,
                attempt: reply.attempt,
                resource: reply.resource.clone(),
            },
        };
        let decision = self.apply(slot, command).await;
        let stale = decision.refusal == Some(Refusal::Stale)
            || decision
                .effects
                .iter()
                .any(|effect| matches!(effect, Effect::DeleteCandidate));
        if stale {
            if wrote {
                self.discard_written(server, &access, generation).await;
            }
            return AuthorizeAnswer::AuthorizationIncomplete;
        }
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

    /// Remove the token this reply stored when that reply is no longer current.
    /// A different generation is a replacement and stays.
    async fn discard_written(&self, server: Uuid, access_token: &str, generation: u64) {
        let Ok(Some(current)) = self.records.load_secret(server).await else {
            return;
        };
        if current.access_token == access_token && current.generation == generation {
            let _ = self.records.delete_secret(server).await;
        }
    }

    async fn revoke_slot(&self, slot: &Arc<Mutex<Slot>>, command: Command) -> RevokeAnswer {
        let running = {
            let mut guard = slot.lock().await;
            let decision = guard.auth.step(command);
            guard.auth = decision.auth.clone();
            // Another call in this process is already driving cleanup. A
            // `Revoking` record loaded after that driver stopped is resumed.
            if decision.refusal == Some(Refusal::JoinRevoke)
                && guard.revoke_running.load(Ordering::SeqCst)
            {
                return revoke_answer(&guard.auth);
            }
            guard.revoke_running.store(true, Ordering::SeqCst);
            guard.revoke_running.clone()
        };
        let _release = ReleaseRevoke(running);
        let carried = match self.seal_revoke(slot).await {
            Ok(carried) => carried,
            Err(()) => return snapshot(slot).await,
        };
        let _ = self.audit_slot(slot, "revoke", true).await;
        let name = {
            let guard = slot.lock().await;
            guard.auth.name.clone()
        };
        self.sessions.drain(&name).await;
        self.apply(slot, Command::LocalDrained).await;
        let observation = self.revoke_remote(slot, carried).await;
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

    /// Write the revoking record, or delete the secret, before the remote
    /// call. Restart then refuses the token this revoke was already retiring.
    /// The deleted token is returned so that call can still name it.
    async fn seal_revoke(&self, slot: &Arc<Mutex<Slot>>) -> Result<Option<TokenMaterial>, ()> {
        if self.persist(slot).await.is_ok() {
            return Ok(None);
        }
        let server = server_of(slot).await;
        let secret = self.records.load_secret(server).await.ok().flatten();
        let deletion = match self.records.delete_secret(server).await {
            Ok(deletion) => deletion,
            Err(RecordFailure::Unavailable) => Deletion::Unknown,
        };
        if deletion != Deletion::Deleted {
            return Err(());
        }
        self.apply(slot, Command::SecretDeletion { deletion }).await;
        let _ = self.persist(slot).await;
        Ok(secret)
    }

    async fn reply_current(&self, slot: &Arc<Mutex<Slot>>, reply: &Reply) -> bool {
        reply.current(&slot.lock().await.auth)
    }

    async fn revoke_remote(
        &self,
        slot: &Arc<Mutex<Slot>>,
        carried: Option<TokenMaterial>,
    ) -> RemoteObservation {
        let (endpoint, server) = {
            let guard = slot.lock().await;
            (guard.revocation_endpoint.clone(), guard.auth.server)
        };
        let Some(endpoint) = endpoint.filter(|url| discovery::https_url(url)) else {
            return RemoteObservation::Unsupported;
        };
        let loaded = match carried {
            Some(secret) => Some(secret),
            None => self.records.load_secret(server).await.ok().flatten(),
        };
        let Some(secret) = loaded else {
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

    async fn refresh(
        self: &Arc<Self>,
        server: Uuid,
        rejected: bool,
    ) -> Result<Option<AdmittedToken>, AdmissionRefusal> {
        let Recalled::Present(slot) = self.existing(server).await else {
            return Err(AdmissionRefusal::Unauthorized);
        };
        let flight = {
            let mut guard = slot.lock().await;
            if let Admission::Bearer { generation } = self.live_admission(&guard, server) {
                if !rejected || generation > guard.handed {
                    guard.handed = generation;
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
                        .unwrap_or(Err(AdmissionRefusal::Unauthorized));
                }
                return Err(AdmissionRefusal::Unauthorized);
            }
            if decision.refusal.is_some() {
                guard.auth = decision.auth;
                return Err(AdmissionRefusal::Unauthorized);
            }
            guard.auth = decision.auth;
            let flight = Arc::new(RefreshFlight {
                notify: Notify::new(),
                result: Mutex::new(None),
            });
            self.flights.lock().await.insert(server, flight.clone());
            flight
        };
        let owner = Arc::clone(self);
        let driving = flight.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            owner.drive_refresh(server, slot, driving).await;
        });
        let notified = flight.notify.notified();
        if let Some(result) = flight.result.lock().await.clone() {
            return result;
        }
        notified.await;
        let result = flight
            .result
            .lock()
            .await
            .clone()
            .unwrap_or(Err(AdmissionRefusal::Unauthorized));
        result
    }

    /// Runs after the flight is published, on its own task. Dropping a waiter
    /// does not drop the token request or leave joiners waiting.
    async fn drive_refresh(
        self: &Arc<Self>,
        server: Uuid,
        slot: Arc<Mutex<Slot>>,
        flight: Arc<RefreshFlight>,
    ) {
        let (reply, endpoint, client_id) = {
            let guard = slot.lock().await;
            (
                Reply::refresh(guard.auth.generation, guard.auth.resource.clone()),
                guard.token_endpoint.clone(),
                guard.auth.client_id.clone(),
            )
        };
        if self.audit_slot(&slot, "refresh", true).await.is_err() {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched, &reply)
                .await;
            return;
        }
        let refresh_token = self
            .records
            .load_secret(server)
            .await
            .ok()
            .flatten()
            .and_then(|secret| secret.refresh_token);
        let Some(endpoint) = endpoint.filter(|url| discovery::https_url(url)) else {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched, &reply)
                .await;
            return;
        };
        let Some(refresh_token) = refresh_token else {
            self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched, &reply)
                .await;
            return;
        };
        {
            let mut guard = slot.lock().await;
            if !reply.current(&guard.auth) {
                drop(guard);
                self.settle_flight(server, &flight, Err(AdmissionRefusal::Unauthorized))
                    .await;
                return;
            }
            guard.auth.refresh_dispatched = true;
        }
        if self.persist(&slot).await.is_err() {
            self.finish_refresh(&slot, &flight, Command::RefreshLost, &reply)
                .await;
            return;
        }
        let body = discovery::form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", client_id.as_deref().unwrap_or("")),
            ("resource", &reply.resource),
        ]);
        let outcome = match self.http.post_form(&endpoint, &body).await {
            Err(OAuthCallFailure::NotSent) => {
                self.finish_refresh(&slot, &flight, Command::RefreshNotDispatched, &reply)
                    .await;
                return;
            }
            Err(OAuthCallFailure::Lost) => {
                self.finish_refresh(&slot, &flight, Command::RefreshLost, &reply)
                    .await;
                return;
            }
            Ok(response) => response,
        };
        let answer = self
            .publish_token(&slot, &outcome.body, &reply, Some(refresh_token))
            .await;
        let result = match answer {
            AuthorizeAnswer::Ready { generation } => {
                self.release_bearer(&slot, server, generation).await
            }
            _ => Err(AdmissionRefusal::Unauthorized),
        };
        self.settle_flight(server, &flight, result).await;
    }

    async fn finish_refresh(
        &self,
        slot: &Arc<Mutex<Slot>>,
        flight: &Arc<RefreshFlight>,
        command: Command,
        reply: &Reply,
    ) {
        if self.reply_current(slot, reply).await {
            self.apply(slot, command).await;
            let _ = self.persist(slot).await;
        }
        let server = server_of(slot).await;
        self.settle_flight(server, flight, Err(AdmissionRefusal::Unauthorized))
            .await;
    }

    async fn settle_flight(
        &self,
        server: Uuid,
        flight: &Arc<RefreshFlight>,
        result: Result<Option<AdmittedToken>, AdmissionRefusal>,
    ) {
        *flight.result.lock().await = Some(result);
        let mut flights = self.flights.lock().await;
        if flights
            .get(&server)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            flights.remove(&server);
        }
        drop(flights);
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
    ) -> Result<Option<AdmittedToken>, AdmissionRefusal> {
        let loaded = self.records.load_secret(server).await;
        let guard = slot.lock().await;
        match self.live_admission(&guard, server) {
            Admission::Bearer {
                generation: current,
            } if current == generation => match loaded {
                Ok(Some(secret)) if secret.generation == generation => Ok(Some(AdmittedToken {
                    access_token: secret.access_token,
                    generation: secret.generation,
                })),
                Ok(Some(_)) | Ok(None) => Err(AdmissionRefusal::Unauthorized),
                Err(RecordFailure::Unavailable) => Err(AdmissionRefusal::Unreachable),
            },
            Admission::Bearer { .. } => Err(AdmissionRefusal::Unauthorized),
            Admission::InsufficientScope => Err(AdmissionRefusal::InsufficientScope),
            Admission::NoneRequired => Ok(None),
            Admission::NeedsRefresh | Admission::Unauthorized => {
                Err(AdmissionRefusal::Unauthorized)
            }
        }
    }

    async fn accept_discovery(
        &self,
        slot: &Arc<Mutex<Slot>>,
        discovered: Discovered,
        scope: Option<String>,
    ) -> Option<Discovered> {
        // The challenge scope is what consent asks for. `scopes_supported`
        // is only the fallback when the challenge names none.
        let scopes = match scope {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => discovered.scopes.clone(),
        };
        let decision = self
            .apply(
                slot,
                Command::DiscoveryAccepted(AcceptedDiscovery {
                    issuer: discovered.issuer.clone(),
                    scopes: scopes.clone(),
                    pkce_s256: discovered.pkce_s256,
                    registration_offered: discovered.registration_endpoint.is_some(),
                    revocation_offered: discovered.revocation_endpoint.is_some(),
                    resource_matches: discovered.resource_matches,
                    issuer_matches: discovered.issuer_matches,
                }),
            )
            .await;
        decision.refusal.is_none().then(|| {
            let mut accepted = discovered;
            accepted.scopes = scopes;
            accepted
        })
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
        match self.http.post_json(endpoint, &body).await {
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

    async fn slot(
        &self,
        server: Uuid,
        name: &str,
        resource: &str,
    ) -> Result<Arc<Mutex<Slot>>, RecordFailure> {
        let slots = self.slots.lock().await;
        if let Some(slot) = slots.get(&server) {
            return Ok(slot.clone());
        }
        drop(slots);
        let auth = match self.records.load(server).await {
            Err(failure) => return Err(failure),
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
            Ok(None) => ServerAuth::consent_needed(server, name, resource),
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
            revoke_running: Arc::new(AtomicBool::new(false)),
        }));
        let mut slots = self.slots.lock().await;
        if let Some(existing) = slots.get(&server) {
            return Ok(existing.clone());
        }
        slots.insert(server, slot.clone());
        Ok(slot)
    }

    async fn existing(&self, server: Uuid) -> Recalled {
        if let Some(slot) = self.slots.lock().await.get(&server).cloned() {
            return Recalled::Present(slot);
        }
        let loaded = match self.records.load(server).await {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return Recalled::Absent,
            Err(RecordFailure::Unavailable) => return Recalled::Unavailable,
        };
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
            revoke_running: Arc::new(AtomicBool::new(false)),
        }));
        let mut slots = self.slots.lock().await;
        if let Some(existing) = slots.get(&server) {
            return Recalled::Present(existing.clone());
        }
        slots.insert(server, slot.clone());
        Recalled::Present(slot)
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

fn absent_revoke() -> RevokeAnswer {
    RevokeAnswer {
        settled: true,
        local_drained: true,
        secret_deleted: true,
        remote: Some(RemoteObservation::Unsupported),
        evidence_acknowledged: true,
    }
}

/// The record could not be read, so the sealed secret and the remote
/// revocation are both unobserved. This is not a settlement.
fn held_revoke() -> RevokeAnswer {
    RevokeAnswer {
        settled: false,
        local_drained: false,
        secret_deleted: false,
        remote: None,
        evidence_acknowledged: false,
    }
}

struct ReleaseRevoke(Arc<AtomicBool>);

impl Drop for ReleaseRevoke {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn snapshot(slot: &Arc<Mutex<Slot>>) -> RevokeAnswer {
    revoke_answer(&slot.lock().await.auth)
}

fn revoke_answer(auth: &ServerAuth) -> RevokeAnswer {
    let (settled, local_drained, secret_deleted, remote, evidence_acknowledged) = match &auth.phase
    {
        Phase::ConsentNeeded => (true, true, true, guard_remote(auth), true),
        Phase::RevocationIncomplete { settlement, .. } | Phase::Revoking { settlement, .. } => (
            false,
            settlement.local_drained,
            settlement.secret_deleted,
            settlement.remote,
            settlement.evidence_acked,
        ),
        _ => (false, false, !auth.secret_present, None, false),
    };
    let _ = secret_deleted;
    RevokeAnswer {
        settled,
        local_drained,
        secret_deleted: !auth.secret_present,
        remote: remote.or(guard_remote(auth)),
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

impl AuthorizationOwner {
    /// The token to send for `server`, or none when the server needs none.
    pub async fn bearer(
        self: &Arc<Self>,
        server: Uuid,
    ) -> Result<Option<AdmittedToken>, AdmissionRefusal> {
        let slot = match self.existing(server).await {
            Recalled::Present(slot) => slot,
            Recalled::Absent => return Ok(None),
            Recalled::Unavailable => return Err(AdmissionRefusal::Unauthorized),
        };
        let admission = {
            let guard = slot.lock().await;
            self.live_admission(&guard, server)
        };
        match admission {
            Admission::NoneRequired => Ok(None),
            Admission::Bearer { generation } => {
                let mut guard = slot.lock().await;
                if generation >= guard.handed {
                    guard.handed = generation;
                }
                drop(guard);
                self.release_bearer(&slot, server, generation).await
            }
            Admission::InsufficientScope => Err(AdmissionRefusal::InsufficientScope),
            Admission::NeedsRefresh => self.refresh(server, false).await,
            Admission::Unauthorized => Err(AdmissionRefusal::Unauthorized),
        }
    }

    /// `server` answered 401 with `www_authenticate`. A token retries the
    /// refused request once.
    pub async fn rejected(
        self: &Arc<Self>,
        server: Uuid,
        _www_authenticate: &str,
    ) -> Result<Option<AdmittedToken>, AdmissionRefusal> {
        self.refresh(server, true).await
    }

    /// `server` answered 403 `insufficient_scope`. The call is not retried.
    pub async fn insufficient_scope(&self, server: Uuid, _www_authenticate: &str) {
        if let Recalled::Present(slot) = self.existing(server).await {
            self.apply(&slot, Command::InsufficientScope).await;
            let _ = self.persist(&slot).await;
            let _ = self.audit_slot(&slot, "scope", false).await;
        }
    }
}
