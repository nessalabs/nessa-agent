//! Lease frames: their wire shape, the hello another build is recognised by,
//! and the bounds on a frame and the bytes it carries.
use super::{
    decode, encode, fingerprint, read_hello, ArtifactReadRefusal, Cleanup, Collection,
    CollectionRefusal, CommandEnd, Data, EndingCause, FetchOutcome, FetchRefusal, FromEnvironment,
    GrantRefusal, GrantedTerms, Hello, MachineDisk, MachineReport, PublishRefusal, Sandbox,
    StagedArtifact, ToEnvironment, MAX_DATA_BYTES, MAX_FRAME_BYTES, MAX_MACHINE_DISKS,
    MAX_MACHINE_FIGURE, MAX_MACHINE_TEXT_BYTES,
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
            sandbox: Sandbox::HarnessDefault,
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
        ToEnvironment::Collected {
            lease: "l".into(),
            artifact: 3,
            outcome: Collection::Held,
        },
        ToEnvironment::Collected {
            lease: "l".into(),
            artifact: 4,
            outcome: Collection::Refused {
                reason: CollectionRefusal::BudgetExceeded,
            },
        },
        ToEnvironment::GrantCommand {
            lease: "l".into(),
            argv: vec!["cargo".into(), "test".into()],
            cwd: Some("crates/a".into()),
            timeout_ms: 120_000,
        },
        ToEnvironment::Run { lease: "l".into() },
        ToEnvironment::Publish {
            lease: "l".into(),
            request: "call-1".into(),
            path: "build/Nessa.dmg".into(),
            media_type: Some("application/x-apple-diskimage".into()),
        },
        ToEnvironment::Fetch {
            lease: "l".into(),
            request: "call-3".into(),
            destination: "assets/logo.png".into(),
            digest: "ab".repeat(32),
            media_type: "image/png".into(),
            size: 7,
        },
        ToEnvironment::Delivered {
            lease: "l".into(),
            request: "call-3".into(),
        },
        ToEnvironment::Undelivered {
            lease: "l".into(),
            request: "call-3".into(),
        },
        ToEnvironment::ReadArtifact {
            lease: "l".into(),
            artifact: 3,
            offset: 65_536,
            length: 65_536,
        },
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
    let revoked: ToEnvironment = decode(&body(&encode(&ToEnvironment::Revoked).unwrap())).unwrap();
    assert_eq!(revoked, ToEnvironment::Revoked);
    assert_eq!(revoked.lease(), None);
    assert_eq!(
        serde_json::to_value(ToEnvironment::Revoked).unwrap(),
        serde_json::json!({"type": "revoked"})
    );
    let from = [
        FromEnvironment::Granted {
            lease: "l".into(),
            terms: GrantedTerms {
                sandbox: Sandbox::HarnessDefault,
                timeout_ms: None,
                lease_ms: None,
            },
        },
        FromEnvironment::Granted {
            lease: "l".into(),
            terms: GrantedTerms {
                sandbox: Sandbox::None,
                timeout_ms: Some(30_000),
                lease_ms: None,
            },
        },
        FromEnvironment::Refused {
            lease: "l".into(),
            reason: GrantRefusal::NotGranted,
        },
        FromEnvironment::Refused {
            lease: "l".into(),
            reason: GrantRefusal::SandboxUnavailable,
        },
        FromEnvironment::Stopped {
            lease: "l".into(),
            channel: 2,
            cleanup: Cleanup::Confirmed { forced: true },
        },
        FromEnvironment::Granted {
            lease: "l".into(),
            terms: GrantedTerms {
                sandbox: Sandbox::HarnessDefault,
                timeout_ms: None,
                lease_ms: Some(3_600_000),
            },
        },
        FromEnvironment::Ending {
            lease: "l".into(),
            cause: EndingCause::Expired,
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
        FromEnvironment::Published {
            lease: "l".into(),
            artifact: 3,
            request: None,
            file: staged(),
        },
        FromEnvironment::Published {
            lease: "l".into(),
            artifact: 4,
            request: Some("call-1".into()),
            file: staged(),
        },
        FromEnvironment::NotPublished {
            lease: "l".into(),
            request: "call-2".into(),
            reason: PublishRefusal::OutsideWorkspace,
        },
        FromEnvironment::FetchAccepted {
            lease: "l".into(),
            request: "call-3".into(),
            path: "/home/me/.local/share/nessa/environment/inbox/l/1.part".into(),
        },
        FromEnvironment::FetchRefused {
            lease: "l".into(),
            request: "call-4".into(),
            reason: FetchRefusal::Occupied,
        },
        FromEnvironment::Fetched {
            lease: "l".into(),
            request: "call-3".into(),
            outcome: FetchOutcome::Placed {
                path: "/work/assets/logo.png".into(),
            },
        },
        FromEnvironment::Fetched {
            lease: "l".into(),
            request: "call-5".into(),
            outcome: FetchOutcome::Refused {
                reason: FetchRefusal::Mismatch,
            },
        },
        FromEnvironment::Ran {
            lease: "l".into(),
            end: CommandEnd::Exited { code: 1 },
            stdout: Data(b"out".to_vec()),
            stderr: Data(Vec::new()),
            dropped_bytes: 7,
            cleanup: Cleanup::Confirmed { forced: false },
        },
        FromEnvironment::ArtifactRead {
            lease: "l".into(),
            artifact: 3,
            offset: 65_536,
            data: Data(vec![1, 2, 3]),
        },
        FromEnvironment::ArtifactNotRead {
            lease: "l".into(),
            artifact: 3,
            offset: 0,
            reason: ArtifactReadRefusal::NotStaged,
        },
    ];
    for frame in from {
        let decoded: FromEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
        assert_eq!(decoded.lease(), Some("l"));
        assert_eq!(decoded, frame);
    }
}

/// A grant asks for its sandbox by name, and its answer says the terms
/// granted: a harness's carries no timeout, a command's its narrowed one.
#[test]
fn a_grant_names_its_sandbox_and_its_answer_the_granted_terms() {
    let grant = serde_json::to_value(ToEnvironment::Grant {
        lease: "l".into(),
        agent: "claude".into(),
        sandbox: Sandbox::HarnessDefault,
    })
    .unwrap();
    assert_eq!(
        grant,
        serde_json::json!({"type": "grant", "lease": "l", "agent": "claude", "sandbox": "harnessDefault"})
    );
    let harness = serde_json::to_value(FromEnvironment::Granted {
        lease: "l".into(),
        terms: GrantedTerms {
            sandbox: Sandbox::HarnessDefault,
            timeout_ms: None,
            lease_ms: None,
        },
    })
    .unwrap();
    assert_eq!(
        harness,
        serde_json::json!({"type": "granted", "lease": "l", "terms": {"sandbox": "harnessDefault"}})
    );
    let command = serde_json::to_value(FromEnvironment::Granted {
        lease: "l".into(),
        terms: GrantedTerms {
            sandbox: Sandbox::None,
            timeout_ms: Some(5_000),
            lease_ms: None,
        },
    })
    .unwrap();
    assert_eq!(
        command,
        serde_json::json!({"type": "granted", "lease": "l", "terms": {"sandbox": "none", "timeoutMs": 5000}})
    );
    // A harness's lease may be granted a duration, which the environment
    // ends it past, and says so.
    let limited = serde_json::to_value(FromEnvironment::Granted {
        lease: "l".into(),
        terms: GrantedTerms {
            sandbox: Sandbox::HarnessDefault,
            timeout_ms: None,
            lease_ms: Some(60_000),
        },
    })
    .unwrap();
    assert_eq!(
        limited,
        serde_json::json!({"type": "granted", "lease": "l", "terms": {"sandbox": "harnessDefault", "leaseMs": 60000}})
    );
    assert_eq!(
        serde_json::to_value(FromEnvironment::Ending {
            lease: "l".into(),
            cause: EndingCause::Expired,
        })
        .unwrap(),
        serde_json::json!({"type": "ending", "lease": "l", "cause": "expired"})
    );
    // A grant without a sandbox is another build's: it is not read as one.
    assert!(decode::<ToEnvironment>(br#"{"type":"grant","lease":"l","agent":"claude"}"#).is_err());
    assert_eq!(
        serde_json::to_value(GrantRefusal::NotGranted).unwrap(),
        "notGranted"
    );
}

fn staged() -> StagedArtifact {
    StagedArtifact {
        name: "Nessa.dmg".into(),
        media_type: "application/x-apple-diskimage".into(),
        size: 7,
        digest: "ab".repeat(32),
        path: "/home/me/.local/share/nessa/environment/outbox/l/3".into(),
    }
}

/// Gate: a publish says which file and where, never its bytes; the
/// collection answers what became of it, by name.
#[test]
fn a_publish_carries_no_bytes_and_its_collection_is_named() {
    let text = String::from_utf8(
        serde_json::to_vec(&FromEnvironment::Published {
            lease: "l".into(),
            artifact: 3,
            request: None,
            file: staged(),
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        format!(
            r#"{{"type":"published","lease":"l","artifact":3,"request":null,"file":{{"name":"Nessa.dmg","mediaType":"application/x-apple-diskimage","size":7,"digest":"{}","path":"/home/me/.local/share/nessa/environment/outbox/l/3"}}}}"#,
            "ab".repeat(32)
        )
    );
    let text = String::from_utf8(
        serde_json::to_vec(&ToEnvironment::Collected {
            lease: "l".into(),
            artifact: 3,
            outcome: Collection::Refused {
                reason: CollectionRefusal::ChannelUnavailable,
            },
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"collected","lease":"l","artifact":3,"outcome":{"outcome":"refused","reason":"channelUnavailable"}}"#
    );
}

/// Gate: a publish the gateway asks for names its request and the path
/// only; its refusal answers that request by name.
#[test]
fn a_gateway_asked_publish_names_its_request_and_its_refusal_answers_it() {
    let text = String::from_utf8(
        serde_json::to_vec(&ToEnvironment::Publish {
            lease: "l".into(),
            request: "call-1".into(),
            path: "out/report.pdf".into(),
            media_type: None,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"publish","lease":"l","request":"call-1","path":"out/report.pdf","mediaType":null}"#
    );
    let text = String::from_utf8(
        serde_json::to_vec(&FromEnvironment::NotPublished {
            lease: "l".into(),
            request: "call-1".into(),
            reason: PublishRefusal::Busy,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"notPublished","lease":"l","request":"call-1","reason":"busy"}"#
    );
}

/// Gate: a fetch names the file by digest, media type and size and where
/// it goes; its answers name the request and carry no bytes of it, only
/// where the bytes are to be written and what became of it.
#[test]
fn a_fetch_names_its_file_and_its_answers_carry_no_bytes() {
    let text = |frame: &dyn erased::Frame| frame.text();
    assert_eq!(
        text(&ToEnvironment::Fetch {
            lease: "l".into(),
            request: "call-1".into(),
            destination: "a.png".into(),
            digest: "ab".repeat(32),
            media_type: "image/png".into(),
            size: 7,
        }),
        format!(
            r#"{{"type":"fetch","lease":"l","request":"call-1","destination":"a.png","digest":"{}","mediaType":"image/png","size":7}}"#,
            "ab".repeat(32)
        )
    );
    assert_eq!(
        text(&ToEnvironment::Delivered {
            lease: "l".into(),
            request: "call-1".into(),
        }),
        r#"{"type":"delivered","lease":"l","request":"call-1"}"#
    );
    assert_eq!(
        text(&ToEnvironment::Undelivered {
            lease: "l".into(),
            request: "call-1".into(),
        }),
        r#"{"type":"undelivered","lease":"l","request":"call-1"}"#
    );
    assert_eq!(
        text(&FromEnvironment::FetchAccepted {
            lease: "l".into(),
            request: "call-1".into(),
            path: "/in/1.part".into(),
        }),
        r#"{"type":"fetchAccepted","lease":"l","request":"call-1","path":"/in/1.part"}"#
    );
    assert_eq!(
        text(&FromEnvironment::FetchRefused {
            lease: "l".into(),
            request: "call-1".into(),
            reason: FetchRefusal::NoDirectory,
        }),
        r#"{"type":"fetchRefused","lease":"l","request":"call-1","reason":"noDirectory"}"#
    );
    assert_eq!(
        text(&FromEnvironment::Fetched {
            lease: "l".into(),
            request: "call-1".into(),
            outcome: FetchOutcome::AlreadyPlaced {
                path: "/work/a.png".into(),
            },
        }),
        r#"{"type":"fetched","lease":"l","request":"call-1","outcome":{"outcome":"alreadyPlaced","path":"/work/a.png"}}"#
    );
    assert_eq!(
        text(&FromEnvironment::Fetched {
            lease: "l".into(),
            request: "call-1".into(),
            outcome: FetchOutcome::Refused {
                reason: FetchRefusal::LeaseEnded,
            },
        }),
        r#"{"type":"fetched","lease":"l","request":"call-1","outcome":{"outcome":"refused","reason":"leaseEnded"}}"#
    );
}

/// Serialises any frame to its JSON text.
mod erased {
    pub(super) trait Frame {
        fn text(&self) -> String;
    }
    impl<T: serde::Serialize> Frame for T {
        fn text(&self) -> String {
            String::from_utf8(serde_json::to_vec(self).unwrap()).unwrap()
        }
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
    let text = String::from_utf8(
        serde_json::to_vec(&FromEnvironment::Ran {
            lease: "l".into(),
            end: CommandEnd::Signalled { signal: 9 },
            stdout: Data(b"a".to_vec()),
            stderr: Data(Vec::new()),
            dropped_bytes: 0,
            cleanup: Cleanup::NotHeld,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"ran","lease":"l","end":{"outcome":"signalled","signal":9},"stdout":"YQ==","stderr":"","droppedBytes":0,"cleanup":{"outcome":"notHeld"}}"#
    );
    let text = String::from_utf8(
        serde_json::to_vec(&ToEnvironment::GrantCommand {
            lease: "l".into(),
            argv: vec!["ls".into()],
            cwd: None,
            timeout_ms: 1,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        text,
        r#"{"type":"grantCommand","lease":"l","argv":["ls"],"cwd":null,"timeoutMs":1}"#
    );
}

/// The largest answer a command lease can have fits in one frame: both
/// streams' kept bytes are bounded together by one frame's data.
#[test]
fn a_commands_whole_capture_fits_in_one_frame() {
    let half = MAX_DATA_BYTES / 2;
    let frame = FromEnvironment::Ran {
        lease: "l".repeat(64),
        end: CommandEnd::Exited { code: i32::MIN },
        stdout: Data(vec![0xff; half]),
        stderr: Data(vec![0xff; MAX_DATA_BYTES - half]),
        dropped_bytes: u64::MAX,
        cleanup: Cleanup::Uncertain,
    };
    assert!(encode(&frame).is_ok());
    for end in [
        CommandEnd::TimedOut,
        CommandEnd::Stopped,
        CommandEnd::NotStarted,
        CommandEnd::Unknown,
    ] {
        let frame = FromEnvironment::Ran {
            lease: "l".into(),
            end,
            stdout: Data(Vec::new()),
            stderr: Data(Vec::new()),
            dropped_bytes: 0,
            cleanup: Cleanup::NotHeld,
        };
        let decoded: FromEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
        assert_eq!(decoded, frame);
    }
}

/// Gate 4: another protocol's hello is recognised by its type, build and
/// protocol alone, whatever else it carries; one naming no protocol names
/// none any gateway speaks; a first frame that is not a hello is no hello.
#[test]
fn a_hello_is_read_by_its_protocol_whatever_else_it_carries() {
    let hello = read_hello(
        br#"{"type":"hello","build":"9.9.9","protocol":"p","workspace":"/w","extra":[1]}"#,
    );
    assert_eq!(
        hello,
        Some(Hello {
            build: "9.9.9".into(),
            protocol: "p".into(),
            workspace: "/w".into(),
        })
    );
    let before_protocols = read_hello(br#"{"type":"hello","build":"0.1.0","workspace":"/w"}"#);
    assert_eq!(
        before_protocols.map(|hello| hello.protocol),
        Some(String::new())
    );
    assert_eq!(read_hello(br#"{"type":"granted","lease":"l"}"#), None);
    assert_eq!(read_hello(b"SSH-2.0-OpenSSH"), None);
    assert_eq!(read_hello(br#"{"type":"hello"}"#), None);
}

#[test]
fn unknown_fields_and_frames_are_refused() {
    assert!(decode::<ToEnvironment>(br#"{"type":"end","lease":"l","more":1}"#).is_err());
    assert!(decode::<ToEnvironment>(br#"{"type":"exec","lease":"l"}"#).is_err());
    assert!(decode::<FromEnvironment>(br#"{"type":"output","channel":1,"data":""}"#).is_err());
}

/// A read over the lease stream names its artifact and range, and its
/// answer carries the bytes base64, or why not, by name.
#[test]
fn an_artifact_read_names_its_range_and_its_answer_the_bytes_or_why_not() {
    assert_eq!(
        String::from_utf8(
            serde_json::to_vec(&ToEnvironment::ReadArtifact {
                lease: "l".into(),
                artifact: 2,
                offset: 10,
                length: 4,
            })
            .unwrap()
        )
        .unwrap(),
        r#"{"type":"readArtifact","lease":"l","artifact":2,"offset":10,"length":4}"#
    );
    assert_eq!(
        String::from_utf8(
            serde_json::to_vec(&FromEnvironment::ArtifactRead {
                lease: "l".into(),
                artifact: 2,
                offset: 10,
                data: Data(b"abcd".to_vec()),
            })
            .unwrap()
        )
        .unwrap(),
        r#"{"type":"artifactRead","lease":"l","artifact":2,"offset":10,"data":"YWJjZA=="}"#
    );
    for (reason, name) in [
        (ArtifactReadRefusal::NotStaged, "notStaged"),
        (ArtifactReadRefusal::Unreadable, "unreadable"),
        (ArtifactReadRefusal::Busy, "busy"),
    ] {
        assert_eq!(
            serde_json::to_value(FromEnvironment::ArtifactNotRead {
                lease: "l".into(),
                artifact: 2,
                offset: 10,
                reason,
            })
            .unwrap(),
            serde_json::json!({
                "type": "artifactNotRead",
                "lease": "l",
                "artifact": 2,
                "offset": 10,
                "reason": name,
            })
        );
    }
    // The largest range one answer carries fits in a frame.
    let largest = FromEnvironment::ArtifactRead {
        lease: "l".repeat(256),
        artifact: u32::MAX,
        offset: u64::MAX,
        data: Data(vec![0xff; MAX_DATA_BYTES]),
    };
    assert!(encode(&largest).is_ok());
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
        sandbox: Sandbox::HarnessDefault,
    };
    assert!(encode(&oversized).is_err());
}

/// The lease protocol is a fingerprint of the sources that define it: any
/// byte of them, or where one file ends and the next begins, changes it;
/// Windows line endings do not.
#[test]
fn a_protocols_fingerprint_follows_every_byte_of_its_sources_but_line_endings() {
    let named = fingerprint(&[b"frames", b"meaning"]);
    assert!(named.iter().all(u8::is_ascii_hexdigit));
    assert_eq!(fingerprint(&[b"frames", b"meaning"]), named);
    assert_ne!(fingerprint(&[b"frames", b"meaninG"]), named);
    assert_ne!(fingerprint(&[b"frame", b"smeaning"]), named);
    assert_ne!(fingerprint(&[b"frames"]), fingerprint(&[b"frames", b""]));
    assert_eq!(fingerprint(&[b"a\r\nb\r\n"]), fingerprint(&[b"a\nb\n"]));
}

/// A machine report is sent outside any lease, and its text and figures are
/// bounded: a report past them is not one to show.
#[test]
fn a_machine_report_names_no_lease_and_is_bounded() {
    let report = MachineReport {
        hostname: "build-box".into(),
        os: "linux".into(),
        os_version: "24.04".into(),
        arch: "x86_64".into(),
        cpus: 8,
        cpu_permille: 610,
        memory_total_bytes: 32,
        memory_used_bytes: 8,
        disk_total_bytes: 100,
        disk_used_bytes: 40,
        disks: vec![MachineDisk {
            mount: "/".into(),
            total_bytes: 100,
            used_bytes: 40,
        }],
    };
    let frame = FromEnvironment::Machine {
        report: report.clone(),
    };
    let decoded: FromEnvironment = decode(&body(&encode(&frame).unwrap())).unwrap();
    assert_eq!(decoded.lease(), None);
    assert_eq!(decoded, frame);
    assert!(report.is_bounded());
    let long = "é".repeat(MAX_MACHINE_TEXT_BYTES);
    let cut = MachineReport::bounded(&long);
    assert!(cut.len() <= MAX_MACHINE_TEXT_BYTES && long.starts_with(&cut));
    for over in [
        MachineReport {
            hostname: long,
            ..report.clone()
        },
        MachineReport {
            cpu_permille: 1_001,
            ..report.clone()
        },
        MachineReport {
            memory_used_bytes: 33,
            ..report.clone()
        },
        MachineReport {
            disk_used_bytes: 101,
            ..report.clone()
        },
        MachineReport {
            memory_total_bytes: MAX_MACHINE_FIGURE + 1,
            ..report.clone()
        },
        MachineReport {
            disk_total_bytes: MAX_MACHINE_FIGURE + 1,
            ..report.clone()
        },
        MachineReport {
            disks: vec![report.disks[0].clone(); MAX_MACHINE_DISKS + 1],
            ..report.clone()
        },
        MachineReport {
            disks: vec![MachineDisk {
                used_bytes: 101,
                ..report.disks[0].clone()
            }],
            ..report.clone()
        },
        MachineReport {
            disks: vec![MachineDisk {
                mount: "m".repeat(MAX_MACHINE_TEXT_BYTES + 1),
                ..report.disks[0].clone()
            }],
            ..report
        },
    ] {
        assert!(!over.is_bounded());
    }
}
