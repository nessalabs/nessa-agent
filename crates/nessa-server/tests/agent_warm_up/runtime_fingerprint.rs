//! A runtime's identity is compared, not described, so every part counts.
use super::{RuntimeFingerprint, RuntimeFingerprintError, WarmUpState};

#[test]
fn a_runtime_is_identified_by_everything_that_would_be_launched() {
    let base = RuntimeFingerprint::new(
        "/Applications/Nessa.app/runtimes/aa/node",
        "/Applications/Nessa.app/runtimes/aa/acp/index.js",
        "claude-sonnet-5",
    )
    .unwrap();
    assert_eq!(base, base.clone());
    // An update stages the runtime under a new directory, which is exactly when
    // the operating system scans the files again.
    for different in [
        RuntimeFingerprint::new(
            "/Applications/Nessa.app/runtimes/bb/node",
            "/Applications/Nessa.app/runtimes/aa/acp/index.js",
            "claude-sonnet-5",
        ),
        RuntimeFingerprint::new(
            "/Applications/Nessa.app/runtimes/aa/node",
            "/Applications/Nessa.app/runtimes/bb/acp/index.js",
            "claude-sonnet-5",
        ),
        RuntimeFingerprint::new(
            "/Applications/Nessa.app/runtimes/aa/node",
            "/Applications/Nessa.app/runtimes/aa/acp/index.js",
            "claude-opus-5",
        ),
    ] {
        assert_ne!(base, different.unwrap());
    }
}

#[test]
fn a_fingerprint_that_names_nothing_is_rejected() {
    assert_eq!(
        RuntimeFingerprint::new("", "/entry", "model"),
        Err(RuntimeFingerprintError::Empty)
    );
    assert_eq!(
        RuntimeFingerprint::new("/node", "", "model"),
        Err(RuntimeFingerprintError::Empty)
    );
    assert_eq!(
        RuntimeFingerprint::new("/node", "/entry", ""),
        Err(RuntimeFingerprintError::Empty)
    );
    // Retained identities are bounded like any other stored text.
    let long = "/".repeat(4097);
    assert_eq!(
        RuntimeFingerprint::new(&long, "/entry", "model"),
        Err(RuntimeFingerprintError::TooLong)
    );
    assert!(RuntimeFingerprint::new(&"/".repeat(4096), "/entry", "model").is_ok());
}

#[test]
fn warm_up_state_reads_the_same_in_every_record() {
    assert_eq!(WarmUpState::Cold.as_str(), "cold");
    assert_eq!(WarmUpState::Warmed.as_str(), "warmed");
    assert_eq!(WarmUpState::Warmed.to_string(), "warmed");
}
