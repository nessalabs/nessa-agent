//! The watch loop against scripted substitutes, one test per row W3–W14 and W17.
use super::*;
use crate::product_contract::generated::{ChangeWatchErrorCode, RecordReadErrorCode};
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Register,
    Wait,
    Pass,
}
/// A session that answers each call from its script, in order.
struct Script {
    register: Option<Result<Registered, GatewayError>>,
    waits: VecDeque<Result<Wait, GatewayError>>,
    passes: VecDeque<PassResult>,
    /// The next pass cannot begin, with this cause.
    unbegun: Option<GatewayError>,
    calls: Vec<Call>,
}
impl Script {
    fn new(
        passes: impl IntoIterator<Item = PassResult>,
        waits: impl IntoIterator<Item = Result<Wait, GatewayError>>,
    ) -> Self {
        Self {
            register: Some(Ok(Registered {
                watch: "watch-1".into(),
                operation: 2,
            })),
            waits: waits.into_iter().collect(),
            passes: passes.into_iter().collect(),
            unbegun: None,
            calls: vec![],
        }
    }
}
impl WatchSession for Script {
    type Report = usize;
    fn register(&mut self) -> Result<Registered, GatewayError> {
        self.calls.push(Call::Register);
        self.register.take().expect("one registration")
    }
    fn wait(&mut self) -> Result<Wait, GatewayError> {
        self.calls.push(Call::Wait);
        self.waits.pop_front().expect("scripted wait")
    }
    fn pass(&mut self) -> Result<WatchPass<usize>, GatewayError> {
        self.calls.push(Call::Pass);
        if let Some(cause) = self.unbegun.take() {
            return Err(cause);
        }
        let count = self
            .calls
            .iter()
            .filter(|call| **call == Call::Pass)
            .count();
        Ok(WatchPass {
            report: count,
            result: self.passes.pop_front().expect("scripted pass"),
        })
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Registered(String),
    Pass(Trigger, usize),
    Hint(bool),
}
/// Lines as written; the line numbered `refuse_at` (from zero) is refused.
#[derive(Default)]
struct Lines {
    written: Vec<Line>,
    refuse_at: Option<usize>,
}
impl Lines {
    fn push(&mut self, line: Line) -> Result<(), OutputLost> {
        if self.refuse_at == Some(self.written.len()) {
            return Err(OutputLost);
        }
        self.written.push(line);
        Ok(())
    }
}
impl WatchEvents<usize> for Lines {
    fn registered(&mut self, registered: &Registered) -> Result<(), OutputLost> {
        self.push(Line::Registered(registered.watch.clone()))
    }
    fn pass(&mut self, trigger: Trigger, report: &usize) -> Result<(), OutputLost> {
        self.push(Line::Pass(trigger, *report))
    }
    fn hint(&mut self, during_pass: bool) -> Result<(), OutputLost> {
        self.push(Line::Hint(during_pass))
    }
}
fn passes(count: usize) -> NonZeroUsize {
    NonZeroUsize::new(count).unwrap()
}
fn run(script: &mut Script, max: usize) -> (End, Vec<Line>) {
    let mut lines = Lines::default();
    let end = follow(script, &mut lines, passes(max)).unwrap();
    (end, lines.written)
}
fn hint(during_pass: bool) -> Result<Wait, GatewayError> {
    Ok(Wait::Hint { during_pass })
}

/// Row W3: every refusal ends before any pass, keeping its typed code;
/// authority codes are the ones `asks_status` names for the PC5 recheck.
#[test]
fn registration_refusal_ends_before_any_pass() {
    for code in [
        ChangeWatchErrorCode::WatchCapacity,
        ChangeWatchErrorCode::WatchDuplicate,
        ChangeWatchErrorCode::TemporarilyUnavailable,
        ChangeWatchErrorCode::StaleEpoch,
    ] {
        let mut script = Script::new([], []);
        script.register = Some(Err(GatewayError::Watch(code)));
        let (end, lines) = run(&mut script, 3);
        assert_eq!(end.reason, EndReason::RegistrationRefused);
        assert_eq!(end.cause, Some(GatewayError::Watch(code)));
        assert!(!end.clean());
        assert_eq!(
            asks_status(end.cause.unwrap()),
            code == ChangeWatchErrorCode::StaleEpoch
        );
        assert_eq!(script.calls, [Call::Register]);
        assert!(lines.is_empty());
    }
    // A registration lost on the transport is not a refusal.
    let mut script = Script::new([], []);
    script.register = Some(Err(GatewayError::Closed(None)));
    assert_eq!(run(&mut script, 3).0.reason, EndReason::ConnectionClosed);
}

/// Row W4: the recheck pass runs first even when the session already holds a
/// hint, which `wait` then returns at once.
#[test]
fn first_pass_is_the_recheck_even_with_a_dirty_inbox() {
    let mut script = Script::new([PassResult::Complete; 2], [hint(true)]);
    let (end, lines) = run(&mut script, 2);
    assert_eq!(
        script.calls,
        [Call::Register, Call::Pass, Call::Wait, Call::Pass]
    );
    assert_eq!(
        lines,
        [
            Line::Registered("watch-1".into()),
            Line::Pass(Trigger::Recheck, 1),
            Line::Hint(true),
            Line::Pass(Trigger::Hint, 2),
        ]
    );
    assert_eq!(end.reason, EndReason::PassesExhausted);
}

/// Row W5: a hint read while waiting starts exactly one pass.
#[test]
fn hint_while_waiting_starts_one_pass() {
    let mut script = Script::new(
        [PassResult::Complete; 2],
        [hint(false), Err(GatewayError::TimedOut)],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        lines[1..],
        [
            Line::Pass(Trigger::Recheck, 1),
            Line::Hint(false),
            Line::Pass(Trigger::Hint, 2),
        ]
    );
    assert_eq!(end.reason, EndReason::Idle);
}

/// Row W7: an incomplete pass is followed at once by the next one, with no
/// wait between them.
#[test]
fn incomplete_pass_continues_without_waiting() {
    let mut script = Script::new(
        [
            PassResult::Incomplete,
            PassResult::Incomplete,
            PassResult::Complete,
        ],
        [Err(GatewayError::TimedOut)],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        script.calls,
        [
            Call::Register,
            Call::Pass,
            Call::Pass,
            Call::Pass,
            Call::Wait
        ]
    );
    assert_eq!(
        lines[1..],
        [
            Line::Pass(Trigger::Recheck, 1),
            Line::Pass(Trigger::Incomplete, 2),
            Line::Pass(Trigger::Incomplete, 3),
        ]
    );
    assert_eq!(end.reason, EndReason::Idle);
}

/// Row W8: an authority refusal during a pass ends the run after its line,
/// with the cause the PC5 recheck is asked for.
#[test]
fn authority_failure_ends_unauthorized() {
    let cause = GatewayError::Record(RecordReadErrorCode::WrongReceiver);
    let mut script = Script::new([PassResult::Failed(Some(cause))], []);
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        end,
        End {
            reason: EndReason::Unauthorized,
            cause: Some(cause)
        }
    );
    assert!(asks_status(cause));
    assert_eq!(lines.last(), Some(&Line::Pass(Trigger::Recheck, 1)));
    assert!(!end.clean());
}

/// Row W9: a transport, timeout or protocol failure during a pass, or a
/// wait's protocol failure, is `unavailable`; a pass that failed with no
/// gateway cause is `passFailed`. Neither waits or passes again.
#[test]
fn transport_or_cache_failure_ends_the_watch() {
    for cause in [
        GatewayError::Transport,
        GatewayError::TimedOut,
        GatewayError::Protocol,
    ] {
        let mut script = Script::new([PassResult::Failed(Some(cause))], []);
        assert_eq!(
            run(&mut script, 5).0,
            End {
                reason: EndReason::Unavailable,
                cause: Some(cause)
            }
        );
        assert_eq!(script.calls, [Call::Register, Call::Pass]);
    }
    let mut script = Script::new([PassResult::Failed(None)], []);
    assert_eq!(
        run(&mut script, 5).0,
        End {
            reason: EndReason::PassFailed,
            cause: None
        }
    );
    let mut script = Script::new([PassResult::Complete], [Err(GatewayError::Correlation)]);
    assert_eq!(run(&mut script, 5).0.reason, EndReason::Unavailable);
}

/// Row W9: a pass that cannot begin has no report, so it writes no pass
/// line; the run ends on its cause.
#[test]
fn pass_that_cannot_begin_ends_without_a_pass_line() {
    let mut script = Script::new([], []);
    script.unbegun = Some(GatewayError::Busy);
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        end,
        End {
            reason: EndReason::Unavailable,
            cause: Some(GatewayError::Busy)
        }
    );
    assert_eq!(lines, [Line::Registered("watch-1".into())]);
}

/// Row W10: the watch's end, kept during a pass, stops the run at the next
/// wait, after that pass's line.
#[test]
fn watch_ended_stops_after_the_running_pass() {
    let mut script = Script::new(
        [PassResult::Complete],
        [Ok(Wait::Ended(ChangeWatchEndReason::NotificationFailed))],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        end.reason,
        EndReason::WatchEnded(ChangeWatchEndReason::NotificationFailed)
    );
    assert_eq!(lines.last(), Some(&Line::Pass(Trigger::Recheck, 1)));
    assert!(!end.clean());
}

/// Row W11: a closed connection, while waiting or during a pass, ends
/// `connectionClosed` with a cause that asks for no status recheck.
#[test]
fn connection_close_ends_without_recheck() {
    let closed = GatewayError::Closed(None);
    let mut waiting = Script::new([PassResult::Complete], [Err(closed)]);
    let mut passing = Script::new([PassResult::Failed(Some(closed))], []);
    for script in [&mut waiting, &mut passing] {
        let (end, _) = run(script, 5);
        assert_eq!(
            end,
            End {
                reason: EndReason::ConnectionClosed,
                cause: Some(closed)
            }
        );
        assert!(!asks_status(closed));
    }
}

/// Row W12: a wait that reaches its deadline with nothing read ends cleanly.
#[test]
fn idle_wait_ends_cleanly() {
    let mut script = Script::new([PassResult::Complete], [Err(GatewayError::TimedOut)]);
    let (end, _) = run(&mut script, 5);
    assert_eq!(
        end,
        End {
            reason: EndReason::Idle,
            cause: None
        }
    );
    assert!(end.clean());
}

/// Row W13: the recheck counts toward the budget, and the last pass is not
/// followed by a wait, even when it was incomplete.
#[test]
fn pass_budget_ends_cleanly() {
    let mut script = Script::new([PassResult::Complete], []);
    let (end, _) = run(&mut script, 1);
    assert_eq!(script.calls, [Call::Register, Call::Pass]);
    assert!(end.clean());
    let mut script = Script::new(
        [PassResult::Complete, PassResult::Incomplete],
        [hint(false)],
    );
    let (end, _) = run(&mut script, 2);
    assert_eq!(
        end,
        End {
            reason: EndReason::PassesExhausted,
            cause: None
        }
    );
    assert_eq!(script.calls.last(), Some(&Call::Pass));
}

/// Row W14: a refused line stops the loop there; nothing after it is called.
#[test]
fn output_failure_stops_the_watch() {
    for (refuse_at, calls) in [
        (0, vec![Call::Register]),
        (1, vec![Call::Register, Call::Pass]),
        (2, vec![Call::Register, Call::Pass, Call::Wait]),
    ] {
        let mut script = Script::new([PassResult::Complete; 2], [hint(false)]);
        let mut lines = Lines {
            refuse_at: Some(refuse_at),
            ..Lines::default()
        };
        assert_eq!(follow(&mut script, &mut lines, passes(5)), Err(OutputLost));
        assert_eq!(script.calls, calls);
    }
}

/// Row W17: a pass answered `source_preparing` is the same pass again, at
/// once and uncounted toward `MAX_PASSES`; the attempt that succeeds
/// continues the watch as any complete pass does.
#[test]
fn preparing_pass_is_retried_until_it_succeeds() {
    let preparing = PassResult::Failed(Some(GatewayError::Record(
        RecordReadErrorCode::SourcePreparing,
    )));
    let mut script = Script::new(
        [preparing, preparing, PassResult::Complete],
        [Err(GatewayError::TimedOut)],
    );
    let (end, lines) = run(&mut script, 1);
    assert_eq!(
        script.calls,
        [Call::Register, Call::Pass, Call::Pass, Call::Pass]
    );
    assert_eq!(
        lines[1..],
        [
            Line::Pass(Trigger::Recheck, 1),
            Line::Pass(Trigger::Preparing, 2),
            Line::Pass(Trigger::Preparing, 3),
        ]
    );
    // The one counted pass is the one that succeeded.
    assert_eq!(end.reason, EndReason::PassesExhausted);
    assert!(end.clean());
}

/// Row W17: a source still preparing after `PREPARING_ATTEMPTS` attempts ends
/// the run explicitly with that cause; it is never reported as success.
#[test]
fn preparing_beyond_the_attempt_bound_ends_unavailable() {
    let cause = GatewayError::Record(RecordReadErrorCode::SourcePreparing);
    let mut script = Script::new(
        std::iter::repeat_n(PassResult::Failed(Some(cause)), PREPARING_ATTEMPTS),
        [],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        end,
        End {
            reason: EndReason::Unavailable,
            cause: Some(cause)
        }
    );
    assert!(!end.clean());
    assert_eq!(lines.len(), 1 + PREPARING_ATTEMPTS);
    assert_eq!(
        script
            .calls
            .iter()
            .filter(|call| **call == Call::Pass)
            .count(),
        PREPARING_ATTEMPTS
    );
    assert!(!script.calls.contains(&Call::Wait));
}

/// Discovery attempts answering these failures in order, and the attempts
/// `discover` made before it returned the one it reports.
fn discovery(answers: &[Option<GatewayError>]) -> (Option<GatewayError>, u64) {
    let mut answers = answers.iter().copied();
    let mut made = 0_u64;
    let reported = discover(|| {
        made += 1;
        Ok::<_, ()>(GatewayAttempt {
            result: Some(()),
            outcome: super::super::GatewayOutcome {
                operation: made,
                failure: answers.next().unwrap(),
            },
        })
    })
    .unwrap();
    assert_eq!(reported.outcome.operation, made);
    (reported.outcome.failure, made)
}

/// Row W17 before registration: a discovery answered `source_preparing` is
/// asked again at once, and the attempt that succeeds is the one reported, so
/// the watch goes on to register instead of refusing.
#[test]
fn preparing_discovery_is_retried_until_it_succeeds() {
    let preparing = Some(GatewayError::Record(RecordReadErrorCode::SourcePreparing));
    assert_eq!(discovery(&[preparing, preparing, None]), (None, 3));
}

/// A discovery still preparing after `PREPARING_ATTEMPTS` attempts is reported
/// with that cause (row W16's refusal); any other failure is reported at once.
#[test]
fn discovery_retries_only_preparing_and_only_up_to_the_bound() {
    let preparing = Some(GatewayError::Record(RecordReadErrorCode::SourcePreparing));
    let answers = vec![preparing; PREPARING_ATTEMPTS + 1];
    assert_eq!(discovery(&answers), (preparing, PREPARING_ATTEMPTS as u64));
    let refused = Some(GatewayError::TimedOut);
    assert_eq!(discovery(&[refused, None]), (refused, 1));
}
