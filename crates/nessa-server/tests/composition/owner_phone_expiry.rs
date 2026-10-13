//! Canonical expiry on signed resume, including actual remembered receiver effects.
use super::*;
use nessa_auth::{
    application::ports::{AccessError, AccessSnapshot, Clock, Decision, PolicyEvaluator},
    domain::{Action, AuthContext, CredentialId, Resource},
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct ElapsedAfterAdmission {
    canonical: CedarPolicyEvaluator,
    clock: Arc<Time>,
    management_calls: AtomicUsize,
    elapse_at: usize,
}
impl PolicyEvaluator for ElapsedAfterAdmission {
    fn evaluate(
        &self,
        context: &AuthContext,
        action: &Action,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Decision, AccessError> {
        let result = self.canonical.evaluate(context, action, resource, snapshot);
        if action.as_str() == "credential.manage"
            && self.management_calls.fetch_add(1, Ordering::SeqCst) + 1 == self.elapse_at
        {
            self.clock.0.fetch_add(86_400_000, Ordering::SeqCst);
        }
        result
    }
}

async fn expired_resume(elapse_at: usize, expected_prior: &str) {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let clock = Arc::new(Time(AtomicU64::new(SystemClock.unix_milliseconds())));
    let mut inputs = namespace.inputs(registry.clone());
    let receivers = inputs.receivers.clone();
    inputs.clock = clock.clone();
    inputs.policy = Arc::new(ElapsedAfterAdmission {
        canonical: CedarPolicyEvaluator::new().unwrap(),
        clock: clock.clone(),
        management_calls: AtomicUsize::new(0),
        elapse_at,
    });
    let (prepared, commands, _peers) = prepare(&loopback(), inputs).await.unwrap();
    let state = product(registry.clone()).with_pairing(Arc::new(commands));
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap();
    });
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 72 + elapse_at as u8);
        let initial_effects = super::refusals::receiver_effects(&namespace);
        let prior = client.ok("phone.authorize", params.clone());
        assert_eq!(prior["phase"], expected_prior, "{prior}");
        assert_eq!(prior["activationStopped"], "permanent");
        assert!(prior["expiresAtMs"].as_u64().unwrap() < clock.unix_milliseconds());
        let prior_binding = prior["credentialId"]
            .as_str()
            .map(|cid| receivers.binding(&CredentialId::new(cid).unwrap()).unwrap());
        if expected_prior == "staging" {
            assert!(prior_binding.as_ref().unwrap().as_ref().unwrap().active);
        } else {
            assert!(prior["credentialId"].is_null());
            assert_eq!(
                super::refusals::receiver_effects(&namespace),
                initial_effects,
                "expired Approved cannot dispatch Pair"
            );
        }
        let path = namespace.root().join("auth/credentials.json");
        let before = std::fs::read(&path).unwrap();
        let before_effects = super::refusals::receiver_effects(&namespace);
        let status = client.ok(
            "phone.status",
            json!({"requestId": prior["original"]["requestId"],
            "originalDigest": prior["originalDigest"]}),
        );
        assert_eq!(status["phase"], expected_prior);
        assert!(
            std::fs::read(&path).unwrap() == before,
            "read-only expired status cannot expire or clean up"
        );
        assert_eq!(
            super::refusals::receiver_effects(&namespace),
            before_effects,
            "read-only status cannot fence or change the receiver journal"
        );
        if let Some(Some(binding)) = prior_binding.as_ref() {
            assert_eq!(
                receivers.binding(&binding.credential_id).unwrap().as_ref(),
                Some(binding)
            );
        }
        let resumed = client.ok("phone.authorize", params);
        assert_eq!(resumed["invitationId"], prior["invitationId"]);
        assert_eq!(resumed["originalDigest"], prior["originalDigest"]);
        assert_eq!(resumed["phase"], "terminal", "{resumed}");
        assert_eq!(resumed["terminal"]["cause"], "expired");
        assert_eq!(resumed["cleanupPending"], false);
        assert_eq!(
            resumed["receiver"]["receiverId"], prior["receiver"]["receiverId"],
            "terminal retains the original receiver identity"
        );
        if let Some(Some(binding)) = prior_binding {
            let current = receivers.binding(&binding.credential_id).unwrap().unwrap();
            assert!(!current.active);
            assert_eq!(
                resumed["receiver"]["accessEpoch"], current.access_epoch,
                "terminal discloses the actual fenced receiver epoch"
            );
            assert!(
                current.access_epoch > binding.access_epoch,
                "actual cleanup advances the original receiver epoch"
            );
            assert_eq!(
                current.receiver_id, binding.receiver_id,
                "cleanup cannot replace the original receiver"
            );
        }
    });
    serving.abort();
    let _ = serving.await;
    drop(bound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_resume_expires_approved_original_without_pairing() {
    expired_resume(2, "approved").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_resume_expires_staging_original_and_fences_its_actual_receiver() {
    expired_resume(3, "staging").await;
}
