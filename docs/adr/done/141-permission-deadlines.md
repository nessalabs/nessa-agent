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
the audit, the review the person is looking at, and `_meta.deadlineReason` on
the response. A harness that reads `_meta` can show it to its model. The
pinned Claude harness does not.

Deferral — answering a review later, after the provider has moved on — is a
capability the third-party harnesses do not have. Saying "not yet in Nessa"
for a provider that cannot defer describes the wrong owner.

## Decision

A pending permission is armed with the deadline for its tool class. When that
deadline passes and no answer has been admitted, the deadline selects the
configured outcome and the turn continues. A person's answer admitted before
that wins, including when the answer and the deadline are both ready in the
same wait.

The standard policy, owned by `PermissionDeadlinePolicy::standard`:

| Class | Kinds | Wait | Outcome | Reason the deadline records |
| --- | --- | --- | --- | --- |
| Read-only | `read`, `search` | 8 seconds | allow | No human answered in time. This read-only tool was allowed so the turn could continue. |
| Everything else | any other kind, or none | 30 seconds | deny | No human answered in time. Continue without this tool or try another approach. |

Eight seconds is long enough to refuse a sensitive read and short enough that
an absent person does not hold a read-heavy turn. It is not zero: a secret
stays interruptible. Thirty seconds is long enough to read a command at the
desk and short enough that walking away does not freeze the turn. Deny is the
fail-closed outcome for mutation, execution, network, mode changes, and a kind
the provider did not name. Search is read-only because it inspects and does
not change the workspace or leave the machine. Fetch is not: it leaves the
machine.

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
| The configured effect has no offered option, and deny is offered | Deny, with the deny sentence, after the same wait. An allow is never sent as a deny. |
| The configured effect has no offered option, and deny is not offered | Cancel the review with `deadline_exceeded` and origin runtime, after the same wait. The transcript still says the deadline denied it. |
| The selected-answer audit fails | The provider is still answered, and the turn continues. The audit failure is retained. |
| A saved review has no deadline | No automatic decision. The review stays pending until a person answers. |
| A saved deadline is not a deadline | The record is corrupt, as any other wrong-typed field is. |

The permission wait is its own branch in the worker, after the command branch
and before the provider's next message. It is not folded into the wait that
fails the worker.

## What a person sees

The pending review carries the wait, the effect, and the reason. When the
deadline answers, that review leaves the pending list and the transcript gains
a local notice: "Allowed automatically" or "Denied automatically", plus the
reason. An app review has no deadline.

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

Turns no longer wait forever on a person. A sensitive read can still be
refused during its eight seconds; anything else is refused for them after
thirty if they are not there. The pinned Claude model sees the harness's own
refusal sentence, not the deadline's reason. The reason is in the audit, the
notice, and `_meta.deadlineReason`.

Remembered approvals, notifications, deferral on the BYOK harness, and
elicitation questions are not this decision. A restored session does not
re-arm a review whose provider request died with the process; a review the
live worker is holding does.
