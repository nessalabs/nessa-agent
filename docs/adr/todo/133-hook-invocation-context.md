# 133. Thinking text reaches a hook only by an explicit disclosure opt-in

## Purpose

A pre-tool hook can read the turn it is judging, not only the proposed call.
This records the disclosure decision for that read: provider reasoning stays
out of the hook unless the hook's own registration opts in. The window's size
and the fail-closed rule for a policy that requires a complete turn stay in
[ADR 0014](0014-nessa-owned-policy-hooks.md). This record does not add retention
and does not build the policy runtime.

- **Date:** 2026-10-12
- **Status:** proposed

## Context

`InvocationRecord.events` already holds the invocation's observations, including
message chunks whose durable save is still deferred. A hook that sees only the
proposed tool call cannot tell a phrase split across those chunks from a phrase
that never occurred, and cannot tell withheld reasoning from text the user sees.
Handing thinking text to a hook discloses model output the transcript does not.
ADR 0014 already caps one evaluation at 256 events and 64 KiB and excludes
thinking unless a verified policy administrator opts in and that choice is
recorded with an immutable rule revision. The policy engine that would record
the revision is not built. Claude Code and Codex both reach this boundary
through the permission exchange the application already owns.

## Decision

The application hands the hook a read-only `InvocationObservationWindow` over
observations already retained for the current invocation. The window uses ADR
0014's evaluation caps. An event whose own payload exceeds 64 KiB is omitted
whole. The window reports an omitted prefix, omitted text, a cut text sequence,
and an oversized omission. Adjacent chunks of the same kind and message identity
are one sequence, so a phrase search crosses those boundaries. A miss is absence
only when every visible sequence is fully included; otherwise it is inconclusive.

Thinking is withheld unless that hook registration opts in
(`disclose_thinking`, or `InvocationHook::thinking_disclosure`). Withheld
thoughts stay out of the budget, the iterator, and the search, and they still
separate neighboring text. The opt-in is the disclosure decision for this
slice. Recording it on an immutable policy-rule revision remains ADR 0014's
policy runtime.

The pre-tool hook borrows the live record and runs before the review is
retained. A hook error reuses the saved before-invocation rejection and stops
the turn; the review is not admitted. Before and after hooks see the same
window type. Their suffix is copied, bounded by those caps, and the evidence
lock is released before the callback. An incomplete window is reported. It does
not by itself fail the tool. A rule that requires a complete turn still fails
closed under ADR 0014 when that rule exists.

## Alternatives considered

A new stored error for the pre-tool gate would change the saved `AgentError`
shape and force a schema bump beside the permission-deadline work. The existing
before-invocation rejection already means "a hook refused a guarded action
before it was admitted."

Cloning the live record on every tool call would make the borrow unnecessary
and would copy message text on the hot path. Holding the evidence lock across
before and after callbacks would stall session admission for the whole callback,
which those callbacks are allowed to spend returning.

Slicing an oversized event into the window would let a search treat a cut
payload as the whole observation. Omitting it whole keeps the incomplete fact
honest.

Defaulting thinking to disclosed would make every existing registration a
disclosure. The opt-in is the decision; the default stays withheld.

## Consequences

Hook authors can match text across chunk boundaries and can see when the window
is not the whole turn. They must opt in before a search can see reasoning, and
a withheld thought still breaks a text sequence. Pre-tool callbacks must return
promptly: they borrow the live record under the evidence lock. Turn-boundary
callbacks do not hold that lock; they pay a copy of the bounded suffix, not of
the omitted prefix.

No history bound changes. No schema version changes. OpenCode is not given a
separate adapter: the shared application path is the one Claude Code and Codex
already use. What remains for the owner is the policy runtime's rule revision,
and whether a hook that requires a complete window should fail closed before
that runtime exists.
