//! `fetch`'s line: the file as named, how far the fetch got, and a typed
//! cause when it kept nothing.
use super::*;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};

fn file() -> HeldFile {
    HeldFile::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        Sha256Digest::from_bytes([1; 32]),
        MediaType::parse("text/plain").unwrap(),
        70_000,
    )
    .unwrap()
}
const PROGRESS: FetchProgress = FetchProgress {
    resumed_from: 32_768,
    reached: 65_536,
    ranges: 1,
    busy_retries: 2,
};
fn render(result: Result<FetchProgress, FetchFailure>) -> (Value, Result<(), CommandError>) {
    let mut output = vec![];
    let written = write_fetch(
        &file(),
        result,
        true,
        json!({"enrollment":{"phase":"active"},"recheck":null}),
        &mut output,
    );
    assert_eq!(output.last(), Some(&b'\n'));
    (serde_json::from_slice(&output).unwrap(), written)
}

#[test]
fn a_kept_file_reports_where_it_began_and_what_it_read() {
    let (value, ok) = render(Ok(FetchProgress {
        reached: 70_000,
        ranges: 2,
        ..PROGRESS
    }));
    assert!(ok.is_ok());
    assert_eq!(
        value,
        json!({
            "operation":"fetch","successful":true,"connectionCheck":"performed",
            "file":{"conversationId":"00000000-0000-4000-8000-000000000001",
                "digest":format!("sha256:{}", "01".repeat(32)),"mimeType":"text/plain","size":"70000"},
            "resumedFrom":"32768","reached":"70000","ranges":"2","busyRetries":"2",
            "fetchFailure":null,"enrollment":{"phase":"active"},"recheck":null,
        })
    );
}

#[test]
fn each_failure_keeps_its_typed_cause() {
    let online =
        |result: &Result<(), CommandError>| matches!(result, Err(CommandError::OnlineRefused));
    let local = |result: &Result<(), CommandError>| matches!(result, Err(CommandError::LocalFile));
    for (cause, expected, exit) in [
        (
            FetchCause::Gateway(GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::AttachmentNotFound,
            ))),
            json!({"code":"gateway","transportFailure":{"code":"heldFile","productCode":"attachment_not_found"}}),
            online as fn(&Result<(), CommandError>) -> bool,
        ),
        (
            FetchCause::Gateway(GatewayError::HeldFile(HeldFileRefusal::Forbidden)),
            json!({"code":"gateway","transportFailure":{"code":"heldFile","productCode":"forbidden"}}),
            online,
        ),
        (
            FetchCause::Gateway(GatewayError::HeldFile(HeldFileRefusal::UnknownMethod)),
            json!({"code":"unsupported","transportFailure":{"code":"heldFile","productCode":"unknown_method"}}),
            online,
        ),
        (
            FetchCause::Gateway(GatewayError::TimedOut),
            json!({"code":"gateway","transportFailure":{"code":"timedOut","productCode":null}}),
            online,
        ),
        (FetchCause::WrongRange, json!({"code":"wrongRange"}), online),
        (
            FetchCause::Integrity {
                cleanup: PartCleanup::Removed,
            },
            json!({"code":"integrity","part":"removed","removeFailure":null,"restartFailure":null}),
            online,
        ),
        (
            FetchCause::Integrity {
                cleanup: PartCleanup::Emptied {
                    remove: FileStep::Remove,
                },
            },
            json!({"code":"integrity","part":"emptied","removeFailure":"remove","restartFailure":null}),
            online,
        ),
        (
            FetchCause::Integrity {
                cleanup: PartCleanup::Left {
                    remove: FileStep::Remove,
                    restart: FileStep::Restart,
                },
            },
            json!({"code":"integrity","part":"left","removeFailure":"remove","restartFailure":"restart"}),
            online,
        ),
        (
            FetchCause::OutputExists,
            json!({"code":"outputExists"}),
            local,
        ),
        (
            FetchCause::File(FileStep::Locked),
            json!({"code":"file","step":"locked"}),
            local,
        ),
        (
            FetchCause::Published {
                step: FileStep::Remove,
            },
            json!({"code":"linked","step":"remove"}),
            local,
        ),
        (
            FetchCause::Published {
                step: FileStep::SyncDirectory,
            },
            json!({"code":"linked","step":"syncDirectory"}),
            local,
        ),
        (
            FetchCause::File(FileStep::Link),
            json!({"code":"file","step":"link"}),
            local,
        ),
        (
            FetchCause::File(FileStep::Name),
            json!({"code":"file","step":"name"}),
            local,
        ),
        (
            FetchCause::File(FileStep::Changed),
            json!({"code":"file","step":"changed"}),
            local,
        ),
    ] {
        let (value, written) = render(Err(FetchFailure {
            cause,
            progress: PROGRESS,
        }));
        assert!(exit(&written), "{cause:?}: {written:?}");
        assert_eq!(value["successful"], false);
        assert_eq!(value["fetchFailure"], expected, "{cause:?}");
        assert_eq!(value["reached"], "65536");
        assert_eq!(value["enrollment"]["phase"], "active");
    }
}

#[test]
fn an_output_answered_without_the_gateway_says_so() {
    let mut output = vec![];
    write_fetch(
        &file(),
        Ok(FetchProgress {
            resumed_from: 70_000,
            reached: 70_000,
            ..FetchProgress::default()
        }),
        false,
        json!({}),
        &mut output,
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["connectionCheck"], "notPerformed");
    assert_eq!(value["ranges"], "0");
    assert!(value.get("enrollment").is_none());
}
