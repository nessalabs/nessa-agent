//! Presentation preserves independent typed evidence and redacts adapter diagnostics.
use super::*;
use crate::read_only_sync::application::{driver::CapturedCheck, GatewayError, GatewayOutcome};
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sync::replication::application::SyncError;

#[test]
fn online_failure_output_keeps_captured_check_and_independent_causes() {
    let attempt = GatewayAttempt {
        result: Some(Err(RecordDriverError {
            cause: RecordDriverCause::Core(SyncError::Denied),
            freshness_failure: Some(CacheError::Quota),
            captured_check: Some(CapturedCheck {
                head: 3,
                checked_at_ms: 9,
            }),
        })),
        outcome: GatewayOutcome {
            operation: 1,
            failure: Some(GatewayError::TimedOut),
        },
    };
    let mut output = vec![];
    assert!(matches!(
        write_records(
            &attempt,
            Err(CacheError::Corrupt),
            Some(CacheError::Stale),
            json!({"enrollment":{"phase":"active"},"recheck":{"enrollment":{"phase":"terminal"}}}),
            &mut output
        ),
        Err(CommandError::OnlineRefused)
    ));
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["capturedCheck"], json!({"head":"3","checkedAtMs":9}));
    assert_eq!(
        value["driverFailure"],
        json!({"owner":"core","cause":{"code":"denied"}})
    );
    assert_eq!(value["transportFailure"]["code"], "timedOut");
    assert_eq!(value["connectionOperation"], "1");
    assert_eq!(value["durable"]["failure"]["code"], "corrupt");
    assert_eq!(value["cacheRefusal"]["code"], "stale");
    assert_eq!(value["freshnessFailure"]["code"], "quota");
    assert_eq!(value["successful"], false);
    // The enrollment the run was admitted under, and its recheck, join the
    // report beside the transport and driver causes.
    assert_eq!(value["enrollment"]["phase"], "active");
    assert_eq!(value["recheck"]["enrollment"]["phase"], "terminal");
}
#[test]
fn refused_open_product_is_reported_as_product_refused() {
    // The example client reads this code to tell a refused `openProduct`
    // from a malformed frame, and re-asks status only for the former.
    assert_eq!(
        gateway_failure(GatewayError::ProductRefused),
        json!({"code":"productRefused","productCode":null})
    );
}

#[test]
fn successful_work_with_unavailable_saved_evidence_is_refused_and_diagnostics_redacted() {
    let attempt = GatewayAttempt {
        result: Some(Ok(RecordRun {
            checked_head: 0,
            checked_at_ms: 7,
            downloaded: 0,
            pages: 0,
            complete: true,
        })),
        outcome: GatewayOutcome {
            operation: 1,
            failure: None,
        },
    };
    let mut output = vec![];
    assert!(matches!(
        write_records(
            &attempt,
            Err(CacheError::Transcript(StorageError::Io(
                "private-path-token".into()
            ))),
            None,
            Value::Null,
            &mut output
        ),
        Err(CommandError::OnlineRefused)
    ));
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["successful"], false);
    assert_eq!(value["work"]["complete"], true);
    assert_eq!(
        value["durable"]["failure"],
        json!({"code":"transcript","cause":{"code":"io"}})
    );
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("private-path-token"));
}
