//! The follow loop against scripted substitutes, one test per behavior.
use super::*;
use crate::read_only_sync::application::{SubscriptionCode, SubscriptionRefusal};
use nessa_protocol::product_contract::generated::{
    ConversationErrorCode, RecordReadErrorCode, SessionCloseReason,
};
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Subscribe,
    Wait,
    Pass,
}
/// A session that answers each call from its script, in order.
struct Script {
    subscribe: Option<Result<Subscribed, GatewayError>>,
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
            subscribe: Some(Ok(Subscribed {
                subscription: "1".into(),
                operation: 2,
                first: Wait::View {
                    cursor: cursor(1),
                    during_pass: false,
                },
            })),
            waits: waits.into_iter().collect(),
            passes: passes.into_iter().collect(),
            unbegun: None,
            calls: vec![],
        }
    }
}
impl FollowSession for Script {
    type Report = usize;
    fn subscribe(&mut self) -> Result<Subscribed, GatewayError> {
        self.calls.push(Call::Subscribe);
        self.subscribe.take().expect("one subscribe")
    }
    fn wait(&mut self) -> Result<Wait, GatewayError> {
        self.calls.push(Call::Wait);
        self.waits.pop_front().expect("scripted wait")
    }
    fn pass(&mut self) -> Result<FollowPass<usize>, GatewayError> {
        self.calls.push(Call::Pass);
        if let Some(cause) = self.unbegun.take() {
            return Err(cause);
        }
        let count = self
            .calls
            .iter()
            .filter(|call| **call == Call::Pass)
            .count();
        Ok(FollowPass {
            report: count,
            result: self.passes.pop_front().expect("scripted pass"),
        })
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Subscribed(String),
    Pass(Trigger, usize),
    View(ViewCursor, bool),
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
impl FollowEvents<usize> for Lines {
    fn subscribed(&mut self, subscribed: &Subscribed) -> Result<(), OutputLost> {
        self.push(Line::Subscribed(subscribed.subscription.clone()))
    }
    fn pass(&mut self, trigger: Trigger, report: &usize) -> Result<(), OutputLost> {
        self.push(Line::Pass(trigger, *report))
    }
    fn view(&mut self, cursor: &ViewCursor, during_pass: bool) -> Result<(), OutputLost> {
        self.push(Line::View(cursor.clone(), during_pass))
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
fn cursor(position: u64) -> ViewCursor {
    ViewCursor {
        incarnation: "incarnation-1".into(),
        position: position.to_string(),
    }
}
fn view(position: u64, during_pass: bool) -> Result<Wait, GatewayError> {
    Ok(Wait::View {
        cursor: cursor(position),
        during_pass,
    })
}

/// Every subscribe refusal ends before any pass, keeping its typed code;
/// authority codes are the ones `asks_status` names for the PC5 recheck.
#[test]
fn subscribe_refusal_ends_before_any_pass() {
    for refusal in [
        SubscriptionRefusal::Subscription(SubscriptionCode::Capacity),
        SubscriptionRefusal::Subscription(SubscriptionCode::Duplicate),
        SubscriptionRefusal::Conversation(ConversationErrorCode::ConversationNotFound),
        SubscriptionRefusal::Forbidden,
    ] {
        let cause = GatewayError::Subscription(refusal);
        let mut script = Script::new([], []);
        script.subscribe = Some(Err(cause));
        let (end, lines) = run(&mut script, 3);
        assert_eq!(end.reason, EndReason::SubscribeRefused);
        assert_eq!(end.cause, Some(cause));
        assert!(!end.clean());
        assert_eq!(
            asks_status(cause),
            refusal == SubscriptionRefusal::Forbidden
        );
        assert_eq!(script.calls, [Call::Subscribe]);
        assert!(lines.is_empty());
    }
    // A subscribe lost on the transport is not a refusal.
    let mut script = Script::new([], []);
    script.subscribe = Some(Err(GatewayError::Closed(None)));
    assert_eq!(run(&mut script, 3).0.reason, EndReason::ConnectionClosed);
}

/// A subscription that ends in its first frame ends the run before any
/// pass, with the refusal it carried.
#[test]
fn an_end_as_the_first_frame_runs_no_pass() {
    let cause = GatewayError::Subscription(SubscriptionRefusal::Forbidden);
    let mut script = Script::new([], []);
    script.subscribe = Some(Ok(Subscribed {
        subscription: "1".into(),
        operation: 2,
        first: Wait::Ended(Ended {
            reason: SubscriptionEnd::Refused,
            cause: Some(cause),
        }),
    }));
    let (end, lines) = run(&mut script, 3);
    assert_eq!(
        end,
        End {
            reason: EndReason::SubscriptionEnded(SubscriptionEnd::Refused),
            cause: Some(cause),
        }
    );
    assert_eq!(script.calls, [Call::Subscribe]);
    assert_eq!(lines, [Line::Subscribed("1".into())]);
}

/// The recheck pass runs first even when the session already holds a newer
/// view, which `wait` then returns at once.
#[test]
fn first_pass_is_the_recheck_even_with_a_kept_view() {
    let mut script = Script::new([PassResult::Complete; 2], [view(2, true)]);
    let (end, lines) = run(&mut script, 2);
    assert_eq!(
        script.calls,
        [Call::Subscribe, Call::Pass, Call::Wait, Call::Pass]
    );
    assert_eq!(
        lines,
        [
            Line::Subscribed("1".into()),
            Line::Pass(Trigger::Recheck, 1),
            Line::View(cursor(2), true),
            Line::Pass(Trigger::View, 2),
        ]
    );
    assert_eq!(end.reason, EndReason::PassesExhausted);
}

/// A view whose cursor moved, read while waiting, starts exactly one pass.
#[test]
fn a_moved_view_while_waiting_starts_one_pass() {
    let mut script = Script::new(
        [PassResult::Complete; 2],
        [view(2, false), Err(GatewayError::TimedOut)],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        lines[1..],
        [
            Line::Pass(Trigger::Recheck, 1),
            Line::View(cursor(2), false),
            Line::Pass(Trigger::View, 2),
        ]
    );
    assert_eq!(end.reason, EndReason::Idle);
}

/// A view at the cursor already copied changed only live facts: it writes
/// no line and runs no pass. That includes the first view again, which the
/// recheck pass already covered. A new incarnation is a moved cursor.
#[test]
fn a_view_at_the_same_cursor_runs_no_pass() {
    let moved = ViewCursor {
        incarnation: "incarnation-2".into(),
        position: "1".into(),
    };
    let replaced = || {
        Ok(Wait::View {
            cursor: moved.clone(),
            during_pass: false,
        })
    };
    let mut script = Script::new(
        [PassResult::Complete; 2],
        [
            view(1, true),
            view(1, false),
            replaced(),
            replaced(),
            Err(GatewayError::TimedOut),
        ],
    );
    let (end, lines) = run(&mut script, 5);
    assert_eq!(
        lines[1..],
        [
            Line::Pass(Trigger::Recheck, 1),
            Line::View(moved.clone(), false),
            Line::Pass(Trigger::View, 2),
        ]
    );
    assert_eq!(end.reason, EndReason::Idle);
}

/// An incomplete pass is followed at once by the next one, with no
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
            Call::Subscribe,
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

/// An authority refusal during a pass ends the run after its line,
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

/// A transport, timeout or protocol failure during a pass, or a
/// wait's protocol failure, is `unavailable`; a pass that failed with no
/// gateway cause is `passFailed`. Neither waits or passes again.
#[test]
fn transport_or_cache_failure_ends_the_follow() {
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
        assert_eq!(script.calls, [Call::Subscribe, Call::Pass]);
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

/// A pass that cannot begin has no report, so it writes no pass
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
    assert_eq!(lines, [Line::Subscribed("1".into())]);
}

/// The subscription's end, kept during a pass, stops the run at the next
/// wait, after that pass's line. A refused end keeps its typed refusal, so
/// an authority refusal asks the pinned status again.
#[test]
fn subscription_end_stops_after_the_running_pass() {
    let lagging = Ended {
        reason: SubscriptionEnd::Lagging,
        cause: None,
    };
    let revoked = GatewayError::Subscription(SubscriptionRefusal::Forbidden);
    let refused = Ended {
        reason: SubscriptionEnd::Refused,
        cause: Some(revoked),
    };
    for ended in [lagging, refused] {
        let mut script = Script::new([PassResult::Complete], [Ok(Wait::Ended(ended))]);
        let (end, lines) = run(&mut script, 5);
        assert_eq!(
            end,
            End {
                reason: EndReason::SubscriptionEnded(ended.reason),
                cause: ended.cause,
            }
        );
        assert_eq!(lines.last(), Some(&Line::Pass(Trigger::Recheck, 1)));
        assert!(!end.clean());
    }
    assert!(asks_status(revoked));
}

/// A closed connection, while waiting or during a pass, ends
/// `connectionClosed` whether or not the close carries a reason. Plain EOF
/// asks for no status recheck. A terminal revoke stays `connectionClosed`
/// and does ask, so the composition reports that recheck on the ended line.
#[test]
fn connection_close_stays_connection_closed() {
    let plain = GatewayError::Closed(None);
    let revoked = GatewayError::Closed(Some(SessionCloseReason::AuthorizationLost));
    for cause in [plain, revoked] {
        let mut waiting = Script::new([PassResult::Complete], [Err(cause)]);
        let mut passing = Script::new([PassResult::Failed(Some(cause))], []);
        for script in [&mut waiting, &mut passing] {
            let (end, _) = run(script, 5);
            assert_eq!(
                end,
                End {
                    reason: EndReason::ConnectionClosed,
                    cause: Some(cause)
                }
            );
        }
    }
    assert!(!asks_status(plain));
    assert!(asks_status(revoked));
}

/// A wait that reaches its deadline with nothing read ends cleanly.
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

/// The recheck counts toward the budget, and the last pass is not
/// followed by a wait, even when it was incomplete.
#[test]
fn pass_budget_ends_cleanly() {
    let mut script = Script::new([PassResult::Complete], []);
    let (end, _) = run(&mut script, 1);
    assert_eq!(script.calls, [Call::Subscribe, Call::Pass]);
    assert!(end.clean());
    let mut script = Script::new(
        [PassResult::Complete, PassResult::Incomplete],
        [view(2, false)],
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

/// A refused line stops the loop there; nothing after it is called.
#[test]
fn output_failure_stops_the_follow() {
    for (refuse_at, calls) in [
        (0, vec![Call::Subscribe]),
        (1, vec![Call::Subscribe, Call::Pass]),
        (2, vec![Call::Subscribe, Call::Pass, Call::Wait]),
    ] {
        let mut script = Script::new([PassResult::Complete; 2], [view(2, false)]);
        let mut lines = Lines {
            refuse_at: Some(refuse_at),
            ..Lines::default()
        };
        assert_eq!(follow(&mut script, &mut lines, passes(5)), Err(OutputLost));
        assert_eq!(script.calls, calls);
    }
}

/// A pass answered `source_preparing` is the same pass again, at
/// once and uncounted toward `MAX_PASSES`; the attempt that succeeds
/// continues the follow loop as any complete pass does.
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
        [Call::Subscribe, Call::Pass, Call::Pass, Call::Pass]
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

/// A source still preparing after `PREPARING_ATTEMPTS` attempts ends
/// the run explicitly with that cause; it is never reported as success.
#[test]
fn preparing_beyond_the_attempt_bound_ends_unavailable() {
    let (captured, _guard) = limit_log();
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
    assert!(
        limit_text(&captured).contains("client.preparing_attempts"),
        "{}",
        limit_text(&captured)
    );
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

/// Before subscribing, a discovery answered `source_preparing` is
/// asked again at once, and the attempt that succeeds is the one reported, so
/// the follow loop goes on to subscribe instead of refusing.
#[test]
fn preparing_discovery_is_retried_until_it_succeeds() {
    let preparing = Some(GatewayError::Record(RecordReadErrorCode::SourcePreparing));
    assert_eq!(discovery(&[preparing, preparing, None]), (None, 3));
}

/// A discovery still preparing after `PREPARING_ATTEMPTS` attempts is reported
/// with that cause; any other failure is reported at once.
#[test]
fn discovery_retries_only_preparing_and_only_up_to_the_bound() {
    let (captured, _guard) = limit_log();
    let preparing = Some(GatewayError::Record(RecordReadErrorCode::SourcePreparing));
    let answers = vec![preparing; PREPARING_ATTEMPTS + 1];
    assert_eq!(discovery(&answers), (preparing, PREPARING_ATTEMPTS as u64));
    assert!(
        limit_text(&captured).contains("client.preparing_attempts"),
        "{}",
        limit_text(&captured)
    );
    let refused = Some(GatewayError::TimedOut);
    assert_eq!(discovery(&[refused, None]), (refused, 1));
}

#[derive(Clone)]
struct LimitLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LimitLog {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("limit log").extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LimitLog {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
fn limit_log() -> (LimitLog, tracing::subscriber::DefaultGuard) {
    let captured = LimitLog(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    (captured, tracing::subscriber::set_default(subscriber))
}
fn limit_text(captured: &LimitLog) -> String {
    String::from_utf8(captured.0.lock().expect("limit log").clone()).unwrap()
}
