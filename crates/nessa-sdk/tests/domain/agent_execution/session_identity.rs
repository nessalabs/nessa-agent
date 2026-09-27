use nessa_sdk::domain::agent_execution::{
    sessions::{ExecutionSessionId, SessionId},
    ExecutionError,
};

#[test]
fn provider_context_identity_bounds_owned_and_borrowed_utf8_without_normalization() {
    for owned in [false, true] {
        for exact in ["a".repeat(ExecutionSessionId::MAX_BYTES), "é".repeat(128)] {
            let id = if owned {
                ExecutionSessionId::new(exact.clone())
            } else {
                ExecutionSessionId::new(exact.as_str())
            }
            .unwrap();
            assert_eq!(id.as_str(), exact);
            assert_eq!(id.clone(), id);
            let excessive = exact + "a";
            let result = if owned {
                ExecutionSessionId::new(excessive.clone())
            } else {
                ExecutionSessionId::new(excessive.as_str())
            };
            assert_eq!(
                result,
                Err(ExecutionError::ValueTooLong {
                    field: "execution session ID",
                    max_bytes: 256
                })
            );
        }
        for blank in ["", " \n", "\u{2003}"] {
            let result = if owned {
                ExecutionSessionId::new(blank.to_owned())
            } else {
                ExecutionSessionId::new(blank)
            };
            assert_eq!(
                result,
                Err(ExecutionError::EmptyValue("execution session ID"))
            );
        }
    }
    assert_eq!(ExecutionSessionId::new(" α\n").unwrap().as_str(), " α\n");
}

#[test]
fn local_session_keys_bound_owned_and_borrowed_input_and_reject_nonportable_text() {
    for owned in [false, true] {
        let exact = "a".repeat(SessionId::MAX_BYTES);
        let result = if owned {
            SessionId::new(exact.clone())
        } else {
            SessionId::new(exact.as_str())
        };
        assert_eq!(result.unwrap().as_str(), exact);
        for invalid in [
            "a".repeat(SessionId::MAX_BYTES + 1),
            "".into(),
            " \n".into(),
            "é".into(),
            "a/b".into(),
        ] {
            let result = if owned {
                SessionId::new(invalid.clone())
            } else {
                SessionId::new(invalid.as_str())
            };
            assert_eq!(result, Err(ExecutionError::InvalidSessionId));
        }
    }
    assert_eq!(SessionId::new("A-z_09").unwrap().as_str(), "A-z_09");
}
