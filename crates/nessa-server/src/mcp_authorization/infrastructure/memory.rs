//! In-memory records for tests, with a gate in front of each secret write
//! and each audit so a test can order a lost reply against revoke.
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::mcp_authorization::application::{
    AuditFailure, AuthAuditRecord, AuthClock, AuthorizationAudit, AuthorizationRecords,
    CallbackBind, CallbackQuery, ConsentCallback, Entropy, OAuthCallFailure, OAuthHttp,
    OAuthResponse, RecordFailure, ResourceLookup, SessionDrain, TokenMaterial,
};
use crate::mcp_authorization::domain::{Deletion, Publication, ServerAuth};

/// `Proceed` runs the effect. `Refuse` is a definite refusal before a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateMode {
    Proceed,
    Refuse,
}

struct Gate {
    mode: GateMode,
    entered: Notify,
}

impl Gate {
    fn open() -> Self {
        Self {
            mode: GateMode::Proceed,
            entered: Notify::new(),
        }
    }
}

/// Records, secrets, audit and scripted HTTPS answers. Nothing is written
/// outside the process.
pub struct MemoryAuthorization {
    records: Mutex<HashMap<Uuid, ServerAuth>>,
    secrets: Mutex<HashMap<Uuid, TokenMaterial>>,
    audits: Mutex<Vec<AuthAuditRecord>>,
    secret_gate: Mutex<Gate>,
    audit_gate: Mutex<Gate>,
    routes: Mutex<Vec<(String, Result<OAuthResponse, OAuthCallFailure>)>>,
    posts: Mutex<Vec<(String, bool, String)>>,
    drained: Mutex<Vec<String>>,
    resources: Mutex<HashMap<Uuid, String>>,
    now: Mutex<u64>,
    writer: bool,
}

impl MemoryAuthorization {
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
            secrets: Mutex::new(HashMap::new()),
            audits: Mutex::new(Vec::new()),
            secret_gate: Mutex::new(Gate::open()),
            audit_gate: Mutex::new(Gate::open()),
            routes: Mutex::new(Vec::new()),
            posts: Mutex::new(Vec::new()),
            drained: Mutex::new(Vec::new()),
            resources: Mutex::new(HashMap::new()),
            now: Mutex::new(1_000),
            writer: true,
        }
    }

    pub fn without_writer() -> Self {
        let mut store = Self::new();
        store.writer = false;
        store
    }

    pub fn writer(&self) -> bool {
        self.writer
    }

    pub async fn push_route(&self, url: &str, response: Result<OAuthResponse, OAuthCallFailure>) {
        self.routes.lock().await.push((url.to_owned(), response));
    }

    /// Posts the owner sent: URL, whether the body was JSON, and the body.
    pub async fn posts(&self) -> Vec<(String, bool, String)> {
        self.posts.lock().await.clone()
    }

    pub async fn set_resource(&self, server: Uuid, url: &str) {
        self.resources.lock().await.insert(server, url.to_owned());
    }

    pub async fn audits(&self) -> Vec<AuthAuditRecord> {
        self.audits.lock().await.clone()
    }

    pub async fn drained(&self) -> Vec<String> {
        self.drained.lock().await.clone()
    }

    pub fn set_now(&self, now: u64) {
        // Tests set the clock before the owner reads it. The mutex is taken
        // from the async methods; this synchronous setter is for arranging
        // a deadline before the runtime polls the owner.
        if let Ok(mut clock) = self.now.try_lock() {
            *clock = now;
        }
    }

    pub async fn advance_to(&self, now: u64) {
        *self.now.lock().await = now;
    }
}

#[async_trait]
impl AuthorizationRecords for MemoryAuthorization {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        Ok(self.records.lock().await.get(&server).cloned())
    }

    async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure> {
        self.records.lock().await.insert(auth.server, auth.clone());
        Ok(())
    }

    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        if !self.writer {
            return Err(RecordFailure::Unavailable);
        }
        Ok(self.secrets.lock().await.get(&server).cloned())
    }

    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure> {
        if !self.writer {
            return Err(RecordFailure::Unavailable);
        }
        let mode = {
            let gate = self.secret_gate.lock().await;
            gate.entered.notify_waiters();
            gate.mode
        };
        match mode {
            GateMode::Proceed => {
                self.secrets.lock().await.insert(server, secret.clone());
                Ok(Publication::Acknowledged)
            }
            GateMode::Refuse => Ok(Publication::Refused),
        }
    }

    async fn delete_secret(&self, server: Uuid) -> Result<Deletion, RecordFailure> {
        if !self.writer {
            return Ok(Deletion::Unknown);
        }
        self.secrets.lock().await.remove(&server);
        Ok(Deletion::Deleted)
    }
}

#[async_trait]
impl AuthorizationAudit for MemoryAuthorization {
    async fn record(&self, record: &AuthAuditRecord) -> Result<(), AuditFailure> {
        let mode = {
            let gate = self.audit_gate.lock().await;
            gate.entered.notify_waiters();
            gate.mode
        };
        if mode == GateMode::Refuse {
            return Err(AuditFailure);
        }
        self.audits.lock().await.push(record.clone());
        Ok(())
    }
}

#[async_trait]
impl OAuthHttp for MemoryAuthorization {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.take(url).await
    }

    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.note(url, false, body).await;
        self.take(url).await
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        self.note(url, true, body).await;
        self.take(url).await
    }
}

impl MemoryAuthorization {
    async fn note(&self, url: &str, json: bool, body: &str) {
        self.posts
            .lock()
            .await
            .push((url.to_owned(), json, body.to_owned()));
    }

    async fn take(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure> {
        let mut routes = self.routes.lock().await;
        let index = routes.iter().position(|(candidate, _)| candidate == url);
        match index {
            Some(index) => routes.remove(index).1,
            None => Err(OAuthCallFailure::NotSent),
        }
    }
}

impl AuthClock for MemoryAuthorization {
    fn now_ms(&self) -> u64 {
        self.now.try_lock().map(|now| *now).unwrap_or(1_000)
    }
}

impl Entropy for MemoryAuthorization {
    fn bytes(&self, len: usize) -> Result<Vec<u8>, ()> {
        Ok(vec![7; len])
    }
}

#[async_trait]
impl SessionDrain for MemoryAuthorization {
    async fn drain(&self, server_name: &str) {
        self.drained.lock().await.push(server_name.to_owned());
    }
}

impl ResourceLookup for MemoryAuthorization {
    fn resource(&self, server: Uuid) -> Option<String> {
        self.resources.try_lock().ok()?.get(&server).cloned()
    }
}

/// A callback the test completes, or that never answers.
pub struct ScriptedCallback {
    query: Mutex<Option<CallbackQuery>>,
}

impl ScriptedCallback {
    pub fn pending() -> Self {
        Self {
            query: Mutex::new(None),
        }
    }

    pub fn with(query: CallbackQuery) -> Self {
        Self {
            query: Mutex::new(Some(query)),
        }
    }
}

#[async_trait]
impl ConsentCallback for ScriptedCallback {
    async fn listen(&self, _wait_for: std::time::Duration) -> Result<CallbackBind, ()> {
        let (sender, accepted) = tokio::sync::oneshot::channel();
        if let Some(query) = self.query.lock().await.take() {
            let _ = sender.send(query);
        }
        Ok(CallbackBind {
            redirect_uri: "http://127.0.0.1:9/mcp-oauth/callback".to_owned(),
            accepted,
        })
    }
}
