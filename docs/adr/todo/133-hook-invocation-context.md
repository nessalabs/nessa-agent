# 133. A hook sees every event kind, and thinking is visible

## Purpose

A permission-review hook can read the turn it is judging, not only the proposed
call. This records what that read contains. The window's size and the
fail-closed rule for a policy that requires a complete turn stay in
[ADR 0014](0014-nessa-owned-policy-hooks.md). This record does not add retention
and does not build the policy runtime. It supersedes ADR 0014's default of
keeping thinking out of a hook, for this window only. ADR 0014 still owns the
numeric caps.

- **Date:** 2026-10-12
- **Status:** proposed

## Context

`InvocationRecord.events` already holds the invocation's observations, including
message chunks whose durable save is still deferred. A hook that sees only the
proposed tool call cannot tell a phrase split across those chunks from a phrase
that never occurred, and cannot tell thinking from model text from a tool call
from a tool result. ADR 0014 caps one evaluation at 256 events and 64 KiB and
excludes thinking unless a verified policy administrator opts in. The policy
engine that would record that revision is not built. Claude Code and Codex both
reach this boundary through the permission exchange the application already owns.

Handing the turn to a hook is still a disclosure. The hook has to tell the
kinds apart, and the events are already typed. Hiding thinking behind a second
opt-in made every registration guess, and a withheld thought still had to be
accounted for so the copy stayed bounded.

## Decision

The application hands the hook a read-only `InvocationObservationWindow` over
observations already retained for the current invocation. The window uses ADR
0014's evaluation caps. Thinking is visible, and it spends the 256-event and
64 KiB budget like any other event. [`ObservationKind`] distinguishes thinking,
model text, a tool call, a tool result, and the other retained updates. There
is no `disclose_thinking` opt-in and no withheld view.

An event whose own payload exceeds 64 KiB is omitted whole. The walk continues.
The omitted event is a separator, sets the oversized flag, and sets omitted
text when it was a message. It spends one of the 256 event slots but no bytes,
so a run of oversized events cannot make the walk size or copy more than 256
events. The window reports an omitted prefix, omitted text, a cut text
sequence, and an oversized omission. Adjacent chunks of the same kind and
message identity are one sequence, so a phrase search crosses those boundaries.
Two messages that both lack an id share that identity. A miss is absence only
when no text was left out; otherwise it is inconclusive. A hook that needs the
whole turn treats inconclusive, and an incomplete window, as a no.

`Debug` prints the counts and the omission flags. It does not print event
payloads. That is a log-size bound, not a secrecy switch.

The permission-review hook runs when the harness asks about a tool. A tool the
harness runs without asking is not offered to this hook. The hook returns
`ToolVerdict`: allow, or refuse with a [`RefusalOutcome`]. The runner can also
record that the turn is already stopping. A refusal is attributed to the hook,
principal `permission-review-hook`, not the person who submitted the turn.
`ApprovalBasis::Hook` keeps the registration index when a hook returned, the
reason capped at 1024 bytes, and the outcome that was applied. A timeout or a
busy refusal has no index: the runtime refused before a hook returned. That
shape matches the deadline basis
from the open permission-deadline work: a variant on the approval basis and a
non-person actor. This branch does not depend on that work's types.

Refuse-and-continue answers the provider's deny option through the agent's
permission answer, so the agent continues. A continue that has no deny option,
when the binding can cancel one turn, is an end-turn: the pending review is
withdrawn as the hook, then the turn is cancelled. When the binding cannot
cancel one turn, that refusal is recorded as continue and the review is still
withdrawn, so the provider is not left waiting. End-turn answers the deny
option when one is offered, then cancels the turn. An end-turn the binding
cannot perform is recorded as continue and does not cancel the turn. It does
not close the session. Close-session withdraws the review as the hook,
recording the applied outcome, then closes with
`SessionCloseRequest::PolicyRefusal`. It does not select an offered option.
The deny, the withdrawal, and the turn cancel go through the agent's control
path. If
the deny or the withdrawal fails, that refusal still records the outcome that
was intended, and the turn is cancelled. The session stays usable, and the
invocation still reports the failure beside the provider result, so an
unaudited refusal never reads as a clean turn. As with any failed invocation,
queued work behind it is stopped rather than dispatched blind. If the turn cancel fails too,
both failures are reported, delivery first, and the session stops on the
observation-failure path. A withdrawal names the applied outcome in its
cancellation code. A hook error, panic, timeout, or busy guard is
deny-and-continue with the reason `"hook failed: error"`,
`"hook failed: panic"`, `"hook failed: timeout"`, or `"hook failed: busy"`.
A stop that arrives while the hook is running, or that was already observed
before the review, does not publish the review and does not answer it. A
refused review is not published either, because the hook has already answered
or withdrawn it. While
an earlier hook thread is still running, or while the process already holds
its 32 hook threads, a later review is refused immediately, without another
blocking thread.

A hook refusal built outside the runner records only continue. End-turn and
close-session are the runner's transitions: answering a deny does not itself
cancel the turn or close the session. So the public `HookRefusal::new` takes no
outcome, and only the runner, which then cancels the turn, builds an end-turn
refusal. No public answer, through the agent or the execution controller
directly, can claim an outcome nothing applied.

Only hooks that review permissions take part. A hook opts out by returning
`false` from `reviews_permissions`; the typed before- and after-invocation
registrations do. An opted-out hook is never called for a review, even when
another hook in the invocation reviews it. A hook whose `reviews_permissions`
panics takes part and refuses each review as a panic. With no reviewing hook,
a review is handled exactly as with no hooks at all, so the runtime can never
refuse it as busy or timed out.
Message text still waiting for its commit deadline is saved before the hook
runs, because the hook's wait cannot poll that deadline.

The review is human-paced. The suffix is copied, the evidence lock is released,
and the hook runs on a blocking thread until the session's injected
message-commit clock advances two seconds, selected against stop. A stop that
arrives first wins. Dropping the wait does not cancel the callback. The
blocking task cannot be cancelled, so a hook that overruns keeps its thread
until it returns; its late verdict is discarded, and it must not re-enter the
session. Before and after hooks see the
same window type from the same bounded copy. A failed capture runs the after
hooks with an empty window marked incomplete, and keeps the provider result.
An incomplete window does not by itself fail the tool. A rule that requires a
complete turn still fails closed under ADR 0014 when that rule exists.

## Alternatives considered

Keeping the withheld-by-default opt-in would leave two windows and a disclosure
flag on every registration. Thinking is part of the turn the hook is judging,
and the kind already says it is thinking. An opt-out was not simpler than
deleting the flag.

A new stored error for a hook failure would change the saved `AgentError` shape.
Hook failure is deny-and-continue. Close-session uses the existing policy
refusal. No schema version changes.

Cloning the live record on every review would copy message text the hook does
not read. Holding the evidence lock across the callback stalls close for the
whole callback. The copy is the suffix that fits, and an oversized body is not
cloned.

Slicing an oversized event into the window would let a search treat a cut
payload as the whole observation. Omitting it whole, and still walking, keeps
older text visible and the incomplete fact honest.

## Consequences

Hook authors match text across chunk boundaries, see thinking, and tell each
event's kind apart. They treat an inconclusive phrase and an incomplete window
as a no when they need the whole turn. Permission-review callbacks are waited
on for two seconds of the session's message-commit clock and do not hold the
evidence lock. A callback that overruns keeps its blocking thread until it
returns. A hook error does not
close the session. A close-session verdict does, and the close is a policy
refusal. A refusal is audited as the hook, with its index, capped reason, and
the outcome that was applied.

No history bound changes. No schema version changes. OpenCode is not given a
separate adapter: the shared application path is the one Claude Code and Codex
already use. What remains for the owner is the policy runtime's rule revision.

Permission deadline work that adds `ExecutionUpdate::DeadlineAnswered` is not
on this branch. When it lands, that update's payload is its id plus its reason,
it is a separator, and a review fixture carries `deadline: None`.
