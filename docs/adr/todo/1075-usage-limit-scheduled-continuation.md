# 1075. A usage limit blocks ordinary retry and continues once, on a schedule

## Purpose

A provider usage window that is exhausted is not a crash. Nessa keeps the
failed turn, stops the ordinary retry loop, and — only when this gateway's
project has asked and a real reset instant exists — sends one continuation
through the same authorized submit path when that instant arrives.

- **Date:** 2026-10-20
- **Status:** proposed

The state table, wire mapping, and invariants live in
[the design note](../../design/usage-limit-continuation.md). This record is
the boundary.

## Context

ACP `error.data` is discarded, so Claude's weekly limit and Codex's
`usageLimitExceeded` both become a generic provider failure. That failure
leaves work owed, and the gateway's backoff then spends the three resume
attempts before the window resets. No conversation names a project, and no
port sends a chat message at a future instant. OpenCode is outside this
harness scope.

## Decision

`AgentError::UsageLimitReached` is the only usage-limit outcome. It does not
leave work owed. The resumption scan also withholds a turn the runner stopped
behind that error, so the queue does not spend a prompt on the same window.
A cut-off whose result is not saved yet stays an ordinary resume. The gateway
does not keep a second copy of that decision.

One scheduled-chat-message store owns a one-shot continuation: due time,
stable execution identity, causal turn, caller, initiation, and cancellation.
Every schedule-row change is one compare-and-set against the token that
plan read: the history generation, the preference revision, and the agent
and model. The same session history is current only while that generation
and that config still match. A toggle completion applies only for the
request that created it. A preference receipt
is not success until its scan settles or its rollback commits; a retry of
an unfinished enable resumes that work. An observed outcome whose audit
does not commit leaves the row `submitting` until a bounded retry; the next
pass records that outcome and does not mint an execution id. A reset instant
is kept through 2100-01-01 UTC, and the card compares that instant on the
gateway's clock, translated the same way as a permission deadline. A
compare-and-set to `delivering` is the only thing that may call
`submit_command`. Cedar and the deleted-target refusal stay inside that
command. Schedule and cancellation audits commit with the status change. The
delivery audit follows admission and, if it fails, leaves the row delivering
so the same execution id is not minted again. The project preference is one
boolean on this gateway, default off. "Auto-resuming" is shown only after
that store accepts the row.

Codex ACP 1.12.0 does not forward cached `resetsAt` to the client. A Codex
usage limit is typed and not retried; its reset stays unknown until the
harness exposes one. Claude's reset is taken from `_claude/rateLimit` when
that object is present and rejected.

## Alternatives considered

- **Leave the turn owed and skip it in the gateway.** The SDK and the gateway
  would both decide retry. Chat-open resumption reads the SDK. The skip loses.
- **Negotiate Claude's session-failure capability.** Capable clients stop
  receiving a JSON-RPC error for every non-auth failure. That rewires
  overload and provider errors to get one usage-limit kind. It loses.
- **Match the diagnostic sentence.** The pinned SDK prefix list is used only
  for the synthetic usage-limit frame that carries no `errorKind`. Arbitrary
  prose and a bare 429 do not classify.
- **Reuse `unfinished_turns`.** That table is the ordinary resume clock. A
  second meaning on `retry_at_ms` would burn the resume budget or hide it.
- **Fork the Codex bundle to emit `resetsAt`.** The pin is a published
  package. Inventing a five-hour window is worse than an honest unknown reset.
- **Default the toggle on, or store it per provider account.** The reference
  checkbox is not a saved preference. Account consent would continue work
  this gateway does not own. This gateway is the project, because a
  conversation has no project id.

## Consequences

Ordinary crashes still retry. A usage window waits. Codex cannot show a real
reset time until its ACP adapter forwards one. The scheduled store is a new
dataset (`user_version` 1) and does not bump conversation metadata. A host
that is off delivers an overdue eligible continuation after it starts; it
does not promise the reset minute while it is down.
