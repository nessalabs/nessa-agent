//! Row tests for remote MCP authorization.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::mcp_authorization::application::{
    AdmissionRefusal, AuthorizationOwner, AuthorizationRecords, AuthorizeAnswer, CallbackQuery,
    OAuthCallFailure, OAuthResponse, RecordFailure, TokenMaterial,
};
use crate::mcp_authorization::domain::{
    AcceptedDiscovery, Admission, Command, Deletion, Phase, Publication, RefreshActivity, Refusal,
    RemoteObservation, ServerAuth, TokenAvailability,
};
use crate::mcp_authorization::infrastructure::{MemoryAuthorization, ScriptedCallback};

fn server() -> Uuid {
    Uuid::from_u128(7)
}

fn auth() -> ServerAuth {
    ServerAuth::consent_needed(server(), "docs", "https://mcp.example/mcp")
}

#[test]
fn a1_a_missing_writer_refuses_before_discovery() {
    let decided = auth().step(Command::Authorize {
        store_available: false,
    });
    assert_eq!(decided.refusal, Some(Refusal::StoreUnavailable));
    assert!(matches!(decided.auth.phase, Phase::ConsentNeeded));
}

#[test]
fn a2_binding_or_missing_pkce_returns_to_consent() {
    let discovering = auth()
        .step(Command::Authorize {
            store_available: true,
        })
        .auth;
    let rejected = discovering.step(Command::DiscoveryAccepted(AcceptedDiscovery {
        issuer: "https://as.example".into(),
        scopes: vec!["mcp".into()],
        pkce_s256: false,
        registration_offered: true,
        revocation_offered: false,
        resource_matches: true,
        issuer_matches: true,
    }));
    assert_eq!(rejected.refusal, Some(Refusal::BindingRejected));
    assert!(matches!(rejected.auth.phase, Phase::ConsentNeeded));
}

#[test]
fn a3_a_callback_is_consumed_once() {
    let pending = pending_consent();
    let state = pending.attempt.as_ref().unwrap().state.clone();
    let exchanged = pending
        .step(Command::Callback {
            state: state.clone(),
            now_ms: 1,
            denied: false,
        })
        .auth;
    assert!(matches!(exchanged.phase, Phase::Exchanging { .. }));
    let replay = exchanged.step(Command::Callback {
        state,
        now_ms: 1,
        denied: false,
    });
    assert_eq!(replay.refusal, Some(Refusal::StaleCallback));
}

#[test]
fn a4_a_lost_exchange_fences_use() {
    let exchanging = pending_consent()
        .step(Command::Callback {
            state: "state".into(),
            now_ms: 1,
            denied: false,
        })
        .auth;
    let lost = exchanging.step(Command::ExchangeUncertain).auth;
    assert!(matches!(lost.phase, Phase::AuthorizationIncomplete { .. }));
    assert!(lost.dispatch_fenced);
    let again = lost.step(Command::Authorize {
        store_available: true,
    });
    assert_eq!(again.refusal, Some(Refusal::ReconciliationRequired));
}

#[test]
fn a6_a_refresh_that_was_not_sent_keeps_a_usable_token() {
    let mut ready = ready_token();
    ready.expires_at_ms = Some(10_000);
    let refreshing = ready
        .step(Command::RefreshRequested {
            now_ms: 9_000,
            rejected: true,
        })
        .auth;
    assert!(matches!(
        refreshing.phase,
        Phase::Ready {
            availability: TokenAvailability::Unavailable,
            refresh: RefreshActivity::Refreshing,
            ..
        }
    ));
    let kept = refreshing.step(Command::RefreshNotDispatched).auth;
    assert!(matches!(
        kept.phase,
        Phase::Ready {
            availability: TokenAvailability::Unavailable,
            refresh: RefreshActivity::Idle,
            ..
        }
    ));
    assert!(!kept.dispatch_fenced);
}

#[test]
fn a6_a_lost_refresh_does_not_replay() {
    let refreshing = ready_token()
        .step(Command::RefreshRequested {
            now_ms: 50_000,
            rejected: false,
        })
        .auth;
    let lost = refreshing.step(Command::RefreshLost).auth;
    assert!(matches!(lost.phase, Phase::AuthorizationIncomplete { .. }));
    let replay = lost.step(Command::RefreshRequested {
        now_ms: 50_000,
        rejected: false,
    });
    assert_eq!(replay.refusal, Some(Refusal::Unauthorized));
}

#[test]
fn a7_a_late_refresh_does_not_publish_over_revoke() {
    let refreshing = ready_token()
        .step(Command::RefreshRequested {
            now_ms: 1,
            rejected: true,
        })
        .auth;
    let revoking = refreshing.step(Command::Revoke).auth;
    let late = revoking.step(Command::RefreshPublication {
        from_generation: 1,
        publication: Publication::Acknowledged,
        expires_at_ms: None,
    });
    assert_eq!(late.refusal, Some(Refusal::Stale));
    assert!(matches!(late.auth.phase, Phase::Revoking { .. }));
    assert_eq!(late.auth.generation, 1);
}

#[test]
fn a8_failed_deletion_stays_fenced() {
    let revoking = ready_token().step(Command::Revoke).auth;
    let drained = revoking.step(Command::LocalDrained).auth;
    let observed = drained
        .step(Command::RemoteObserved(RemoteObservation::Unconfirmed))
        .auth;
    let failed = observed
        .step(Command::SecretDeletion {
            deletion: Deletion::Failed,
        })
        .auth;
    assert!(matches!(failed.phase, Phase::RevocationIncomplete { .. }));
    assert!(failed.dispatch_fenced);
}

#[test]
fn a9_deletion_without_audit_is_incomplete() {
    let mut revoking = ready_token().step(Command::Revoke).auth;
    revoking = revoking.step(Command::LocalDrained).auth;
    revoking = revoking
        .step(Command::RemoteObserved(RemoteObservation::Acknowledged))
        .auth;
    revoking = revoking
        .step(Command::SecretDeletion {
            deletion: Deletion::Deleted,
        })
        .auth;
    let unaudited = revoking.step(Command::Evidence { acked: false }).auth;
    assert!(matches!(
        unaudited.phase,
        Phase::RevocationIncomplete { .. }
    ));
}

#[test]
fn a10_restart_does_not_treat_a_fenced_secret_as_usable() {
    let mut ready = ready_token();
    ready.dispatch_fenced = true;
    let restarted = ready
        .step(Command::Restart {
            now_ms: 1,
            secret_present: true,
            resource_now: Some(ready.resource.clone()),
        })
        .auth;
    assert!(matches!(restarted.phase, Phase::ConsentNeeded));
    assert_eq!(
        restarted.admission(1, &ready.resource),
        Admission::Unauthorized
    );
}

fn pending_consent() -> ServerAuth {
    let discovering = auth()
        .step(Command::Authorize {
            store_available: true,
        })
        .auth;
    discovering
        .step(Command::ConsentReady {
            state: "state".into(),
            deadline_ms: 1_000,
        })
        .auth
}

fn ready_token() -> ServerAuth {
    let mut ready = auth();
    ready.phase = Phase::Ready {
        availability: TokenAvailability::Usable,
        refresh: RefreshActivity::Idle,
        scope_required: false,
    };
    ready.generation = 1;
    ready.secret_present = true;
    ready.expires_at_ms = Some(100_000);
    ready
}

#[tokio::test]
async fn a_store_without_a_writer_refuses_authorize() {
    let memory = Arc::new(MemoryAuthorization::without_writer());
    let owner = Arc::new(owner(memory));
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    assert_eq!(answer, AuthorizeAnswer::StoreUnavailable);
}

#[tokio::test]
async fn authorize_returns_a_consent_url_and_rejects_a_replayed_state() {
    let memory = Arc::new(MemoryAuthorization::new());
    memory
        .push_route(
            "https://mcp.example/mcp",
            Ok(OAuthResponse {
                status: 401,
                body: String::new(),
                www_authenticate: Some(
                    "Bearer resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\""
                        .into(),
                ),
            }),
        )
        .await;
    memory
        .push_route(
            "https://mcp.example/.well-known/oauth-protected-resource",
            Ok(metadata()),
        )
        .await;
    memory
        .push_route(
            "https://as.example/.well-known/oauth-authorization-server",
            Ok(server_metadata()),
        )
        .await;
    memory
        .push_route(
            "https://as.example/register",
            Ok(OAuthResponse {
                status: 201,
                body: r#"{"client_id":"client"}"#.into(),
                www_authenticate: None,
            }),
        )
        .await;
    let owner = Arc::new(owner(memory.clone()));
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    let AuthorizeAnswer::PendingConsent { consent_url, .. } = answer else {
        panic!("expected consent, got {answer:?}");
    };
    assert!(consent_url.contains("code_challenge_method=S256"));
    assert!(!consent_url.contains("code_verifier"));
    let state = consent_url
        .split("state=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();
    memory
        .push_route(
            "https://as.example/token",
            Ok(OAuthResponse {
                status: 200,
                body: r#"{"access_token":"access","refresh_token":"refresh","expires_in":60}"#
                    .into(),
                www_authenticate: None,
            }),
        )
        .await;
    let ready = owner
        .complete_callback(
            server(),
            CallbackQuery {
                state: state.clone(),
                code: Some("code".into()),
                denied: false,
            },
        )
        .await;
    assert!(matches!(ready, AuthorizeAnswer::Ready { generation: 1 }));
    let replay = owner
        .complete_callback(
            server(),
            CallbackQuery {
                state,
                code: Some("code".into()),
                denied: false,
            },
        )
        .await;
    assert!(!matches!(replay, AuthorizeAnswer::Ready { .. }));
    let _ = OAuthCallFailure::NotSent;
}

fn owner(memory: Arc<MemoryAuthorization>) -> AuthorizationOwner {
    AuthorizationOwner::new(
        memory.clone(),
        memory.clone(),
        memory.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.writer(),
    )
}

fn metadata() -> OAuthResponse {
    OAuthResponse {
        status: 200,
        body: r#"{"resource":"https://mcp.example/mcp","authorization_servers":["https://as.example"],"scopes_supported":["mcp"]}"#.into(),
        www_authenticate: None,
    }
}

fn server_metadata() -> OAuthResponse {
    OAuthResponse {
        status: 200,
        body: r#"{"issuer":"https://as.example","authorization_endpoint":"https://as.example/authorize","token_endpoint":"https://as.example/token","registration_endpoint":"https://as.example/register","revocation_endpoint":"https://as.example/revoke","code_challenge_methods_supported":["S256"]}"#.into(),
        www_authenticate: None,
    }
}

async fn push_discovery(memory: &MemoryAuthorization) {
    memory
        .push_route(
            "https://mcp.example/mcp",
            Ok(OAuthResponse {
                status: 401,
                body: String::new(),
                www_authenticate: Some(
                    "Bearer resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\""
                        .into(),
                ),
            }),
        )
        .await;
    memory
        .push_route(
            "https://mcp.example/.well-known/oauth-protected-resource",
            Ok(metadata()),
        )
        .await;
    memory
        .push_route(
            "https://as.example/.well-known/oauth-authorization-server",
            Ok(server_metadata()),
        )
        .await;
    memory
        .push_route(
            "https://as.example/register",
            Ok(OAuthResponse {
                status: 201,
                body: r#"{"client_id":"client"}"#.into(),
                www_authenticate: None,
            }),
        )
        .await;
}

fn token_body(access: &str) -> OAuthResponse {
    OAuthResponse {
        status: 200,
        body: format!(r#"{{"access_token":"{access}","refresh_token":"refresh","expires_in":60}}"#),
        www_authenticate: None,
    }
}

#[tokio::test]
async fn dynamic_registration_is_posted_as_json() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    let owner = Arc::new(owner(memory.clone()));
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    assert!(matches!(answer, AuthorizeAnswer::PendingConsent { .. }));
    let register = memory
        .posts()
        .await
        .into_iter()
        .find(|(url, _, _)| url == "https://as.example/register")
        .expect("registration post");
    assert!(register.1);
    assert!(register.2.contains("\"redirect_uris\""));
    assert!(!register.2.contains("redirect_uris="));
}

#[tokio::test]
async fn a_probe_other_than_401_needs_no_token() {
    let memory = Arc::new(MemoryAuthorization::new());
    memory
        .push_route(
            "https://mcp.example/mcp",
            Ok(OAuthResponse {
                status: 405,
                body: String::new(),
                www_authenticate: None,
            }),
        )
        .await;
    let owner = Arc::new(owner(memory.clone()));
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    assert_eq!(answer, AuthorizeAnswer::NotRequired);
    memory
        .set_resource(server(), "https://mcp.example/mcp")
        .await;
    assert_eq!(owner.bearer(server()).await, Ok(None));
}

#[tokio::test]
async fn a_rejected_bearer_refreshes_the_handed_generation() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    let owner = Arc::new(owner(memory.clone()));
    let answer = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await;
    let AuthorizeAnswer::PendingConsent { consent_url, .. } = answer else {
        panic!("expected consent, got {answer:?}");
    };
    let state = consent_url
        .split("state=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();
    memory
        .push_route("https://as.example/token", Ok(token_body("access")))
        .await;
    let ready = owner
        .complete_callback(
            server(),
            CallbackQuery {
                state,
                code: Some("code".into()),
                denied: false,
            },
        )
        .await;
    assert!(matches!(ready, AuthorizeAnswer::Ready { generation: 1 }));
    let token_post = memory
        .posts()
        .await
        .into_iter()
        .find(|(url, _, _)| url == "https://as.example/token")
        .expect("token post");
    assert!(!token_post.1);
    memory
        .set_resource(server(), "https://mcp.example/mcp")
        .await;
    let handed = owner.bearer(server()).await.unwrap().unwrap();
    assert_eq!(handed.generation, 1);
    memory
        .push_route("https://as.example/token", Ok(token_body("replacement")))
        .await;
    let refreshed = owner.rejected(server(), "Bearer").await.unwrap().unwrap();
    assert_eq!(refreshed.generation, 2);
    assert_eq!(refreshed.access_token, "replacement");
}

/// Holds `load` until two callers have entered it, then holds the later
/// `load_secret` calls until the test releases them. The first two secret
/// loads are the presence checks inside `existing`.
struct GatedRecords {
    inner: Arc<MemoryAuthorization>,
    loads: AtomicUsize,
    load_arrived: AtomicUsize,
    load_released: AtomicBool,
    secret_calls: AtomicUsize,
    presence_arrived: AtomicUsize,
    presence_released: AtomicBool,
    release_arrived: AtomicUsize,
    secret_released: AtomicBool,
}

impl GatedRecords {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            loads: AtomicUsize::new(0),
            load_arrived: AtomicUsize::new(0),
            load_released: AtomicBool::new(false),
            secret_calls: AtomicUsize::new(0),
            presence_arrived: AtomicUsize::new(0),
            presence_released: AtomicBool::new(false),
            release_arrived: AtomicUsize::new(0),
            secret_released: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl AuthorizationRecords for GatedRecords {
    async fn load(
        &self,
        server: Uuid,
    ) -> Result<Option<crate::mcp_authorization::domain::ServerAuth>, RecordFailure> {
        let ticket = self.loads.fetch_add(1, Ordering::SeqCst);
        if ticket < 2 {
            self.load_arrived.fetch_add(1, Ordering::SeqCst);
            while !self.load_released.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        self.inner.load(server).await
    }

    async fn store(
        &self,
        auth: &crate::mcp_authorization::domain::ServerAuth,
    ) -> Result<(), RecordFailure> {
        self.inner.store(auth).await
    }

    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        let ticket = self.secret_calls.fetch_add(1, Ordering::SeqCst);
        if ticket < 2 {
            self.presence_arrived.fetch_add(1, Ordering::SeqCst);
            while !self.presence_released.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        } else {
            self.release_arrived.fetch_add(1, Ordering::SeqCst);
            while !self.secret_released.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        self.inner.load_secret(server).await
    }

    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
        self.inner.store_secret(server, secret).await
    }

    async fn delete_secret(
        &self,
        server: Uuid,
    ) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
        self.inner.delete_secret(server).await
    }
}

#[tokio::test]
async fn two_callers_share_one_slot_so_revoke_refuses_both_bearers() {
    let memory = Arc::new(MemoryAuthorization::new());
    let records = Arc::new(GatedRecords::new(memory.clone()));
    records.store(&ready_token()).await.unwrap();
    records
        .store_secret(
            server(),
            &TokenMaterial {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                generation: 1,
            },
        )
        .await
        .unwrap();
    memory
        .set_resource(server(), "https://mcp.example/mcp")
        .await;
    let owner = Arc::new(AuthorizationOwner::new(
        records.clone(),
        memory.clone(),
        memory.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ));
    let first = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.bearer(server()).await })
    };
    let second = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.bearer(server()).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while records.load_arrived.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both loads");
    records.load_released.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        while records.presence_arrived.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both presence checks");
    records.presence_released.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        while records.release_arrived.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both bearer loads");
    let revoking = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.revoke(server()).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if memory
                .audits()
                .await
                .iter()
                .any(|record| record.action == "revoke")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("revoke audited");
    records.secret_released.store(true, Ordering::SeqCst);
    assert_eq!(first.await.unwrap(), Err(AdmissionRefusal::Unauthorized));
    assert_eq!(second.await.unwrap(), Err(AdmissionRefusal::Unauthorized));
    revoking.await.unwrap();
}
