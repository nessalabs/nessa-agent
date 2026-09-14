use super::{ExecutionSessionId, SessionId};

#[test]
fn owned_identity_inputs_and_clones_retain_only_their_text_allocation() {
    let mut oversized = String::with_capacity(1 << 20);
    oversized.push_str("context");
    assert!(oversized.capacity() > oversized.len());
    let provider = ExecutionSessionId::new(oversized).unwrap();
    let provider_clone = provider.clone();
    assert_eq!(provider.0.into_string().capacity(), "context".len());
    assert_eq!(provider_clone.0.into_string().capacity(), "context".len());

    let mut oversized = String::with_capacity(1 << 20);
    oversized.push_str("local_session");
    let local = SessionId::new(oversized).unwrap();
    let local_clone = local.clone();
    assert_eq!(local.0.into_string().capacity(), "local_session".len());
    assert_eq!(
        local_clone.0.into_string().capacity(),
        "local_session".len()
    );
}
