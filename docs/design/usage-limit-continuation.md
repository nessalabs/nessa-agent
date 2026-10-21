# Usage limits and one scheduled continuation

Issue [#1075](https://github.com/nessalabs/nessa-agent/issues/1075). The
boundary is [ADR 1075](../adr/todo/1075-usage-limit-scheduled-continuation.md).
Harnesses: Claude ACP 0.76.0 with Claude Agent SDK 0.3.257, and Codex ACP
1.12.0. OpenCode is not mapped.

## Who decides

| Owner | Decides | Does not decide |
| --- | --- | --- |
| ACP usage-limit classifier | Whether an RPC error or a Claude rate-limit object is a usage limit, and which recovery it is | Whether the gateway retries, and whether a continuation is sent |
| `AgentError::leaves_work_owed` | Whether ordinary resumption may run | What the card says |
| Scheduled-chat-message store | Due time, one row per causal turn, cancellation, delivery receipt | Authorization. `ConversationService::submit` owns that, Cedar included |
| Project preference | Whether this gateway may schedule continuations. Default off | Which account is blocked |

```text
provider frame --> classifier --> UsageLimitReached
                                      |
                    leaves_work_owed? -- no --> no FailureBackoff, no resume_turn
                                      |
                    preference on and future reset?
                                      |
                                      v
                    scheduled store accepts one row (audit in the same transaction)
                                      |
                    due --> submit(stable execution id) --> Cedar inside submit
```

## Wire

Classification reads structured fields. A bare HTTP 429, `-32603` with no
`errorKind` and no SDK prefix, and free diagnostic text do not select this
error. Codex ACP 1.12.0 on this client (no JetBrains session-failure
capability) puts a prompt failure only on the string `usageLimitExceeded`.
`rateLimitExceeded` is a text chunk and `end_turn`, so it is not this error.
`willRetry: true` is not a terminal usage limit.

| Signal | Recovery | Reset |
| --- | --- | --- |
| Codex `error.data.codexErrorInfo == "usageLimitExceeded"` and `willRetry` is not true | `Unknown` | Absent. The bundle keeps `resetsAt` in its process and does not put it on the RPC error |
| Codex `sessionBudgetExceeded`, `contextWindowExceeded`, `serverOverloaded`, `rateLimitExceeded`, object `codexErrorInfo`, or HTTP 429 | Not a usage limit | |
| Claude `error.data.errorKind` `rate_limit` | `Unknown`, or `TimedReset` when this turn observed a rejected rate-limit object that is not a user action | The later rejected instant |
| Claude `errorKind` `billing_error` or `account_on_hold` | `UserAction` | Never copied |
| Claude rate-limit `errorCode` `credits_required`, or `overageDisabledReason` in the credit, seat, member, group, org, or admin set | `UserAction` | Never copied, including `overageResetsAt` |
| Claude `status` or `overageStatus` `rejected`, and not a user action | `TimedReset` when the rejected window's instant parses, else `Unknown` | `resetsAt` for the primary window, `overageResetsAt` for a rejected overage window; the later one wins |
| Claude `status` `allowed` or `allowed_warning` with no rejected overage | Not a pause | |
| Claude internal error (`-32603`), no `errorKind`, diagnostic starts with `You've hit your` or `You've reached your` (SDK 0.3.257) | Same as `rate_limit` | Same merge. This is the SDK's published prefix list for the synthetic frame, not a scan of other prose |
| The other SDK 0.3.257 `USAGE_LIMIT_ERROR_PREFIXES` (`add funds`, `contact your admin`, credits, seat, group, extra usage) | `UserAction` | Never copied |
| Claude `overloaded`, `max_output_tokens` | Not a usage limit | |

`resetsAt` below `1e11` is Unix seconds. `1e11` or more is Unix milliseconds.
An instant is kept only when it falls on 2024-01-01 through 2100-01-01 UTC.
The scheduler then requires it to be after the gateway clock. A past instant
does not schedule.

## State

`leaves_work_owed` is false for every `UsageLimitReached`. That stops the
failed turn itself, and only when its saved result is that error. A turn the
runner stopped behind it has no result of its own, so `failure_owed` would
still resume it and spend a prompt on the same window. The resumption scan
is the one owner of that decision: when the newest turn that actually ran
ended in a blocking usage limit, a later `RunnerStopped` turn is not owed.
A restart cut-off (`Interrupted` / `Suspended`) whose result is not yet that
error stays an ordinary cut-off. One prompt can still happen for that
cut-off; the saved error then blocks the next one.

The schedule row is the durable continuation. The card reads it. Delivery
does not ask whether the conversation exists or was deleted before
`submit_command`: that command already refuses a deleted target through the
access port, and Cedar `conversation.act` runs inside the submit it calls.
The scheduler passes the causal turn's initiation so a grant is re-checked
there, not as the owner.

| State | Event | Next | What is true after |
| --- | --- | --- | --- |
| Running | Saved `UsageLimitReached` | Blocked | That turn is not owed. `RunnerStopped` turns behind it are not owed. Resume budget unchanged |
| Running | Cut off before the error is saved | Owed | Ordinary resume. The next saved usage limit blocks further resumes |
| Running | `rateLimitExceeded` text and `end_turn`, or `willRetry` | Unchanged | Not this error |
| Blocked | Preference off, reset not in the future, or user action | Blocked | No schedule row |
| Blocked | Preference on and `resets_at_ms` is after the gateway clock | Scheduled | One row. Audit `scheduled`, initiator system or the person who enabled the preference. Card may say auto-resuming |
| Scheduled | Same causal turn reports a later future reset | Scheduled | Same row, due time replaced. No second row |
| Scheduled | Preference off, a later invocation exists (settled or not), or agent/model changed | Cancelled | Audit `cancelled`. No submit. A committed model change reconciles immediately |
| Scheduled | Due, preference on, causal turn still the latest usage-limit failure, agent and model match | Delivering | One compare-and-set wins the claim. It may not submit yet |
| Delivering | Cancel, defer, or a model change | Cancelled or scheduled | That wins. `begin_submit` is false. No submit |
| Delivering | `begin_submit` | Submitting | That wins. Cancel, defer, and a model change no longer match this row |
| Submitting | Submit admits or refuses | Delivered or refused | The outcome audit follows. A second pass does not submit again |
| Submitting | Preference off while this process holds the fence | Submitting | The submit finishes |
| Submitting | Preference off and no process holds the fence | Cancelled | A restart does not submit it |
| Submitting | `submit_command` admits the stored execution id | Delivered | Audit `delivered` in a following transaction. One admission |
| Submitting | `submit_command` returns deleted, not found, revoked, or the causal turn is no longer the usage limit | Refused | Audit `refused`. The command's own refusal is the deletion fact |
| Submitting | Submit admitted the turn, then the delivery audit fails | Submitting | No second execution id. The next pass records the audit and does not enqueue again |
| Submitting | Restart, execution id absent from the session | Submitting | The same id is submitted once |
| Submitting | Restart, execution id already saved | Delivered or refused | No second enqueue. Not `resume_turn` |
| Scheduled | Due, but the observed reset is now later | Scheduled | Due time moves. The claim is released. No submit |
| Delivered, cancelled, or refused | Another due scan | Unchanged | No second submit |

A past reset does not schedule. An overdue row that was accepted while the
reset was still in the future is delivered after the process starts. Nothing
promises that minute while the process is down. Two overlapping loops in one
process, or two processes on the file, take the compare-and-set; the loser
does not submit.

A pass claims at most eight due rows. A scheduled row still due at that
clock, or a row left delivering or submitting, is due on the next pass. The loop waits
until the clock for that pass. It does not wait for a separate notification.
A row due later waits until that time. The usage limit on the schedule is the
newest invocation, and only when that invocation is itself the limit. A later
turn with no saved result clears it.

## Invariants

1. `UsageLimitReached` returns `leaves_work_owed() == false`, including
   through nested operation failures that contain it.
2. While the newest turn that ran ended in that error, `owed_turns` is empty
   of that turn and of `RunnerStopped` turns behind it. `resume_turn` queues
   nothing for them.
3. `resets_at_ms` is set only for `TimedReset`, from a parsed absolute
   instant in 2024–2100. User action never carries one.
4. At most one open schedule (`scheduled`, `delivering`, or `submitting`)
   exists per conversation. The open row's execution id is minted once.
5. Only `begin_submit`, the compare-and-set from `delivering` to
   `submitting`, may call `submit_command`. A second scanner does not submit.
6. Schedule and cancellation audits commit with the status change. Delivery
   audit commits after admission. If that audit fails, the row stays
   `submitting` and the next pass records the audit without a new execution id.
7. The card copies "Auto-resuming" only when the open row is `scheduled`,
   `delivering`, or `submitting` and the preference is on.
8. The preference defaults to off. Turning it on walks every catalogue page
   and schedules each currently blocked eligible turn. The newest-first list
   bound does not apply. Turning it off cancels open rows that are not inside
   a submit that has already been fenced.
9. OpenCode frames are not classified as usage limits.
10. Audit initiator for a provider limit is the system. The person is the
    initiator only for the preference change and the cancellations it causes.
11. A published usage limit omits the required-work sentence when every nested
    failure is that usage limit. `ExecutionObservation` of the same limit on
    both sides is that case: the live settlement records the provider error
    twice. A different error, a successful execution beside the limit, storage
    after the turn, or hook failures after it keep the sentence and
    `requiredWorkFailed`.
12. The schedule observes a usage limit only when the newest invocation is
    that limit. [`observed_limit_index`](../../crates/nessa-server/src/conversation/domain/scheduled_message.rs)
    is the owner of that choice. A later invocation clears it, and the plan
    cancels an open row.
13. `next_due` is the owner of the loop's next wait. A scheduled row at or
    before the clock, or any delivering or submitting row, is due now. Only a
    later scheduled time waits.
14. `begin_submit` is the only permission to submit. It moves `delivering` to
    `submitting`, or adopts a `submitting` row this process is not inside.
    Cancel, defer, and a model change match `scheduled` and `delivering` only.

## Card

The transcript card is titled **Usage limit reached**. The checkbox label is
**Auto-continue this project's threads when limits reset**. A schedule that
has been accepted shows **Auto-resuming** plus the local date and time. The
local zone is the environment zone, the same way other desktop labels format
time. The checkbox is the gateway preference, not a per-turn flag.

## Review brief

Review the design note and ADR 1075 before implementation. Base is
`160d994ab1455153d7207a33def78a45cf041c99`. The tree under review is this
branch with only these documents. Intended behavior is the state table above.
Scope is the decision, the state table, and the wire mapping. Do not treat
later code as in scope. Standards: `CODING_STANDARDS.md` gates 1, 12, 13, 15,
and 16, audit evidence, and Cedar-at-delivery. Known non-goals: OpenCode,
forking Codex to expose `resetsAt`, negotiating Claude session-failure
settlement. Unverified: no runtime test has been run; the wire rows are from
the pinned package sources read on 2026-10-20.
