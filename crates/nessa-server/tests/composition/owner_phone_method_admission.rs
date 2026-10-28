//! Draft actual native product requests with canonical policy invocation accounting.
use super::*;
use crate::conversation::application::{
    OwnerPhoneDecision, OwnerPhoneOperation, OwnerPhonePolicyEvaluator, OwnerPhonePolicyFacts,
};
use crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator;
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeTransport},
    application::ports::AccessError,
};
use nessa_protocol::pairing::{
    encode_frame,
    wire::{encode_request, NativePairingRequest},
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};
struct CountPolicy {
    real: CedarOwnerPhonePolicyEvaluator,
    methods: Mutex<Vec<String>>,
}
impl OwnerPhonePolicyEvaluator for CountPolicy {
    fn evaluate(
        &self,
        facts: OwnerPhonePolicyFacts<'_>,
    ) -> Result<OwnerPhoneDecision, AccessError> {
        if let OwnerPhoneOperation::ProductMethod { method, .. } = &facts.operation {
            self.methods.lock().unwrap().push((*method).to_owned());
        }
        self.real.evaluate(facts)
    }
}
struct HealthEffect(AtomicUsize);
impl nessa_protocol::clock::Clock for HealthEffect {
    fn elapsed_ms(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst);
        17
    }
}
struct Probe {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
    next: u64,
}
impl Probe {
    fn frame(&mut self) -> Value {
        loop {
            let count = self
                .transport
                .read(self.frames.unfilled().unwrap())
                .unwrap();
            assert!(count > 0, "actual product connection remains open");
            if let Some(body) = self.frames.filled(count).unwrap() {
                return serde_json::from_slice(&body).unwrap();
            }
        }
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        let body = json!({"type":"req","id":id,"method":method,"params":params}).to_string();
        self.transport
            .write_all(&encode_frame(MAX_PROTECTED_REQUEST_BYTES, body.as_bytes()).unwrap())
            .unwrap();
        self.transport.flush().unwrap();
        loop {
            let frame = self.frame();
            if frame["type"] == "res" && frame["id"] == id {
                return frame;
            }
        }
    }
    fn open(original: &nessa_auth::domain::pairing::OwnerPhoneRequest, credential: &str) -> Self {
        let identity =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([82; 32]))).unwrap();
        let stream = TcpStream::connect(original.endpoint()).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        let transport = NativeTransport::connect(
            stream,
            &identity,
            GatewayTrust::Pinned(*original.gateway_pin()),
        )
        .unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        channel
            .send_envelope(&encode_request(&NativePairingRequest::OpenProduct).unwrap())
            .unwrap();
        let mut probe = Self {
            transport: channel.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES),
            next: 0,
        };
        let challenge = probe.frame();
        assert_eq!(challenge["event"], "session.challenge");
        let ready=probe.call("session.authenticate",json!({"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],"credential":credential,"client":{"id":"app.nessa.mobile"},"surface":{"kind":"panel","instance":"app.nessa.mobile"}}));
        assert_eq!(
            ready["ok"], true,
            "actual issued native identity authenticates"
        );
        probe
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_native_request_asks_method_once_and_denial_has_zero_handler_effects() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let inputs = namespace.inputs(registry.clone());
    let receivers = inputs.receivers.clone();
    let (prepared, commands, _) = prepared_loopback(inputs).await;
    let policy = Arc::new(CountPolicy {
        real: CedarOwnerPhonePolicyEvaluator::new().unwrap(),
        methods: Mutex::new(vec![]),
    });
    let health = Arc::new(HealthEffect(AtomicUsize::new(0)));
    let mut state = product(registry.clone()).with_pairing(Arc::new(commands));
    state.owner_phone_policy = policy.clone();
    state.conversation_access = Some(crate::conversation_test_support::access_policy());
    state.owner_phone_receivers = Some(receivers.clone());
    state.uptime_clock = health.clone();
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let (failure, failed) = watch::channel(None);
    let mut running = start(bound, failure);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap()
    });
    let (params, active) = tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 82);
        let active = client.ok("phone.authorize", params.clone());
        (params, active)
    });
    let original =
        owner_phone::request(serde_json::from_value(params["original"].clone()).unwrap()).unwrap();
    let path = namespace.root().join("auth/credentials.json");
    let bytes = std::fs::read(&path).unwrap();
    let effects = super::refusals::receiver_effects(&namespace);
    tokio::task::block_in_place(|| {
        let mut probe = Probe::open(&original, active["credentialId"].as_str().unwrap());
        policy.methods.lock().unwrap().clear();
        health.0.store(0, Ordering::SeqCst);
        let allowed = probe.call("server.health", json!({}));
        assert_eq!(allowed["ok"], true);
        assert_eq!(
            health.0.load(Ordering::SeqCst),
            1,
            "the actual allowed handler executes once"
        );
        assert_eq!(
            policy.methods.lock().unwrap().clone(),
            vec!["server.health".to_owned()],
            "ordinary method policy is asked once"
        );
        policy.methods.lock().unwrap().clear();
        let denied = probe.call("agents.install", json!({}));
        assert_eq!(denied["ok"], false);
        assert_eq!(denied["error"]["code"],"forbidden","canonical method policy precedes handler validation/effects despite its coarse conversation.write grant");
        assert_eq!(
            policy.methods.lock().unwrap().clone(),
            vec!["agents.install".to_owned()]
        );
        assert_eq!(health.0.load(Ordering::SeqCst), 1);
        // The phone's card reads what a review or question asks before it is
        // answered: the method policy admits both reads, as it does the answers.
        for (method, key) in [
            ("conversation.permissionReview", "review"),
            ("conversation.questionReview", "question"),
        ] {
            policy.methods.lock().unwrap().clear();
            let read = probe.call(
                method,
                json!({
                    "conversationId": "00000000-0000-4000-8000-000000000001",
                    key: "0".repeat(32),
                    "offset": 0,
                    "maxBytes": 1024,
                }),
            );
            assert_ne!(read["error"]["code"], "forbidden", "{method}: {read}");
            assert_eq!(
                policy.methods.lock().unwrap().clone(),
                vec![method.to_owned()]
            );
        }
    });
    assert!(
        std::fs::read(&path).unwrap() == bytes,
        "ordinary reads/refusals do not alter credential history"
    );
    assert_eq!(super::refusals::receiver_effects(&namespace), effects);
    running.signal_stop();
    assert!(matches!(
        tokio::time::timeout(WAIT, running.join()).await,
        Ok(Ok(()))
    ));
    assert!(failed.borrow().is_none());
    serving.abort();
    let _ = serving.await;
}
