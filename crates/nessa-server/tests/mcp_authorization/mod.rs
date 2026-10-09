//! Row tests for remote MCP authorization.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use crate::mcp_authorization::application::{
    AdmissionRefusal, AuthorizationOwner, AuthorizationRecords, AuthorizeAnswer, BindingChange,
    CallbackQuery, FenceRefusal, OAuthCallFailure, OAuthHttp, OAuthResponse, RecordFailure,
    TokenMaterial,
};
use crate::mcp_authorization::domain::{
    AcceptedDiscovery, Admission, Command, Deletion, Phase, Publication, RefreshActivity, Refusal,
    RemoteObservation, RevokeCause, ServerAuth, Settlement, TokenAvailability,
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
            resource: pending.resource.clone(),
        })
        .auth;
    assert!(matches!(exchanged.phase, Phase::Exchanging { .. }));
    let replay = exchanged.step(Command::Callback {
        state,
        now_ms: 1,
        denied: false,
        resource: exchanged.resource.clone(),
    });
    assert_eq!(replay.refusal, Some(Refusal::StaleCallback));
}

#[test]
fn a_callback_for_a_replaced_resource_is_not_consumed() {
    let pending = pending_consent();
    let state = pending.attempt.as_ref().unwrap().state.clone();
    let rejected = pending.step(Command::Callback {
        state,
        now_ms: 1,
        denied: false,
        resource: "https://other.example/mcp".into(),
    });
    assert_eq!(rejected.refusal, Some(Refusal::StaleCallback));
    assert!(matches!(rejected.auth.phase, Phase::PendingConsent { .. }));
    assert!(!rejected.auth.attempt.unwrap().consumed);
}

#[test]
fn a4_a_lost_exchange_fences_use() {
    let exchanging = pending_consent()
        .step(Command::Callback {
            state: "state".into(),
            now_ms: 1,
            denied: false,
            resource: "https://mcp.example/mcp".into(),
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
        resource: revoking.resource.clone(),
    });
    assert_eq!(late.refusal, Some(Refusal::Stale));
    assert!(matches!(late.auth.phase, Phase::Revoking { .. }));
    assert_eq!(late.auth.generation, 1);
}

#[test]
fn a_token_reply_for_another_attempt_does_not_publish() {
    let mut exchanging = pending_consent();
    exchanging.phase = Phase::Exchanging { attempt: 2 };
    exchanging.resource = "https://other.example/mcp".into();
    let late = exchanging.step(Command::SecretPublication {
        publication: Publication::Acknowledged,
        generation: 1,
        expires_at_ms: None,
        attempt: 1,
        resource: "https://mcp.example/mcp".into(),
    });
    assert_eq!(late.refusal, Some(Refusal::Stale));
    assert!(late.effects.is_empty());
    assert!(matches!(late.auth.phase, Phase::Exchanging { attempt: 2 }));
    assert_eq!(late.auth.resource, "https://other.example/mcp");
    assert!(!late.auth.secret_present);
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

async fn assert_refresh_token_survives_two_expiries(
    replacement_refresh: Option<&str>,
    expected_refresh: &str,
    rejected: bool,
) {
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
        .push_route("https://as.example/token", Ok(token_body("initial")))
        .await;
    assert!(matches!(
        owner
            .complete_callback(
                server(),
                CallbackQuery {
                    state,
                    code: Some("code".into()),
                    denied: false,
                },
            )
            .await,
        AuthorizeAnswer::Ready { generation: 1 }
    ));
    memory
        .set_resource(server(), "https://mcp.example/mcp")
        .await;
    let initial = owner.bearer(server()).await.unwrap().unwrap();
    assert_eq!(initial.generation, 1);

    let mut response = serde_json::json!({
        "access_token": "replacement",
        "token_type": "Bearer",
        "expires_in": 60,
    });
    if let Some(refresh) = replacement_refresh {
        response["refresh_token"] = serde_json::json!(refresh);
    }
    memory
        .push_route(
            "https://as.example/token",
            Ok(OAuthResponse {
                status: 200,
                body: response.to_string(),
                www_authenticate: None,
            }),
        )
        .await;
    // The injected clock starts at 1,000 ms. Each token lives for 60 seconds.
    let first = if rejected {
        owner.rejected(server(), "Bearer").await
    } else {
        memory.advance_to(61_000).await;
        owner.bearer(server()).await
    }
    .unwrap()
    .unwrap();
    assert_eq!(first.generation, 2);
    assert_eq!(first.access_token, "replacement");
    let stored = memory.load_secret(server()).await.unwrap().unwrap();

    memory
        .push_route(
            "https://as.example/token",
            Ok(OAuthResponse {
                status: 200,
                body: serde_json::json!({
                    "access_token": "after-second-expiry",
                    "token_type": "Bearer",
                    "refresh_token": expected_refresh,
                    "expires_in": 60,
                })
                .to_string(),
                www_authenticate: None,
            }),
        )
        .await;
    // A new owner loads the acknowledged credential and authorization record.
    let restored = Arc::new(self::owner(memory.clone()));
    memory.advance_to(121_000).await;
    let second = restored.bearer(server()).await;
    let posts = memory.posts().await;
    let refresh_posts: Vec<_> = posts
        .iter()
        .filter(|(url, _, body)| {
            url == "https://as.example/token" && body.contains("grant_type=refresh_token")
        })
        .collect();
    assert_eq!(stored.generation, 2);
    assert_eq!(stored.refresh_token.as_deref(), Some(expected_refresh));
    let second = second.unwrap().unwrap();
    assert_eq!(second.generation, 3);
    assert_eq!(second.access_token, "after-second-expiry");
    assert_eq!(refresh_posts.len(), 2);
    assert!(!refresh_posts[0].1);
    assert!(refresh_posts[0].2.contains("refresh_token=refresh&"));
    assert!(!refresh_posts[1].1);
    assert!(refresh_posts[1]
        .2
        .contains(&format!("refresh_token={expected_refresh}&")));
}

#[tokio::test]
async fn a_refresh_without_a_new_refresh_token_preserves_the_old_one() {
    assert_refresh_token_survives_two_expiries(None, "refresh", false).await;
}

#[tokio::test]
async fn a_refresh_with_a_new_refresh_token_uses_the_rotated_one() {
    assert_refresh_token_survives_two_expiries(Some("rotated"), "rotated", false).await;
}

#[tokio::test]
async fn a_rejected_bearer_refresh_without_a_new_refresh_token_preserves_the_old_one() {
    assert_refresh_token_survives_two_expiries(None, "refresh", true).await;
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

/// Blocks `store_secret` while `hold` is set, after counting the waiter.
struct HoldStore {
    inner: Arc<MemoryAuthorization>,
    hold: AtomicBool,
    waiting: AtomicUsize,
}

impl HoldStore {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            hold: AtomicBool::new(false),
            waiting: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AuthorizationRecords for HoldStore {
    async fn load(
        &self,
        server: Uuid,
    ) -> Result<Option<crate::mcp_authorization::domain::ServerAuth>, RecordFailure> {
        self.inner.load(server).await
    }

    async fn store(
        &self,
        auth: &crate::mcp_authorization::domain::ServerAuth,
    ) -> Result<(), RecordFailure> {
        self.inner.store(auth).await
    }

    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        self.inner.load_secret(server).await
    }

    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
        if self.hold.load(Ordering::SeqCst) {
            self.waiting.fetch_add(1, Ordering::SeqCst);
            while self.hold.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        self.inner.store_secret(server, secret).await
    }

    async fn delete_secret(
        &self,
        server: Uuid,
    ) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
        self.inner.delete_secret(server).await
    }
}

/// Blocks a token-endpoint form post until released.
struct GatedHttp {
    inner: Arc<MemoryAuthorization>,
    entered: AtomicUsize,
    release: AtomicBool,
}

impl GatedHttp {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            entered: AtomicUsize::new(0),
            release: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl OAuthHttp for GatedHttp {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.get(url).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        if url.ends_with("/token") {
            self.entered.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        self.inner.post_form(url, body).await
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.post_json(url, body).await
    }
}

fn ready_with_token_endpoint() -> crate::mcp_authorization::domain::ServerAuth {
    let mut auth = ready_token();
    auth.token_endpoint = Some("https://as.example/token".into());
    auth.client_id = Some("client".into());
    auth
}

async fn seed_ready(records: &impl AuthorizationRecords, memory: &MemoryAuthorization) {
    records.store(&ready_with_token_endpoint()).await.unwrap();
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
}

#[tokio::test]
async fn a_token_published_after_revoke_is_deleted() {
    let memory = Arc::new(MemoryAuthorization::new());
    let records = Arc::new(HoldStore::new(memory.clone()));
    seed_ready(records.as_ref(), &memory).await;
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
    assert_eq!(owner.bearer(server()).await.unwrap().unwrap().generation, 1);
    records.hold.store(true, Ordering::SeqCst);
    memory
        .push_route("https://as.example/token", Ok(token_body("late")))
        .await;
    let refreshing = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.rejected(server(), "Bearer").await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while records.waiting.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("refresh is storing");
    owner.revoke(server()).await;
    records.hold.store(false, Ordering::SeqCst);
    let _ = refreshing.await.unwrap();
    assert!(memory.load_secret(server()).await.unwrap().is_none());
}

#[tokio::test]
async fn dropping_a_refresh_waiter_does_not_cancel_the_flight() {
    let memory = Arc::new(MemoryAuthorization::new());
    let http = Arc::new(GatedHttp::new(memory.clone()));
    seed_ready(memory.as_ref(), &memory).await;
    let owner = Arc::new(AuthorizationOwner::new(
        memory.clone(),
        memory.clone(),
        http.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ));
    assert_eq!(owner.bearer(server()).await.unwrap().unwrap().generation, 1);
    memory
        .push_route("https://as.example/token", Ok(token_body("kept")))
        .await;
    let waiter = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.rejected(server(), "Bearer").await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while http.entered.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("token post started");
    waiter.abort();
    http.release.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if memory
                .load_secret(server())
                .await
                .ok()
                .flatten()
                .is_some_and(|secret| secret.generation == 2)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the refresh flight finished after its waiter was dropped");
}

#[tokio::test]
async fn an_absent_authorization_revoke_is_settled() {
    let owner = owner(Arc::new(MemoryAuthorization::new()));
    let answer = owner.revoke(server()).await;
    assert!(answer.settled);
    assert!(answer.secret_deleted);
    assert!(answer.evidence_acknowledged);
    assert_eq!(answer.remote, Some(RemoteObservation::Unsupported));
    assert!(owner.facts(server()).await.is_none());
    assert!(owner.fence(&[]).await.is_ok());
    assert!(owner
        .revalidate(&[(server(), "https://mcp.example/mcp".into())])
        .await
        .is_ok());
}

#[tokio::test]
async fn an_unreadable_authorization_is_held() {
    let memory = Arc::new(MemoryAuthorization::new());
    let records = Arc::new(UnreadableRecords::default());
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
    assert_eq!(
        owner
            .authorize(server(), "docs", "https://mcp.example/mcp")
            .await,
        AuthorizeAnswer::AuthorizationIncomplete
    );
    assert!(!records.stored.load(Ordering::SeqCst));
    let revoked = owner.revoke(server()).await;
    assert!(!revoked.settled);
    assert!(!revoked.local_drained);
    assert!(!revoked.secret_deleted);
    assert!(revoked.remote.is_none());
    assert!(!revoked.evidence_acknowledged);
    assert!(!records.deleted.load(Ordering::SeqCst));
    assert_eq!(
        owner.facts(server()).await.map(|facts| facts.phase),
        Some("revocation_incomplete")
    );
    let change = BindingChange::Removed {
        id: server(),
        url: "https://mcp.example/mcp".into(),
    };
    assert_eq!(owner.fence(&[change]).await, Err(FenceRefusal));
    assert_eq!(
        owner
            .revalidate(&[(server(), "https://mcp.example/other".into())])
            .await,
        Err(FenceRefusal)
    );
    assert_eq!(
        owner.bearer(server()).await,
        Err(AdmissionRefusal::Unauthorized)
    );
}

#[derive(Default)]
struct UnreadableRecords {
    stored: AtomicBool,
    deleted: AtomicBool,
}

#[async_trait]
impl AuthorizationRecords for UnreadableRecords {
    async fn load(&self, _server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        Err(RecordFailure::Unavailable)
    }

    async fn store(&self, _auth: &ServerAuth) -> Result<(), RecordFailure> {
        self.stored.store(true, Ordering::SeqCst);
        Err(RecordFailure::Unavailable)
    }

    async fn load_secret(&self, _server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        Err(RecordFailure::Unavailable)
    }

    async fn store_secret(
        &self,
        _server: Uuid,
        _secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure> {
        Err(RecordFailure::Unavailable)
    }

    async fn delete_secret(&self, _server: Uuid) -> Result<Deletion, RecordFailure> {
        self.deleted.store(true, Ordering::SeqCst);
        Err(RecordFailure::Unavailable)
    }
}

#[tokio::test]
async fn a_refused_revalidation_fence_is_not_discarded() {
    let memory = Arc::new(MemoryAuthorization::new());
    let records = Arc::new(UndeletableSecret(memory.clone()));
    records.store(&ready_token()).await.unwrap();
    records
        .store_secret(
            server(),
            &TokenMaterial {
                access_token: "access".into(),
                refresh_token: None,
                generation: 1,
            },
        )
        .await
        .unwrap();
    memory
        .set_resource(server(), "https://mcp.example/mcp")
        .await;
    let owner = AuthorizationOwner::new(
        records,
        memory.clone(),
        memory.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    );
    assert_eq!(
        owner
            .revalidate(&[(server(), "https://other.example/mcp".into())])
            .await,
        Err(FenceRefusal)
    );
}

struct UndeletableSecret(Arc<MemoryAuthorization>);

#[async_trait]
impl AuthorizationRecords for UndeletableSecret {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        self.0.load(server).await
    }
    async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure> {
        self.0.store(auth).await
    }
    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        self.0.load_secret(server).await
    }
    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure> {
        self.0.store_secret(server, secret).await
    }
    async fn delete_secret(&self, _server: Uuid) -> Result<Deletion, RecordFailure> {
        Ok(Deletion::Unknown)
    }
}

#[tokio::test]
async fn consent_asks_for_the_challenge_scope_only() {
    let memory = Arc::new(MemoryAuthorization::new());
    memory
        .push_route(
            "https://mcp.example/mcp",
            Ok(OAuthResponse {
                status: 401,
                body: String::new(),
                www_authenticate: Some(
                    "Bearer scope=\"read\", resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\""
                        .into(),
                ),
            }),
        )
        .await;
    memory
        .push_route(
            "https://mcp.example/.well-known/oauth-protected-resource",
            Ok(OAuthResponse {
                status: 200,
                body: r#"{"resource":"https://mcp.example/mcp","authorization_servers":["https://as.example"],"scopes_supported":["mcp","admin"]}"#.into(),
                www_authenticate: None,
            }),
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
    let owner = Arc::new(owner(memory));
    let AuthorizeAnswer::PendingConsent { consent_url, .. } = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await
    else {
        panic!("expected consent");
    };
    assert!(consent_url.contains("scope=read"), "{consent_url}");
    assert!(!consent_url.contains("admin"), "{consent_url}");
}

#[tokio::test]
async fn a_saved_revocation_without_a_worker_is_resumed() {
    let memory = Arc::new(MemoryAuthorization::new());
    let mut stranded = ready_token();
    stranded.phase = Phase::Revoking {
        cause: RevokeCause::Revoke,
        settlement: Settlement {
            local_drained: false,
            secret_deleted: false,
            remote: None,
            evidence_acked: false,
        },
    };
    memory.store(&stranded).await.unwrap();
    memory
        .store_secret(
            server(),
            &TokenMaterial {
                access_token: "access".into(),
                refresh_token: None,
                generation: 1,
            },
        )
        .await
        .unwrap();
    memory.set_resource(server(), &stranded.resource).await;
    let owner = owner(memory.clone());
    let answer = owner.revoke(server()).await;
    assert!(answer.settled, "{answer:?}");
    assert!(answer.secret_deleted);
    assert!(memory.load_secret(server()).await.unwrap().is_none());
}

#[tokio::test]
async fn a_revoke_joins_one_that_is_still_running() {
    let memory = Arc::new(MemoryAuthorization::new());
    let records = Arc::new(GatedDelete::new(memory.clone()));
    records.store(&ready_token()).await.unwrap();
    records
        .store_secret(
            server(),
            &TokenMaterial {
                access_token: "access".into(),
                refresh_token: None,
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
        tokio::spawn(async move { owner.revoke(server()).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !records.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delete started");
    let joined = owner.revoke(server()).await;
    assert!(!joined.settled, "{joined:?}");
    assert_eq!(records.deletes.load(Ordering::SeqCst), 1);
    records.release.notify_one();
    let finished = first.await.unwrap();
    assert!(finished.settled, "{finished:?}");
    assert_eq!(records.deletes.load(Ordering::SeqCst), 1);
}

struct GatedDelete {
    inner: Arc<MemoryAuthorization>,
    entered: AtomicBool,
    release: tokio::sync::Notify,
    deletes: AtomicUsize,
}

impl GatedDelete {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            entered: AtomicBool::new(false),
            release: tokio::sync::Notify::new(),
            deletes: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AuthorizationRecords for GatedDelete {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        self.inner.load(server).await
    }
    async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure> {
        self.inner.store(auth).await
    }
    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        self.inner.load_secret(server).await
    }
    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure> {
        self.inner.store_secret(server, secret).await
    }
    async fn delete_secret(&self, server: Uuid) -> Result<Deletion, RecordFailure> {
        if self.deletes.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.store(true, Ordering::SeqCst);
            self.release.notified().await;
        }
        self.inner.delete_secret(server).await
    }
}

const NEW_RESOURCE: &str = "https://other.example/mcp";

async fn push_host(memory: &MemoryAuthorization, resource: &str) {
    let metadata = format!("{resource}/.well-known/oauth-protected-resource");
    memory
        .push_route(
            resource,
            Ok(OAuthResponse {
                status: 401,
                body: String::new(),
                www_authenticate: Some(format!("Bearer resource_metadata=\"{metadata}\"")),
            }),
        )
        .await;
    memory
        .push_route(
            &metadata,
            Ok(OAuthResponse {
                status: 200,
                body: format!(
                    r#"{{"resource":"{resource}","authorization_servers":["https://as.example"],"scopes_supported":["mcp"]}}"#
                ),
                www_authenticate: None,
            }),
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

fn consent_state(consent_url: &str) -> String {
    consent_url
        .split("state=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned()
}

/// The first token POST waits until `released` passes its index. Later token
/// posts wait the same way, then use the inner client.
struct HoldTokenPosts {
    inner: Arc<MemoryAuthorization>,
    started: AtomicUsize,
    released: AtomicUsize,
}

impl HoldTokenPosts {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            started: AtomicUsize::new(0),
            released: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl OAuthHttp for HoldTokenPosts {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.get(url).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        if url.ends_with("/token") {
            let index = self.started.fetch_add(1, Ordering::SeqCst);
            while self.released.load(Ordering::SeqCst) <= index {
                tokio::task::yield_now().await;
            }
            if index == 0 {
                return Ok(token_body("stale-from-old-host"));
            }
        }
        self.inner.post_form(url, body).await
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.post_json(url, body).await
    }
}

#[tokio::test]
async fn a_callback_after_the_url_changed_does_not_exchange() {
    let memory = Arc::new(MemoryAuthorization::new());
    push_discovery(&memory).await;
    let owner = Arc::new(owner(memory.clone()));
    let AuthorizeAnswer::PendingConsent { consent_url, .. } = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await
    else {
        panic!("expected consent");
    };
    memory.set_resource(server(), NEW_RESOURCE).await;
    memory
        .push_route(
            "https://as.example/token",
            Ok(token_body("should-not-store")),
        )
        .await;
    let answer = owner
        .complete_callback(
            server(),
            CallbackQuery {
                state: consent_state(&consent_url),
                code: Some("code".into()),
                denied: false,
            },
        )
        .await;
    assert!(
        !matches!(answer, AuthorizeAnswer::Ready { .. }),
        "{answer:?}"
    );
    assert!(memory.load_secret(server()).await.unwrap().is_none());
    assert!(
        memory
            .posts()
            .await
            .iter()
            .all(|(url, _, _)| !url.ends_with("/token")),
        "the old attempt exchanged after the URL changed"
    );
}

#[tokio::test]
async fn a_stale_exchange_does_not_bind_its_token_to_the_new_host() {
    let memory = Arc::new(MemoryAuthorization::new());
    let http = Arc::new(HoldTokenPosts::new(memory.clone()));
    push_discovery(&memory).await;
    let owner = Arc::new(AuthorizationOwner::new(
        memory.clone(),
        memory.clone(),
        http.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ));
    let AuthorizeAnswer::PendingConsent { consent_url, .. } = owner
        .authorize(server(), "docs", "https://mcp.example/mcp")
        .await
    else {
        panic!("expected consent");
    };
    let first = {
        let owner = owner.clone();
        let state = consent_state(&consent_url);
        tokio::spawn(async move {
            owner
                .complete_callback(
                    server(),
                    CallbackQuery {
                        state,
                        code: Some("old-code".into()),
                        denied: false,
                    },
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while http.started.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first token post started");
    memory.set_resource(server(), NEW_RESOURCE).await;
    let fenced = owner
        .fence(&[BindingChange::ResourceChanged {
            id: server(),
            previous_url: "https://mcp.example/mcp".into(),
            url: NEW_RESOURCE.into(),
        }])
        .await;
    assert!(fenced.is_ok(), "{fenced:?}");
    push_host(&memory, NEW_RESOURCE).await;
    memory
        .push_route(
            "https://as.example/token",
            Ok(token_body("fresh-for-new-host")),
        )
        .await;
    let AuthorizeAnswer::PendingConsent { consent_url, .. } =
        owner.authorize(server(), "docs", NEW_RESOURCE).await
    else {
        panic!("expected a replacement consent");
    };
    let second = {
        let owner = owner.clone();
        let state = consent_state(&consent_url);
        tokio::spawn(async move {
            owner
                .complete_callback(
                    server(),
                    CallbackQuery {
                        state,
                        code: Some("new-code".into()),
                        denied: false,
                    },
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while http.started.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the replacement exchange reached its token post");
    http.released.store(1, Ordering::SeqCst);
    let stale = first.await.unwrap();
    assert!(!matches!(stale, AuthorizeAnswer::Ready { .. }), "{stale:?}");
    let stored = memory.load_secret(server()).await.unwrap();
    assert!(
        stored.as_ref().map(|secret| secret.access_token.as_str()) != Some("stale-from-old-host"),
        "the previous host's token was stored for the new attempt"
    );
    if let Ok(Some(token)) = owner.bearer(server()).await {
        assert_ne!(token.access_token, "stale-from-old-host");
    }
    http.released.store(2, Ordering::SeqCst);
    let fresh = second.await.unwrap();
    assert!(matches!(fresh, AuthorizeAnswer::Ready { .. }), "{fresh:?}");
    let admitted = owner.bearer(server()).await.unwrap().unwrap();
    assert_eq!(admitted.access_token, "fresh-for-new-host");
}

struct FailingStore {
    inner: Arc<MemoryAuthorization>,
}

#[async_trait]
impl AuthorizationRecords for FailingStore {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        self.inner.load(server).await
    }
    async fn store(&self, _auth: &ServerAuth) -> Result<(), RecordFailure> {
        Err(RecordFailure::Unavailable)
    }
    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        self.inner.load_secret(server).await
    }
    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure> {
        self.inner.store_secret(server, secret).await
    }
    async fn delete_secret(&self, server: Uuid) -> Result<Deletion, RecordFailure> {
        self.inner.delete_secret(server).await
    }
}

/// Holds the revocation POST until `release` is set.
struct HangRevoke {
    inner: Arc<MemoryAuthorization>,
    entered: AtomicBool,
    release: AtomicBool,
}

impl HangRevoke {
    fn new(inner: Arc<MemoryAuthorization>) -> Self {
        Self {
            inner,
            entered: AtomicBool::new(false),
            release: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl OAuthHttp for HangRevoke {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.get(url).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        if url.ends_with("/revoke") {
            self.entered.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        self.inner.post_form(url, body).await
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.inner.post_json(url, body).await
    }
}

#[tokio::test]
async fn an_unfinished_revoke_whose_record_could_not_be_saved_is_not_reusable() {
    let memory = Arc::new(MemoryAuthorization::new());
    let mut ready = ready_with_token_endpoint();
    ready.revocation_endpoint = Some("https://as.example/revoke".into());
    memory.store(&ready).await.unwrap();
    memory
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
    let records = Arc::new(FailingStore {
        inner: memory.clone(),
    });
    let http = Arc::new(HangRevoke::new(memory.clone()));
    let owner = Arc::new(AuthorizationOwner::new(
        records.clone(),
        memory.clone(),
        http.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ));
    let revoking = {
        let owner = owner.clone();
        tokio::spawn(async move { owner.revoke(server()).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !http.entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("revocation post started");
    revoking.abort();
    let restarted = Arc::new(AuthorizationOwner::new(
        records,
        memory.clone(),
        memory.clone(),
        Arc::new(ScriptedCallback::pending()),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        memory.clone(),
        true,
    ));
    assert_eq!(
        restarted.bearer(server()).await,
        Err(AdmissionRefusal::Unauthorized)
    );
    assert!(memory.load_secret(server()).await.unwrap().is_none());
}

#[tokio::test]
async fn a_restart_resumes_a_stranded_revocation() {
    let memory = Arc::new(MemoryAuthorization::new());
    let mut stranded = ready_token();
    stranded.phase = Phase::Revoking {
        cause: RevokeCause::Revoke,
        settlement: Settlement {
            local_drained: false,
            secret_deleted: false,
            remote: None,
            evidence_acked: false,
        },
    };
    stranded.revocation_endpoint = Some("https://as.example/revoke".into());
    memory.store(&stranded).await.unwrap();
    memory
        .store_secret(
            server(),
            &TokenMaterial {
                access_token: "access".into(),
                refresh_token: None,
                generation: 1,
            },
        )
        .await
        .unwrap();
    memory.set_resource(server(), &stranded.resource).await;
    memory
        .push_route(
            "https://as.example/revoke",
            Ok(OAuthResponse {
                status: 200,
                body: String::new(),
                www_authenticate: None,
            }),
        )
        .await;
    let owner = Arc::new(owner(memory.clone()));
    owner
        .revalidate(&[(server(), stranded.resource.clone())])
        .await
        .unwrap();
    assert!(memory.load_secret(server()).await.unwrap().is_none());
    assert_eq!(
        owner.bearer(server()).await,
        Err(AdmissionRefusal::Unauthorized)
    );
}

#[path = "application/callback.rs"]
mod callback_application;
