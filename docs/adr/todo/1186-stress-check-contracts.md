# 1186. Stress checks measure the contract they name

## Purpose

Three conversation-stress failures are checks measuring a different event
from the one they name. Each check keeps its assertion. What it observes is
moved onto the event the assertion describes: WebKit's platform control
chord, the moment a tool report is queued, and the turn's settled
scheduling stage. This settles
[#1186](https://github.com/nessalabs/nessa-agent/issues/1186) and
[#1136](https://github.com/nessalabs/nessa-agent/issues/1136) together,
[#1183](https://github.com/nessalabs/nessa-agent/issues/1183), and
[#1181](https://github.com/nessalabs/nessa-agent/issues/1181).

- **Date:** 2026-11-02
- **Status:** accepted

## Context

The GitHub row's Disconnect and Choose repositories controls are a native
button and a native link, both at tab index 0, neither inert nor
`aria-hidden`. At rest their box has `pointer-events: none` and opacity 0;
`:hover` and `:focus-within` give the box back to the pointer and fade the
actions in. A macOS WebKit probe focused Disconnect, then saw plain Tab and
Shift+Tab leave the row, and Option+Tab and Option+Shift+Tab move between
the two controls. That is WebKit's platform rule: Tab walks text fields
unless Option is held or Full Keyboard Access is on. `pointer-events` does
not decide it. The controls were already reachable by the platform chord.

The trace check reads `startedAtMs` and `endedAtMs`. Those are the session's
receipt time. The supervisor was stamping them when it later read the
event, and the scripted pause was a `setTimeout` started before that read.
Under load the end was stamped during the pause, so the next start looked
early. The pause itself was not short.

The restart-report test registers two saved reports after `finish_turn`.
`finish_turn` was returning when the invocation result was saved, which is
before the after-hook. The hook then saw the new rows and delivered the
finished turn. One row at the next start was that delivery, not a lost
report. An isolated rerun usually returns only after the hook has run, so
the same binary passes.

## Decision

WebKit stress steps move between controls with Option+Tab (`Alt+Tab` in
Playwright) and Option+Shift+Tab. Chromium keeps Tab and Shift+Tab. The
product does not add a key handler, and it does not take the actions out of
the tab order to hide them.

A provider report is stamped when the session queues it, from the session's
injected message-commit clock, and a later read keeps that time. The runtime
clock reads the process timeline. A time the provider wrote on the event is
still replaced. The scripted pause waits until the gateway has queued every
update already written, and only then sleeps. The gap the trace check reads
is that sleep, on one monotonic timeline.

`finish_turn` returns when the invocation's scheduling stage is Settled or
Cancelled. The after-hook has returned by then. Reports saved after that
are the stopped run's rows. A report saved before the hook runs can still
be delivered then; that delivery is the parent's history, and the pending
count does not include it. The settlement look itself is unchanged.

## States

| Check | What is true before the assertion | What the assertion reads |
| --- | --- | --- |
| GitHub row, Chromium | Disconnect is focused. The actions are in the tab order. | Shift+Tab lands on Choose repositories. Tab lands on Disconnect. Focus in the row shows the actions. |
| GitHub row, WebKit | Same controls, same focus. Plain Tab walks text fields only. | Alt+Shift+Tab lands on Choose repositories. Alt+Tab lands on Disconnect. Focus in the row shows the actions. |
| Trace pause | The previous call's end is already queued. The sleep has not started. | After the sleep, the next call's queued start is at least the pause later, minus the existing timer slack. A slow supervisor read does not move either time. |
| Restart reports | The child's finished turn is Settled or Cancelled. Its after-hook has returned. | Both rows are still pending. The next start delivers the finished turn as completed and the turn that never ran as interrupted. |

## Invariants

| Guarantee | Enforcer |
| --- | --- |
| WebKit's backward control chord is Option+Shift+Tab, and the forward chord is Option+Tab. Chromium's are Shift+Tab and Tab. The GitHub step presses those chords and asserts the focused element. | `keys.previousControl` and `keys.nextControl`. `selectors.test.mjs` reads the WebKit chords. The GitHub step presses them. |
| Hiding the GitHub actions from the pointer does not remove them from the tab order. | The connected row keeps tab index 0 on Choose repositories, an enabled Disconnect, and no `inert` on the actions. `github-row.test.tsx` reads that. |
| A report's saved time is the queue time. A read delayed past that queue does not rewrite it. A provider-supplied time is not that time. | `EventSender::try_send` stamps the open's injected clock and marks the event. Session opens pass the session manager's clock. `RuntimeMessageCommitClock` reads `process_unix_ms`. `SessionManager::observed` keeps a marked time and replaces any other. The queue test moves a substituted clock after the send and reads the earlier time back, including from `observed`. The manager test reads a marked time back unchanged and a provider time replaced. |
| The scripted pause sleeps only after the gateway has taken the updates written before it. | `runSteps` awaits `accepted` before the timer. The agent sends `session/accept` and treats the reply, including the unsupported-method error, as that take. The scenario test leaves the request unanswered and the turn does not finish its pause. |
| A report registered once the turn's scheduling has settled is not delivered by that turn's after-hook. | `finish_turn` waits for Settled or Cancelled, which the runner records after the hook returns. The restart test reads two pending rows after that, including after yielding. |

## Alternatives considered

- **Enable Full Keyboard Access in the harness.** Plain Tab would then reach
  the controls, and the step would not name the chord a person uses when
  that setting is off. The probe's failing chord was plain Tab. The passing
  chord was Option+Tab. The check should press the chord it claims to test.
- **A page key handler that walks the row on Tab.** That replaces the
  platform order for every browser, including Chromium, where Tab already
  reaches the controls. The actions would no longer be native tab stops.
- **Treat `pointer-events: none` as the cause and force the actions visible.**
  The probe focused Disconnect and still saw plain Tab leave the row, with
  the actions' pointer events already `auto` under `:focus-within`. Opacity
  and pointer events are the pointer contract. They are not the tab order.
- **Widen the 100 ms trace slack.** The shortfall was the end being stamped
  during the pause. A larger slack still compares a dequeue delay with a
  sleep, and the next load can exceed it.
- **Stamp with a fresh `SystemTime` at dequeue.** That is the clock the
  supervisor was already using. It moves when the operating system steps
  the wall clock, and it moves again when the supervisor is late. The
  process timeline and the queue time are the ones the other gateway stamps
  already use.
- **Accept either pending count in the restart test.** One row can be a
  delivered report or a lost one. The parent's history is what distinguishes
  them. The test keeps a single expected count, taken after the hook has
  had its turn.
- **Sleep, then register.** A sleep hopes the hook has run. Scheduling
  Settled is the hook's own completion, recorded by the runner after the
  hook returns.

## Consequences

WebKit coverage of the GitHub actions stays a focus assertion. It fails if
Option+Tab does not move between those two controls. It does not click them
instead.

Tool waterfalls move to queue time. A supervisor that is busy drawing or
saving no longer shortens a bar. A provider cannot choose the time. The
scripted pause pays one unsupported-method round trip before it sleeps.
That request is the barrier, not a product method; a gateway that answers
it with a result or with "method not found" has queued the earlier updates
either way.

`finish_turn` waits for scheduling, so a test that only needed the result
field now also waits for the runner's Settled edge. That edge follows the
after-hook on the queued path these tests use. A report saved in the gap
before that edge can still be delivered, and a test that wants the stopped
run's rows waits for the edge.

Restart console lines (#1187, #1182) and the desktop-verification fixture
edits on #1205 are outside this decision.

## Review brief

Review the branch `cursor/stress-flake-contracts-2299` against `main`.
The tree under review is the committed head; do not restack it during the
check. Intended behavior is the state table above.

Scope: `keys.previousControl` and the GitHub stress step; the arrival stamp
on the ACP event queue and `SessionManager::observed`; the scripted pause
barrier; `finish_turn`'s settlement wait and the restart-report test; this
record. Out of scope: restart console classification, vault unlock in the
stress setup, and any `codex/*` or `claude/*` branch.

Try to break the guarantees. Press plain Tab on the WebKit chord table and
confirm the test rejects it. Queue an event on a substituted clock, move
that clock, and confirm the saved time is still the enqueue time; put a
provider time on an unmarked event and confirm the session clock replaces
it. Leave
`session/accept` unanswered and confirm a 30 ms pause does not finish.
Register the two restart reports before the scheduling stage settles, if
you can open that window, and confirm the finished turn's report is in the
parent history rather than missing. Register them after Settled and confirm
both are still pending. A retry, a larger slack, or a click in place of
the chord is a miss even if a later run is green.
