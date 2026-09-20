//! A runtime's identity is compared, not described, so every part counts.
use super::{RuntimeFingerprint, RuntimeFingerprintError, WarmUpState};

fn fingerprint(model: &str, configuration: &str) -> RuntimeFingerprint {
    RuntimeFingerprint::new("claude-acp", model, configuration).unwrap()
}

#[test]
fn a_runtime_is_identified_by_the_provider_the_model_and_its_configuration() {
    let base = fingerprint("claude-sonnet-5", "sha256:aa");
    assert_eq!(base, base.clone());
    for different in [
        // A new build stages the runtime elsewhere and replaces the binaries
        // beside it. All of that is inside the provider's own configuration
        // identity, so it arrives here as a different configuration.
        fingerprint("claude-sonnet-5", "sha256:bb"),
        fingerprint("claude-opus-5", "sha256:aa"),
        RuntimeFingerprint::new("other-provider", "claude-sonnet-5", "sha256:aa").unwrap(),
    ] {
        assert_ne!(base, different);
    }
}

#[test]
fn a_fingerprint_that_names_no_runtime_is_rejected() {
    assert_eq!(
        RuntimeFingerprint::new("", "model", "sha256:aa"),
        Err(RuntimeFingerprintError::Empty)
    );
    assert_eq!(
        RuntimeFingerprint::new("claude-acp", "", "sha256:aa"),
        Err(RuntimeFingerprintError::Empty)
    );
    // A provider with no settings to describe is legitimate, and the SDK's own
    // identity allows an empty context, so this must not be rejected.
    assert!(RuntimeFingerprint::new("claude-acp", "model", "").is_ok());
    // Retained identities are bounded like any other stored text.
    let long = "x".repeat(4097);
    assert_eq!(
        RuntimeFingerprint::new("claude-acp", "model", &long),
        Err(RuntimeFingerprintError::TooLong)
    );
    assert!(RuntimeFingerprint::new("claude-acp", "model", &"x".repeat(4096)).is_ok());
}

#[test]
fn warm_up_state_reads_the_same_in_every_record() {
    assert_eq!(WarmUpState::Cold.as_str(), "cold");
    assert_eq!(WarmUpState::Warmed.as_str(), "warmed");
    assert_eq!(WarmUpState::Warmed.to_string(), "warmed");
}
