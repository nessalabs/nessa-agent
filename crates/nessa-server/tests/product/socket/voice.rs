// The voice methods at the socket: refused without the manifest's grant,
// shape-checked before anything is kept, `voice_not_configured` without
// voice, and no answer ever holding the key (`docs/design/voice-chat.md`).
use super::*;
use crate::secrets::{SecretStore, SecretUnavailable};
use crate::voice::application::{
    AuditUnavailable, MintFailure, RealtimeProvider, VoiceAudit, VoiceCalls, VoiceRecord,
};
use crate::voice::domain::{CallGrant, CallTransport, VoiceKey};

const KEY: &str = "sk-route-test-key";
const CHAT: &str = "00000000-0000-4000-8000-000000000001";

/// The gateway keeps one conversation, `CHAT`.
fn with_chat(state: ProductRouteState) -> ProductRouteState {
    state.with_passive_read(
        Arc::new(UnpairedBinding),
        repository_of(vec![owned_conversation(CHAT, "owner")]),
        Arc::new(GrantedConversation(CHAT.into())),
        Arc::new(conversation_support::ObservedPolicy::default()),
    )
}

#[derive(Default)]
struct Memory(Mutex<Option<Vec<u8>>>);

impl SecretStore for Memory {
    fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
        Ok(self.0.lock().unwrap().clone().map(zeroize::Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        *self.0.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

struct Minting;

#[async_trait::async_trait]
impl RealtimeProvider for Minting {
    async fn mint(&self, _key: &VoiceKey) -> Result<CallGrant, MintFailure> {
        Ok(CallGrant {
            transport: CallTransport::Scripted,
            call_url: "ws://127.0.0.1:1/realtime".into(),
            client_secret: "ek_route".into(),
            expires_at_ms: 60_000,
            model: "gpt-realtime".into(),
        })
    }
}

#[derive(Default)]
struct Kept(Mutex<Vec<VoiceRecord>>);

impl VoiceAudit for Kept {
    fn record(&self, record: &VoiceRecord) -> Result<(), AuditUnavailable> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

fn voice() -> (Arc<VoiceCalls>, Arc<Memory>, Arc<Kept>) {
    let memory = Arc::new(Memory::default());
    let kept = Arc::new(Kept::default());
    let calls = Arc::new(VoiceCalls::new(
        memory.clone(),
        Arc::new(Minting),
        kept.clone(),
    ));
    (calls, memory, kept)
}

fn with(id: &str, method: &str, params: Value) -> RequestFrame {
    let mut frame = request(id, method);
    frame.params = params;
    frame
}

fn payload(message: OutgoingMessage) -> Value {
    let OutgoingMessage::Response(response) = message else {
        panic!("response expected");
    };
    assert!(response.ok, "{:?}", response.error);
    response.payload.unwrap()
}

#[tokio::test]
async fn without_voice_every_method_says_it_is_not_configured() {
    let (state, _) = fixture(MembershipRole::Admin);
    let session = authenticate(&state).await;
    assert_eq!(
        code_of(dispatch(&state, &session, request("1", "voice.status")).await),
        "voice_not_configured"
    );
}

#[tokio::test]
async fn a_gateway_without_conversations_starts_no_call() {
    let (calls, _, _) = voice();
    let (state, authority) = fixture(MembershipRole::Admin);
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Admin,
        MembershipStatus::Active,
        &["conversation.write"],
    );
    let state = state.with_voice(calls);
    let session = authenticate(&state).await;
    let start = with("1", "voice.startCall", json!({ "conversationId": CHAT }));
    assert_eq!(
        code_of(dispatch(&state, &session, start).await),
        "conversations_not_configured"
    );
}

#[tokio::test]
async fn the_key_methods_need_credential_manage_and_a_call_conversation_write() {
    let (calls, memory, _) = voice();
    let (state, authority) = fixture(MembershipRole::Member);
    let state = with_chat(state.with_voice(calls));
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Member,
        MembershipStatus::Active,
        &["conversation.write"],
    );
    let session = authenticate(&state).await;
    let save = with("1", "voice.saveKey", json!({ "key": KEY }));
    assert_eq!(code_of(dispatch(&state, &session, save).await), "forbidden");
    assert!(memory.0.lock().unwrap().is_none());
    // Admitted to start a call; there is no key to start it with.
    let start = with(
        "2",
        "voice.startCall",
        json!({ "conversationId": CHAT }),
    );
    assert_eq!(
        code_of(dispatch(&state, &session, start).await),
        "voice_key_missing"
    );
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Member,
        MembershipStatus::Active,
        &["credential.manage"],
    );
    let start = with(
        "3",
        "voice.startCall",
        json!({ "conversationId": CHAT }),
    );
    assert_eq!(code_of(dispatch(&state, &session, start).await), "forbidden");
}

#[tokio::test]
async fn a_saved_key_is_never_answered_and_shapes_are_checked_first() {
    let (calls, memory, kept) = voice();
    let (state, authority) = fixture(MembershipRole::Admin);
    *authority.snapshot.lock().unwrap() = snapshot_with(
        MembershipRole::Admin,
        MembershipStatus::Active,
        &["credential.manage", "conversation.write"],
    );
    let state = with_chat(state.with_voice(calls));
    let session = authenticate(&state).await;
    for (id, params) in [
        ("1", json!({ "key": "has a space" })),
        ("2", json!({ "key": "" })),
        ("3", json!({ "key": KEY, "extra": true })),
    ] {
        let frame = with(id, "voice.saveKey", params);
        assert_eq!(
            code_of(dispatch(&state, &session, frame).await),
            "invalid_request"
        );
    }
    assert!(kept.0.lock().unwrap().is_empty());
    let saved = payload(dispatch(&state, &session, with("4", "voice.saveKey", json!({ "key": KEY }))).await);
    assert_eq!(saved["keySaved"], true);
    assert!(!saved.to_string().contains(KEY));
    assert_eq!(memory.0.lock().unwrap().as_deref(), Some(KEY.as_bytes()));
    let status = payload(dispatch(&state, &session, request("5", "voice.status")).await);
    assert!(!status.to_string().contains(KEY));
    // A call names a conversation by its id, or is refused before any record.
    let records = kept.0.lock().unwrap().len();
    let bad = with("6", "voice.startCall", json!({ "conversationId": "not-an-id" }));
    assert_eq!(
        code_of(dispatch(&state, &session, bad).await),
        "invalid_request"
    );
    // Nor about a conversation the gateway does not keep.
    let missing = with(
        "6b",
        "voice.startCall",
        json!({ "conversationId": "00000000-0000-4000-8000-0000000000ff" }),
    );
    assert_eq!(
        code_of(dispatch(&state, &session, missing).await),
        "conversation_not_found"
    );
    assert_eq!(kept.0.lock().unwrap().len(), records);
    let call = payload(
        dispatch(
            &state,
            &session,
            with(
                "7",
                "voice.startCall",
                json!({ "conversationId": CHAT }),
            ),
        )
        .await,
    );
    assert_eq!(call["transport"], "scripted");
    assert_eq!(call["clientSecret"], "ek_route");
    assert!(!call.to_string().contains(KEY));
}
