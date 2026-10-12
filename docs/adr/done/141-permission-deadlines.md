# 141. An unanswered permission settles at a deadline

## Purpose

An agent that is waiting on a person who is not there should keep working.
This decision gives every pending permission a deadline and a default outcome,
so the turn ends that wait on its own and the model is told what happened.
It is the bounded-decision slice of
[0014](../todo/0014-nessa-owned-policy-hooks.md) and of issue 141. Remembered approvals,
notifications, true deferral, and elicitation questions stay on that issue.

- **Date:** 2026-10-11
- **Status:** accepted

## Context

A permission request today waits until a person answers, the provider
withdraws it, or the whole execution's deadline fails the worker. An absent
person therefore stalls the turn. The owner asked for the opposite: when a
tool is blocked, the agent knows and continues.

The pinned Claude harness answers a selected `reject_once` with its own
sentence, "User refused permission to run tool", and ignores `_meta`. A custom
sentence cannot be put in that model's mouth through the permission response.
[0014](../todo/0014-nessa-owned-policy-hooks.md) forbids a steering message standing in
for a permission answer. The reason is carried where Nessa owns the channel:
the audit, the notice after the deadline answers, and `_meta.deadlineReason`
on the response. A harness that reads `_meta` can show it to its model. The
pinned Claude harness (`@agentclientprotocol/claude-agent-acp` 0.76.0) does
not. That is a Claude adapter limit.

The pinned Codex harness (`@agentclientprotocol/codex-acp` 1.12.0) offers two
refusals that both map to ACP `reject_once`. `decline` continues the turn.
`cancel` stops it. File edits, MCP tool approvals, and extra-permission
commands offer only `cancel`. Nessa prefers `decline` when that id is
offered. When `cancel` is the only refusal, the deadline takes it and the
transcript says the agent stopped. That is a Codex limit: the harness does
not offer a refusal that continues.

Deferral — answering a review later, after the provider has moved on — is a
capability the third-party harnesses do not have. Saying "not yet in Nessa"
for a provider that cannot defer describes the wrong owner.

## Decision

A pending permission is armed with the deadline for its class. When that
deadline passes and no answer has been admitted, the deadline selects the
configured outcome. The turn continues, except when the only refusal the
agent offered stops the turn. A person's answer admitted before that wins,
including when the answer and the deadline are both ready in the same wait.

A read or a search is the read-only class only when every target path
canonicalizes inside the session workspace. A path that leaves, including
through `..` or a symlink, a missing path, or no path at all, is the other
class. The bytes of an allowed read are sent to the model provider. Staying
inside the workspace is about what the tool changes, not about the content
remaining on the machine.

The path is checked when the request arrives. The tool reads those bytes
later, as much as eight seconds later when the deadline allows the read. A
path that was inside the workspace at the check can be replaced before that
read.

The standard policy, owned by `PermissionDeadlinePolicy::standard`:

| Class | What lands here | Wait | Outcome | Reason the deadline records |
| --- | --- | --- | --- | --- |
| Read-only | `read` or `search` whose paths all resolve inside the workspace | 8 seconds | allow | No human answered in time. This read-only tool was allowed so the turn could continue. |
| Everything else | any other kind, a read or search outside the workspace, a symlink escape, or no path | 30 seconds | deny | No human answered in time. Continue without this tool or try another approach. |

When the chosen refusal stops the turn, the recorded reason is instead "No
human answered in time. The only refusal this agent offered stops the turn."

Eight seconds is long enough to refuse a sensitive read and short enough that
an absent person does not hold a read-heavy turn. It is not zero: a secret
stays interruptible. Thirty seconds is long enough to read a command at the
desk and short enough that walking away does not freeze the turn. Deny is the
fail-closed outcome for mutation, execution, network, mode changes, a kind
the provider did not name, and a read or search that is not confined to the
workspace. Search is read-only only in that confined case, because it
inspects and does not change those files. Fetch is not: it leaves the
machine. Each profile names which deny option the deadline prefers. Codex
prefers `decline`. A profile that does not say keeps the first deny option
that does not stop the turn.

The policy sits on `AcpConfig` beside the offer policy and the refusal
outcome. Composition installs `standard()`. A wait of zero, a wait longer
than a day, a blank reason, or a reason with a control character is rejected
at construction, as is a deadline installed in the other class's slot. The
restoration fingerprint does not include it: it is host policy for this
open, as a refusal outcome is.

The deadline is its own decider. The audit basis is `deadline`, the actor
principal is `permission-deadline`, and the surface is the tool class. It is
not a person and not a rule.

## Ordering

One pending review is in `waiting` until an answer is admitted or the deadline
has passed with none admitted. These are the orderings that move it. The
first row that matches is what happens.

| What is observed | Result |
| --- | --- |
| A person's answer is admitted, and the deadline has not passed | That answer. Basis `explicit` (or the basis the person supplied). The deadline is disarmed. |
| The deadline has passed, and no answer has been admitted | The deadline's outcome. Basis `deadline`. |
| The person's answer and the deadline are both ready in one wait | The person's answer. The command is taken before the deadline wait. |
| The provider withdraws the review, or the person cancels it, first | That cancellation. The deadline is disarmed. |
| The execution's own deadline passes | The worker fails, as it does today. The permission deadline is a separate wait and does not shorten this one. |
| The configured effect has no offered option, and deny is offered | Deny, with the deny sentence, after the same wait. An allow is never sent as a deny. A missing allow does not reuse the allow sentence. |
| The configured effect has no offered option, and deny is not offered | Cancel the review with `deadline_exceeded` and origin runtime, after the same wait. The transcript still says the deadline denied it. |
| The only refusal offered stops the turn | Deny that option. The reason and the notice say the agent stopped. Codex `cancel` is that option. Codex `decline` is not, and is preferred when both are offered. |
| A deadline's stopping refusal is written, and the provider then reports the turn cancelled | That turn ends with Finished(Cancelled). The session stays open. The cancelled result is the turn ending, because the refusal asked for it. |
| A person selects a refusal that stops the turn, and that selection is written | That turn ends with Finished(Cancelled). The session stays open. Codex ends the turn when `cancel` is chosen; Nessa does not tear the session down. |
| Two deadlines are due in one wait | The earlier deadline settles first. The same instant settles in permission-id order. |
| The turn ends while a review is still pending | The deadline is disarmed and does not answer later. |
| A turn-end frame and a due deadline are both ready | The deadline answers first. It is the earlier branch of the biased wait, and the turn-end frame is still in the socket. The turn end is read after that answer. |
| The provider cancels the turn | The deadline is disarmed, then session teardown with Finished(Cancelled), the same as when nothing is armed. |
| The session closes, or the person stops the turn, while a review is pending | The deadline is disarmed before the shutdown wait. Stopping the turn holds the next prompt, as 132 does. |
| The selected-answer audit fails | The provider is still answered. The audit failure is retained and does not stop the turn. |
| A saved review has no deadline | No automatic decision. The review stays pending until a person answers. |
| A saved deadline is restored | The deadline stays on the record, including whether it would have stopped the turn and when it was due. It is not armed. The card does not promise a decision the new process will not make. Whether this process will decide is not part of the recorded decision, so a live save of an armed review still matches. |
| A saved deadline is not a deadline | The record is corrupt, as any other wrong-typed field is. A blank reason or a wait of zero is skipped, and the review waits for a person. |

The permission wait is its own branch in the worker, after the command branch
and before the provider's next message. It is not folded into the wait that
fails the worker. Because that branch is before the reader, a turn end and a
due deadline that arrive together resolve deadline-first.

## What a person sees

The pending review carries the wait, an absolute `dueAtMs`, the effect,
whether the refusal stops the turn, and the past-tense reason. `dueAtMs` is
the injected clock's unix time plus the wait. It is not a fresh read of the
wall clock when the request arrives. The card does
not show that reason. It says what will happen, from the effect and the time
left until `dueAtMs`: "If nobody answers within 30 seconds, this is denied
automatically." When the only refusal stops the turn, that line says the
agent stops. A reloaded client counts down from `dueAtMs` rather than
starting `waitMs` again. When the deadline answers, the transcript gains a
local notice in the past tense: "Allowed automatically" or "Denied
automatically", plus the reason. The audit records the same past-tense
reason. An app review has no deadline. A restored review keeps the deadline
on the saved record and the projection omits it, so the card never promises
a decision that will not come.

A third-party harness that declares it cannot defer is reported as
`unsupported` ("Not available"). A provider that can defer, or one that has
not said, stays `unsupported_not_implemented` ("Not yet in Nessa"): this
slice does not build deferral.

Session records that carry a deadline or a deadline answer are schema version
3. A version this build does not read is refused. Nothing is migrated.

## Alternatives considered

- **Fail the turn when the wait ends.** That is what an execution deadline
  already does, and it is the stall this decision removes.
- **Treat the deadline as a rule or as a person.** Both would name the wrong
  decider. The basis has a deadline arm for that reason.
- **Steer the reason into the model.** 0014 forbids a steering message
  standing in for a permission answer, and the pinned harness would not be
  reading a permission response anymore.
- **Auto-allow every class.** A command, a write, or a network call would run
  because nobody was at the desk.
- **A wait of zero.** The request would be decided in the instant it appeared,
  and a person could not refuse it.
- **One wait for every class.** A read and a shell command are not the same
  risk, and the owner asked for the read-only class to be the one that can
  proceed.

## Consequences

Turns no longer wait forever on a person. A read or search whose paths stay
inside the workspace can still be refused during its eight seconds, and is
allowed if nobody does. Anything else, including a read outside the
workspace, is refused after thirty seconds if they are not there. The content
of an allowed read is sent to the model provider.

The pinned Claude model sees the harness's own refusal sentence, "User
refused permission to run tool", not the deadline's reason. Nessa still sends
`_meta.deadlineReason`. The adapter ignores it. That is a Claude adapter
limit. The reason is in the audit and the notice.

The pinned Codex harness is answered with `decline` when that option is
offered, and the turn continues. When it offers only `cancel`, the deadline
takes `cancel`, the turn stops, and the notice says so. The session stays
open, and a later turn can start. A person choosing that `cancel` ends the
turn the same way. That is a Codex limit.

Remembered approvals, notifications, deferral on the BYOK harness, and
elicitation questions are not this decision. A restored session does not
re-arm a review whose provider request died with the process. The saved
deadline remains on the record and is not shown as a promise. A review the
live worker is holding is armed, and its card counts down from `dueAtMs`.
