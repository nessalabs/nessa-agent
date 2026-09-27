use super::*;
use crate::shell::application::{Audit, Evidence, Runner, Task};

/// The typed stop behind a boxed error, so assertions read fields not prose.
fn unclean_stop(error: &Error) -> &UncleanStop {
    error
        .downcast_ref::<UncleanStop>()
        .expect("an unclean stop is reported as one")
}
fn active(completion: Completion) -> (Option<Active>, watch::Receiver<Option<StopCause>>) {
    let (stop, observed) = watch::channel(None);
    let task = tokio::spawn(async move { completion });
    (
        Some(Active {
            id: json!(1),
            stop,
            task,
        }),
        observed,
    )
}

#[tokio::test]
async fn a_command_that_ends_at_eof_keeps_its_cleanup_and_audit_outcome() {
    // The reply is discarded here — nobody is reading it — so these two facts
    // would be the ones to disappear with it.
    let (mut audit_failed, observed) = active(Completion {
        response: json!({}),
        cleanup_verified: true,
        audit_error: Some("completion audit failed".into()),
    });
    let error = finish(&mut audit_failed, StopCause::ClientClosed)
        .await
        .expect_err("a final audit failure is not a clean stop");
    let unclean = unclean_stop(&error);
    assert_eq!(
        unclean.audit_error.as_deref(),
        Some("completion audit failed")
    );
    assert!(!unclean.cleanup_unverified);
    // The command was stopped through its own cause, never aborted.
    assert_eq!(*observed.borrow(), Some(StopCause::ClientClosed));
    assert!(audit_failed.is_none());

    let (mut unverified, _observed) = active(Completion {
        response: json!({}),
        cleanup_verified: false,
        audit_error: None,
    });
    let error = finish(&mut unverified, StopCause::HostShutdown)
        .await
        .expect_err("unverified cleanup is not a clean stop");
    assert!(error
        .to_string()
        .contains("could not verify process cleanup"));

    // Both facts hold, so both are named; reporting one would drop the other.
    let (mut both, _observed) = active(Completion {
        response: json!({}),
        cleanup_verified: false,
        audit_error: Some("completion audit failed".into()),
    });
    let error = finish(&mut both, StopCause::HostShutdown)
        .await
        .expect_err("neither fact is a clean stop");
    let error = error.to_string();
    assert!(
        error.contains("could not verify process cleanup"),
        "{error}"
    );
    assert!(error.contains("audit was not accepted"), "{error}");

    let (mut clean, _observed) = active(Completion {
        response: json!({}),
        cleanup_verified: true,
        audit_error: None,
    });
    assert!(finish(&mut clean, StopCause::HostShutdown).await.is_ok());

    let mut idle = None;
    assert!(finish(&mut idle, StopCause::HostShutdown).await.is_ok());
}

/// Never runs anything. Admission was refused, so being asked to is the failure.
struct UnusedRunner;
impl Runner for UnusedRunner {
    fn run<'a>(
        &'a self,
        _: &'a RunRequest,
        _: watch::Receiver<Option<StopCause>>,
        _: &'a dyn Audit,
    ) -> Task<'a, RunResult> {
        unreachable!("admission was refused, so nothing should be started")
    }
}

/// Refuses to record anything.
struct RefusingAudit;
impl Audit for RefusingAudit {
    fn record<'a>(&'a self, _: &'a RunRequest, _: Evidence) -> Task<'a, Result<(), ()>> {
        Box::pin(async { Err(()) })
    }
}

#[tokio::test]
async fn an_admission_audit_that_is_refused_is_not_a_clean_stop() {
    // Nothing is started, so there is no process cleanup to miss and `main`'s
    // scope check has nothing to catch. The refusal itself is an audit delivery
    // that was never acknowledged, and once the client has gone the exit code is
    // the only thing left to carry it.
    //
    // There is no interleaving to arrange here: admission never reads the stop
    // receiver, so when the stop arrives relative to it does not change the
    // outcome. What this drives is the real `ShellService` and the real mapping
    // into `Completion`, rather than a `Completion` built by hand.
    let service = Arc::new(ShellService::new(
        Arc::new(UnusedRunner),
        Arc::new(RefusingAudit),
    ));
    let request = RunRequest {
        id: "command".into(),
        invocation: ToolInvocation::provider(String::from("owner"), ToolRequestId::Signed(1))
            .unwrap(),
        command: ShellCommand::new(String::from("true"), 1).unwrap(),
        cwd: std::env::temp_dir(),
    };
    let (stop, receive) = watch::channel(None);
    let task = tokio::spawn(async move {
        let outcome = service.run(&request, receive).await;
        completion(json!(1), "command", outcome)
    });
    let mut active = Some(Active {
        id: json!(1),
        stop,
        task,
    });

    let error = finish(&mut active, StopCause::ClientClosed)
        .await
        .expect_err("an unacknowledged admission audit is not a clean stop");
    assert!(
        error.to_string().contains("audit was not accepted"),
        "{error}"
    );
}

#[tokio::test]
async fn a_command_that_ran_and_failed_is_still_a_clean_stop() {
    // The one thing this must not do is call a command's own failure an
    // infrastructure fault: a non-zero exit stops nothing.
    let failed = completion(
        json!(1),
        "command",
        Ok(RunResult {
            cause: RunCause::Exited,
            scope_id: Some(1),
            process_id: Some(1),
            os_pid: Some(1),
            exit_code: Some(7),
            exit_signal: None,
            forced: Some(false),
            stdout: String::new(),
            stderr: String::new(),
            dropped_bytes: 0,
            cleanup_verified: true,
            cleanup_error: None,
            output_errors: vec![],
            audit_error: None,
        }),
    );
    assert!(failed.unclean().is_none());
    // The reply still reports it to whoever is listening.
    assert_eq!(failed.response["result"]["isError"], true);
}
