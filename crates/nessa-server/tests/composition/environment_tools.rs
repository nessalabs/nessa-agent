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
    assert_eq!(names, ["environments_list", "run"]);
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
}

#[test]
fn every_refusal_has_its_own_code() {
    let refusals = [
        CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentVersionMismatch),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentBusy),
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
        .await;
    assert_eq!(answer["isError"], true);
}
