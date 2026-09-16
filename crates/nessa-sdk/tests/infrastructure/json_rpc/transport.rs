//! Framing and exact outbound preflight through the shared JSON-RPC transport.
use super::*;
use crate::infrastructure::json_rpc::RpcId;
use serde_json::json;
#[cfg(unix)]
use std::process::Stdio;
use tokio::io::duplex;

#[cfg(unix)]
#[tokio::test]
async fn provider_pipe_probe_reads_flushed_bytes_before_reactor_notification() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("flushed");
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args([
            "-c",
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":17,\"result\":null}'; : > \"$1\"; sleep 30",
            "probe",
            marker.to_str().unwrap(),
        ])
        .stdout(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut reader = Reader::new(child.stdout.take().unwrap(), 256);
    while !marker.exists() {
        tokio::task::yield_now().await;
    }

    assert!(reader.read_ready_os_bytes().unwrap());
    assert_eq!(reader.next().await.unwrap().id, Some(RpcId::Number(17)));
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn fragmented_utf8_and_cancelled_reads_preserve_frame_boundaries() {
    let (mut writer, input) = duplex(256);
    let mut reader = Reader::new(input, 256);
    writer
        .write_all(b"\n \r\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"\xce")
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), reader.next())
            .await
            .is_err()
    );
    writer
        .write_all(b"\xb1\"}\r\n{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":null}\n")
        .await
        .unwrap();
    let first = reader.next().await.unwrap();
    assert_eq!(first.id, Some(RpcId::Number(1)));
    assert_eq!(first.result, Some(json!("α")));
    let second = reader.next().await.unwrap();
    assert_eq!(second.id, Some(RpcId::Number(2)));
    assert_eq!(second.result, Some(Value::Null));
}

#[tokio::test]
async fn framing_enforces_exact_limits_and_rejects_truncated_input() {
    let frame = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}";
    let mut bytes = frame.to_vec();
    bytes.push(b'\n');
    assert!(Reader::new(bytes.as_slice(), frame.len())
        .next()
        .await
        .is_ok());
    assert!(matches!(
        Reader::new(bytes.as_slice(), frame.len() - 1).next().await,
        Err(AgentError::Protocol(_))
    ));
    assert!(matches!(
        Reader::new(frame.as_slice(), 256).next().await,
        Err(AgentError::Protocol(_))
    ));
    assert!(matches!(
        Reader::new(b"".as_slice(), 256).next().await,
        Err(AgentError::Transport(_))
    ));
}

#[tokio::test]
async fn a_valid_frame_is_delivered_before_a_later_oversize_failure() {
    let bytes = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\n{}",
        "x".repeat(128)
    );
    let mut reader = Reader::new(bytes.as_bytes(), 64);
    assert_eq!(reader.next().await.unwrap().id, Some(RpcId::Number(1)));
    assert!(matches!(reader.next().await, Err(AgentError::Protocol(_))));
}

#[tokio::test]
async fn writes_are_bounded_and_round_trip_without_a_provider() {
    let (mut writer, input) = duplex(128);
    let mut reader = Reader::new(input, 128);
    let value = json!({"jsonrpc":"2.0","id":5,"result":{}});
    assert!(matches!(
        encode(value.clone(), 1),
        Err(AgentError::InvalidInput(_))
    ));
    send_encoded(
        &mut writer,
        &encode(value.clone(), 128).unwrap(),
        Duration::from_secs(1),
        None,
    )
    .await
    .unwrap();
    assert_eq!(reader.next().await.unwrap().id, Some(RpcId::Number(5)));
    let (mut stalled, _unread) = duplex(1);
    assert_eq!(
        send_encoded(
            &mut stalled,
            &encode(value, 128).unwrap(),
            Duration::from_millis(10),
            None
        )
        .await,
        Err(AgentError::Deadline)
    );
}

#[tokio::test]
async fn outbound_preflight_counts_utf8_and_json_escapes_at_the_exact_byte_boundary() {
    let value =
        json!({"jsonrpc":"2.0", "id":8, "method":"session/prompt", "params":{"text":"é\n\u{0}\""}});
    let expected = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        encode(value.clone(), expected.len() - 1),
        Err(AgentError::InvalidInput(_))
    ));
    let encoded = encode(value, expected.len()).unwrap();
    assert_eq!(&encoded[..expected.len()], expected);
    assert_eq!(encoded.last(), Some(&b'\n'));
    let mut output = Vec::new();
    send_encoded(&mut output, &encoded, Duration::from_secs(1), None)
        .await
        .unwrap();
    assert_eq!(output, encoded);
}

#[tokio::test]
async fn duplicate_keys_are_rejected_at_every_depth_before_dispatch() {
    for object in [
        r#"{"currentModeId":"bypassPermissions","currentModeId":"default"}"#,
        r#"{"currentModeId":"default","currentModeId":"bypassPermissions"}"#,
        r#"{"currentModeId":"default","currentModeId":"default"}"#,
        r#"{"currentModeId":"default","currentMode\u0049d":"default"}"#,
        r#"{"options":[{"id":"a","id":"b"}]}"#,
        r#"{"nested":{"deep":{"x":1,"x":2}}}"#,
    ] {
        let frame = format!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{object}}}\n"#)
            .replace("\\n", "\n");
        assert!(
            matches!(
                Reader::new(frame.as_bytes(), 4096).next().await,
                Err(AgentError::Protocol(_))
            ),
            "{object}"
        );
    }
    let frame = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"a\":{\"id\":1},\"b\":{\"id\":2}}}\n";
    assert!(Reader::new(frame.as_slice(), 4096).next().await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn cancellation_writes_share_one_absolute_grace_under_backpressure() {
    let (mut output, _unread) = duplex(1);
    let frame = encode(
        json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId":"context"}}),
        1024,
    )
    .unwrap();
    let began = tokio::time::Instant::now();
    let deadline = began + Duration::from_millis(20);
    assert_eq!(
        send_encoded(&mut output, &frame, Duration::from_secs(1), Some(deadline)).await,
        Err(AgentError::Deadline)
    );
    let first = tokio::time::Instant::now();
    assert!(first - began >= Duration::from_millis(20));
    assert!(first - began <= Duration::from_millis(21));
    assert_eq!(
        send_encoded(&mut output, &frame, Duration::from_secs(1), Some(deadline)).await,
        Err(AgentError::Deadline)
    );
    assert_eq!(
        tokio::time::Instant::now(),
        first,
        "fallback must not restart shutdown grace"
    );
}

#[tokio::test(start_paused = true)]
async fn cancellation_write_transport_bound_remains_earlier_than_long_shutdown_grace() {
    let (mut output, _unread) = duplex(1);
    let began = tokio::time::Instant::now();
    let deadline = began + Duration::from_secs(5);
    assert_eq!(
        send_encoded(
            &mut output,
            b"session/cancel",
            Duration::from_secs(1),
            Some(deadline)
        )
        .await,
        Err(AgentError::Deadline)
    );
    let now = tokio::time::Instant::now();
    assert!(now < deadline);
    assert!(now - began >= Duration::from_secs(1));
    assert!(now - began <= Duration::from_millis(1001));
}
