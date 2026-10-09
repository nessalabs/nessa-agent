//! Lease frames: their wire shape, the hello another build is recognised by,
//! and the bounds on a frame and the bytes it carries.
use super::{
    decode, encode, read_hello, Cleanup, Data, FromEnvironment, Hello, ToEnvironment,
    MAX_DATA_BYTES, MAX_FRAME_BYTES,
};
use crate::pairing::FrameReader;
use std::collections::BTreeMap;

fn body(bytes: &[u8]) -> Vec<u8> {
    let mut reader = FrameReader::new(MAX_FRAME_BYTES);
    let buffer = reader.unfilled().unwrap();
    buffer.copy_from_slice(&bytes[..4]);
    assert!(reader.filled(4).unwrap().is_none());
    let rest = &bytes[4..];
    let buffer = reader.unfilled().unwrap();
    buffer.copy_from_slice(rest);
    reader.filled(rest.len()).unwrap().unwrap()
}

#[test]
fn every_frame_round_trips_and_names_its_lease() {
    let to = [
        ToEnvironment::Grant {
            lease: "l".into(),
            agent: "claude".into(),
        },
        ToEnvironment::Start {
            lease: "l".into(),
            channel: 1,
            environment: BTreeMap::from([("ANTHROPIC_MODEL".into(), "m".into())]),
        },
        ToEnvironment::Input {
            lease: "l".into(),
            channel: 1,
            data: Data(vec![0, 1, 255]),
        },
        ToEnvironment::InputClosed {
            lease: "l".into(),
            channel: 1,
        },
        ToEnvironment::Stop {
            lease: "l".into(),
            channel: 1,
            grace_ms: 10,
            kill_ms: 20,
        },
        ToEnvironment::End { lease: "l".into() },
        ToEnvironment::Account { lease: "l".into() },
    ];
    for frame in to {
        let decoded: ToEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
        assert_eq!(decoded.lease(), Some("l"));
        assert_eq!(decoded, frame);
    }
    let keepalive: ToEnvironment =
        decode(&body(&encode(&ToEnvironment::Keepalive).unwrap())).unwrap();
    assert_eq!(keepalive, ToEnvironment::Keepalive);
    assert_eq!(keepalive.lease(), None);
    let from = [
        FromEnvironment::Stopped {
            lease: "l".into(),
            channel: 2,
            cleanup: Cleanup::Confirmed { forced: true },
        },
        FromEnvironment::Ended {
            lease: "l".into(),
            cleanup: Cleanup::NotHeld,
        },
        FromEnvironment::Accounted {
            lease: "l".into(),
            cleanup: Cleanup::Uncertain,
        },
        FromEnvironment::Output {
            lease: "l".into(),
            channel: 2,
            data: Data(b"{}\n".to_vec()),
        },
    ];
    for frame in from {
        let decoded: FromEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
        assert_eq!(decoded.lease(), Some("l"));
        assert_eq!(decoded, frame);
    }
}

#[test]
fn the_wire_names_are_camel_case() {
    let text = String::from_utf8(
        serde_json::to_vec(&ToEnvironment::Stop {
            lease: "l".into(),
            channel: 3,
            grace_ms: 1,
            kill_ms: 2,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"stop","lease":"l","channel":3,"graceMs":1,"killMs":2}"#
    );
    let text = String::from_utf8(
        serde_json::to_vec(&FromEnvironment::Ended {
            lease: "l".into(),
            cleanup: Cleanup::Confirmed { forced: false },
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"ended","lease":"l","cleanup":{"outcome":"confirmed","forced":false}}"#
    );
}

/// Gate 4: another build's hello is recognised by its type and build alone,
/// whatever else it carries; a first frame that is not a hello is no hello.
#[test]
fn a_hello_is_read_by_its_build_whatever_else_it_carries() {
    let hello = read_hello(br#"{"type":"hello","build":"9.9.9","workspace":"/w","extra":[1]}"#);
    assert_eq!(
        hello,
        Some(Hello {
            build: "9.9.9".into(),
            workspace: "/w".into(),
        })
    );
    assert_eq!(read_hello(br#"{"type":"granted","lease":"l"}"#), None);
    assert_eq!(read_hello(b"SSH-2.0-OpenSSH"), None);
    assert_eq!(read_hello(br#"{"type":"hello"}"#), None);
}

#[test]
fn unknown_fields_and_frames_are_refused() {
    assert!(decode::<ToEnvironment>(br#"{"type":"end","lease":"l","more":1}"#).is_err());
    assert!(decode::<ToEnvironment>(br#"{"type":"run","lease":"l"}"#).is_err());
    assert!(decode::<FromEnvironment>(br#"{"type":"output","channel":1,"data":""}"#).is_err());
}

#[test]
fn data_and_frames_are_bounded() {
    let largest = Data(vec![7; MAX_DATA_BYTES]);
    let frame = FromEnvironment::Output {
        lease: "l".into(),
        channel: 1,
        data: largest.clone(),
    };
    let decoded: FromEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
    assert_eq!(decoded, frame);
    let over = serde_json::to_vec(&FromEnvironment::Output {
        lease: "l".into(),
        channel: 1,
        data: Data(vec![7; MAX_DATA_BYTES + 1]),
    })
    .unwrap();
    assert!(decode::<FromEnvironment>(&over).is_err());
    let oversized = ToEnvironment::Grant {
        lease: "l".repeat(MAX_FRAME_BYTES),
        agent: "claude".into(),
    };
    assert!(encode(&oversized).is_err());
}
