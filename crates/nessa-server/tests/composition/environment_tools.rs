//! The agent's environment tools as the harness sees them: what they list,
//! how `run`'s arguments are read, and what each answer says.
use super::*;
use crate::conversation::application::CommandResult;
use nessa_sdk::domain::agent_execution::leases::LeaseId;

fn tools() -> Arc<EnvironmentTools> {
    EnvironmentTools::new(&CommandPolicy::new(["devbox".to_owned()], None, Vec::new()))
}

#[test]
fn the_tools_are_named_as_a_model_api_accepts_and_run_is_marked_destructive() {
    let listed = tools().tools();
    let names: Vec<&str> = listed
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "environments_list",
            "run",
            "artifacts_publish",
            "artifacts_fetch"
        ]
    );
    for name in names.iter().chain([SERVER_NAME].iter()) {
        assert!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "{name}"
        );
    }
    assert_eq!(listed[0]["annotations"]["readOnlyHint"], true);
    assert_eq!(listed[1]["annotations"]["destructiveHint"], true);
    assert_eq!(
        listed[1]["inputSchema"]["required"],
        json!(["environment", "argv"])
    );
    assert_eq!(listed[2]["annotations"]["destructiveHint"], false);
    assert_eq!(
        listed[2]["inputSchema"]["required"],
        json!(["environment", "path"])
    );
    assert_eq!(listed[3]["annotations"]["destructiveHint"], false);
    assert_eq!(
        listed[3]["inputSchema"]["required"],
        json!(["environment", "digest", "mediaType", "size", "destination"])
    );
    assert_eq!(listed[3]["inputSchema"]["additionalProperties"], false);
}

#[test]
fn artifacts_publish_reads_its_arguments_and_refuses_anything_else() {
    let call = publish_call(
        json!({"environment": "devbox", "path": "out/report.pdf"}),
        "c".into(),
    )
    .unwrap();
    assert_eq!(call.environment, "devbox");
    assert_eq!(call.path, "out/report.pdf");
    assert_eq!(call.media_type, None);
    assert_eq!(call.call, "c");
    let call = publish_call(
        json!({"environment": "here", "path": "/srv/work/a.png", "mediaType": "image/png"}),
        "c".into(),
    )
    .unwrap();
    assert_eq!(call.media_type.as_deref(), Some("image/png"));
    for refused in [
        json!({"path": "a.png"}),
        json!({"environment": "devbox"}),
        json!({"environment": "devbox", "path": 3}),
        json!({"environment": "devbox", "path": "a.png", "media_type": "image/png"}),
        json!({"environment": "devbox", "path": "a.png", "bytes": "AAAA"}),
    ] {
        assert!(
            publish_call(refused.clone(), "c".into()).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn a_publish_answer_mirrors_the_hosts_and_is_an_error_unless_held() {
    let held = published(&PublishAnswer::Kept {
        already_held: false,
        name: "report.pdf".into(),
        digest: "ab".repeat(32),
        size: 20,
    });
    assert_eq!(held["isError"], false);
    assert_eq!(
        held["structuredContent"],
        json!({"outcome": "held", "digest": "ab".repeat(32), "size": 20, "name": "report.pdf"})
    );
    let again = published(&PublishAnswer::Kept {
        already_held: true,
        name: "report.pdf".into(),
        digest: "ab".repeat(32),
        size: 20,
    });
    assert_eq!(again["isError"], false);
    assert_eq!(again["structuredContent"]["outcome"], "alreadyHeld");
    for (refusal, code) in [
        (
            PublishCallRefusal::EnvironmentNotLeased,
            "environment_not_leased",
        ),
        (PublishCallRefusal::NoTurn, "no_turn"),
        (PublishCallRefusal::DuplicateRequest, "duplicate_request"),
        (PublishCallRefusal::NotAllowed, "not_allowed"),
        (
            PublishCallRefusal::PublishUnavailable,
            "publish_unavailable",
        ),
        (PublishCallRefusal::TimedOut, "timed_out"),
        (
            PublishCallRefusal::NotPublished(PublishRefusal::OutsideWorkspace),
            "outside_workspace",
        ),
        (
            PublishCallRefusal::NotKept(CollectionRefusal::BudgetExceeded),
            "budget_exceeded",
        ),
    ] {
        let answer = published(&PublishAnswer::Refused(refusal));
        assert_eq!(answer["isError"], true, "{code}");
        assert_eq!(answer["structuredContent"]["refused"], code);
        assert!(answer["structuredContent"]["message"].is_string());
    }
}

#[test]
fn artifacts_fetch_reads_its_arguments_and_refuses_anything_else() {
    let digest = "ab".repeat(32);
    let call = fetch_call(
        json!({
            "environment": "devbox",
            "digest": digest,
            "mediaType": "image/png",
            "size": 20,
            "destination": "assets/cat.png",
        }),
        "c".into(),
    )
    .unwrap();
    assert_eq!(call.environment, "devbox");
    assert_eq!(call.digest, digest);
    assert_eq!(call.media_type, "image/png");
    assert_eq!(call.size, 20);
    assert_eq!(call.destination, "assets/cat.png");
    assert_eq!(call.call, "c");
    let whole = json!({
        "environment": "devbox",
        "digest": digest,
        "mediaType": "image/png",
        "size": 20,
        "destination": "a.png",
    });
    let mut refused = Vec::new();
    for field in ["environment", "digest", "mediaType", "size", "destination"] {
        let mut missing = whole.clone();
        missing.as_object_mut().unwrap().remove(field);
        refused.push(missing);
    }
    for (field, value) in [
        ("size", json!(-1)),
        ("size", json!("20")),
        ("digest", json!(3)),
        ("media_type", json!("image/png")),
        ("bytes", json!("AAAA")),
        ("replace", json!(true)),
    ] {
        let mut wrong = whole.clone();
        wrong.as_object_mut().unwrap().insert(field.into(), value);
        refused.push(wrong);
    }
    for refused in refused {
        assert!(
            fetch_call(refused.clone(), "c".into()).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn a_fetch_answer_says_where_the_file_is_and_is_an_error_unless_placed_and_recorded() {
    let placed_as = |already_placed, recorded| {
        fetched(&FetchAnswer::Placed {
            already_placed,
            path: "/work/a.png".into(),
            digest: "ab".repeat(32),
            size: 20,
            recorded,
        })
    };
    let placed = placed_as(false, true);
    assert_eq!(placed["isError"], false);
    assert_eq!(
        placed["structuredContent"],
        json!({"outcome": "placed", "path": "/work/a.png", "digest": "ab".repeat(32), "size": 20, "recorded": true})
    );
    let again = placed_as(true, true);
    assert_eq!(again["isError"], false);
    assert_eq!(again["structuredContent"]["outcome"], "alreadyPlaced");
    // Placed, but its end is not in the records: said, and an error, as a
    // command whose end could not be saved is.
    for already_placed in [false, true] {
        let unrecorded = placed_as(already_placed, false);
        assert_eq!(unrecorded["isError"], true);
        assert_eq!(unrecorded["structuredContent"]["recorded"], false);
        assert_eq!(unrecorded["structuredContent"]["path"], "/work/a.png");
    }
    let refusals = [
        (FetchCallRefusal::NoTurn, "no_turn"),
        (
            FetchCallRefusal::EnvironmentNotLeased,
            "environment_not_leased",
        ),
        (FetchCallRefusal::DuplicateRequest, "duplicate_request"),
        (FetchCallRefusal::NotAllowed, "not_allowed"),
        (FetchCallRefusal::FetchUnavailable, "fetch_unavailable"),
        (FetchCallRefusal::Invalid, "invalid"),
        (FetchCallRefusal::NotHeld, "not_held"),
        (FetchCallRefusal::BudgetExceeded, "budget_exceeded"),
        (FetchCallRefusal::Busy, "busy"),
        (FetchCallRefusal::AuditUnavailable, "audit_unavailable"),
        (FetchCallRefusal::SourceUnavailable, "source_unavailable"),
        (FetchCallRefusal::ChannelUnavailable, "channel_unavailable"),
        (
            FetchCallRefusal::Unanswered { delivered: true },
            "unanswered",
        ),
        (
            FetchCallRefusal::Unanswered { delivered: false },
            "unanswered",
        ),
        (FetchCallRefusal::TimedOut { delivered: true }, "timed_out"),
        (FetchCallRefusal::TimedOut { delivered: false }, "timed_out"),
        (FetchCallRefusal::Cancelled { delivered: true }, "cancelled"),
        (
            FetchCallRefusal::Cancelled { delivered: false },
            "cancelled",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::Unavailable),
            "fetch_unavailable",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::OutsideWorkspace),
            "outside_workspace",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::NoDirectory),
            "no_directory",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::Occupied),
            "occupied",
        ),
        (FetchCallRefusal::NotPlaced(FetchRefusal::Size), "size"),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::Invalid),
            "invalid",
        ),
        (FetchCallRefusal::NotPlaced(FetchRefusal::Busy), "busy"),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::Mismatch),
            "mismatch",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::PlacingFailed),
            "placing_failed",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::AuditUnavailable),
            "audit_unavailable",
        ),
        (
            FetchCallRefusal::NotPlaced(FetchRefusal::LeaseEnded),
            "lease_ended",
        ),
    ];
    for (refusal, code) in refusals {
        let answer = fetched(&FetchAnswer::Refused(refusal));
        assert_eq!(answer["isError"], true, "{code}");
        assert_eq!(answer["structuredContent"]["refused"], code);
        assert_eq!(fetch_refusal_code(refusal), code);
        assert!(answer["structuredContent"]["message"].is_string());
        assert!(answer["structuredContent"].get("recorded").is_none());
        // Its end not in the records: the same refusal, said unrecorded.
        let unrecorded = fetched(&FetchAnswer::RefusedUnrecorded(refusal));
        assert_eq!(unrecorded["isError"], true, "{code}");
        assert_eq!(unrecorded["structuredContent"]["refused"], code);
        assert_eq!(unrecorded["structuredContent"]["recorded"], false);
    }
}

/// With no answer, a fetch says whether every byte was delivered, and only
/// one that was says the file may yet be placed.
#[test]
fn a_fetch_with_no_answer_says_it_may_be_placed_only_once_every_byte_was_delivered() {
    for delivered in [false, true] {
        for refusal in [
            FetchCallRefusal::Unanswered { delivered },
            FetchCallRefusal::TimedOut { delivered },
            FetchCallRefusal::Cancelled { delivered },
        ] {
            let said = &fetched(&FetchAnswer::Refused(refusal))["structuredContent"];
            assert_eq!(said["delivered"], delivered, "{refusal:?}");
            let message = said["message"].as_str().unwrap();
            assert_eq!(message.contains("may"), delivered, "{message}");
            assert_eq!(
                message.contains("nothing was placed"),
                !delivered,
                "{message}"
            );
        }
    }
    // An answer that says what happened carries no such word.
    let said = &fetched(&FetchAnswer::Refused(FetchCallRefusal::NotHeld))["structuredContent"];
    assert!(said.get("delivered").is_none());
}

#[test]
fn another_policy_is_another_configuration() {
    let tools = tools();
    let other = EnvironmentTools::new(&CommandPolicy::new(
        ["devbox".to_owned()],
        None,
        vec!["rm".to_owned()],
    ));
    assert_ne!(tools.configuration(), other.configuration());
    assert_eq!(tools.configuration()[0], TOOLS_REVISION);
}

#[test]
fn run_reads_its_arguments_with_their_defaults_and_refuses_anything_else() {
    let call = command_call(
        json!({"environment": "devbox", "argv": ["ls", "-l"]}),
        "c".into(),
    )
    .unwrap();
    assert_eq!(call.environment, "devbox");
    assert_eq!(call.argv, ["ls", "-l"]);
    assert_eq!(call.cwd, None);
    assert_eq!(call.timeout_ms, CommandWork::DEFAULT_TIMEOUT_MS);
    assert_eq!(call.sandbox, SandboxProfile::None);
    assert_eq!(call.call, "c");
    let call = command_call(
        json!({"environment": "devbox", "argv": ["ls"], "cwd": "sub", "timeoutMs": 5, "sandbox": "harness-default"}),
        "c".into(),
    )
    .unwrap();
    assert_eq!(call.cwd.as_deref(), Some("sub"));
    assert_eq!(call.timeout_ms, 5);
    assert_eq!(call.sandbox, SandboxProfile::HarnessDefault);
    for refused in [
        json!({"argv": ["ls"]}),
        json!({"environment": "devbox"}),
        json!({"environment": "devbox", "argv": "ls"}),
        json!({"environment": "devbox", "argv": ["ls"], "shell": true}),
        json!({"environment": "devbox", "argv": ["ls"], "sandbox": "container"}),
        json!({"environment": "devbox", "argv": ["ls"], "timeoutMs": -1}),
    ] {
        assert!(
            command_call(refused.clone(), "c".into()).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn an_answer_says_how_the_command_ended_or_why_it_was_refused() {
    let refused = answered(&CommandAnswer::Refused(CommandRefusal::Environment(
        LeaseRefusal::EnvironmentUnreachable,
    )));
    assert_eq!(refused["isError"], true);
    assert_eq!(
        refused["structuredContent"]["refused"],
        "environment_unreachable"
    );
    let ran_as = |exit, cleanup, recorded| {
        answered(&CommandAnswer::Ran {
            lease: LeaseId::new("lease-1").unwrap(),
            result: CommandResult {
                exit,
                stdout: b"out".to_vec(),
                stderr: vec![0xff],
                dropped_bytes: 4,
                cleanup,
                published: Vec::new(),
            },
            recorded,
        })
    };
    let ran = |exit, cleanup| ran_as(exit, cleanup, true);
    let exited = ran(
        CommandExit::Exited { code: 0 },
        Some(LeaseCleanup::Confirmed { forced: false }),
    );
    assert_eq!(exited["isError"], false);
    assert_eq!(
        exited["structuredContent"],
        json!({
            "lease": "lease-1",
            "exit": {"kind": "exited", "code": 0},
            "stdout": "out",
            "stderr": "\u{fffd}",
            "droppedBytes": 4,
            "cleanup": "confirmed",
            "recorded": true,
            "published": [],
        })
    );
    assert_eq!(exited["content"][0]["type"], "text");
    let stopped = ran(
        CommandExit::Stopped {
            cause: LeaseEndCause::Closed,
        },
        None,
    );
    assert_eq!(stopped["isError"], true);
    assert_eq!(
        stopped["structuredContent"]["exit"],
        json!({"kind": "stopped", "cause": "closed"})
    );
    assert_eq!(stopped["structuredContent"]["cleanup"], "uncertain");
    assert_eq!(ran(CommandExit::Exited { code: 1 }, None)["isError"], true);
    // A command whose end the records do not hold is not answered as a
    // success, though what it printed is still said.
    let unrecorded = ran_as(
        CommandExit::Exited { code: 0 },
        Some(LeaseCleanup::Confirmed { forced: false }),
        false,
    );
    assert_eq!(unrecorded["isError"], true);
    assert_eq!(unrecorded["structuredContent"]["recorded"], false);
    assert_eq!(unrecorded["structuredContent"]["stdout"], "out");
    // Nor one whose cleanup the host could not confirm, or said was of
    // nothing held.
    assert_eq!(ran(CommandExit::Exited { code: 0 }, None)["isError"], true);
    let not_held = ran(CommandExit::Exited { code: 0 }, Some(LeaseCleanup::NotHeld));
    assert_eq!(not_held["isError"], true);
    assert_eq!(not_held["structuredContent"]["cleanup"], "not_held");
}

#[test]
fn every_refusal_has_its_own_code() {
    let refusals = [
        CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentVersionMismatch),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentBusy),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentPlatformUnsupported),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentInstallFailed),
        CommandRefusal::Environment(LeaseRefusal::AgentUnavailable),
        CommandRefusal::EnvironmentNotGranted,
        CommandRefusal::CommandDenied,
        CommandRefusal::BudgetExceeded,
        CommandRefusal::CommandsUnavailable,
    ];
    let codes: std::collections::BTreeSet<&str> = refusals
        .iter()
        .map(|refusal| refusal_code(*refusal))
        .collect();
    assert_eq!(codes.len(), refusals.len());
}

#[tokio::test]
async fn a_call_before_the_service_is_attached_or_for_no_conversation_is_an_error() {
    let (_stop, stop) = watch::channel(false);
    let answer = tools()
        .call(
            &SessionId::new("5c0a9f2e-1d3b-4c7a-8e6f-2b9d4a1c7e30").unwrap(),
            "c".into(),
            "environments_list".into(),
            json!({}),
            stop,
        )
        .await
        .result;
    assert_eq!(answer["isError"], true);
}
