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
| Scheduled-chat-message store | Due time, one row per causal turn, the arm, cancellation, delivery receipt, deletion scrub | Authorization. `ConversationService::submit` owns delivery, Cedar included. The preference change has its own policy |
| Usage-limit preference policy | Whether this caller may change the gateway preference. Facts are the current role, whether the membership is active, and each grant | Whether a continuation is due, and which account is blocked |
| Project preference | Whether this gateway may schedule continuations. Default off. The change and the scan of every catalogue owner are one operation | Which account is blocked |

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

The schedule row is the durable continuation. The card reads the open row.
Delivery does not ask whether the conversation exists or was deleted before
`submit_command`: that command already refuses a deleted target through the
access port, and Cedar `conversation.act` runs inside the submit it calls.
The scheduler passes the causal turn's initiation so a grant is re-checked
there, not as the owner. Deletion of the conversation blanks the prompt and
the identifying fields on every row and writes a new audit; it does not
rewrite the audit rows already stored.

### Statuses

`delivered`, `cancelled`, `refused`, and `unconfirmed` are terminal. They are
not open, `next_due` ignores them, and `open` does not return them. The
partial unique index covers only `scheduled`, `delivering`, and `submitting`,
so a later causal turn can still be accepted after a terminal row.

| Status | What is true |
| --- | --- |
| `scheduled` | Accepted. No submit has been claimed. Cancel, defer, and a model change match it |
| `delivering` | `claim_due` won. Cancel, defer, and a model change can still win. It may not call `submit_command` |
| `submitting` | `begin_submit` committed. Cancel and defer no longer match. At most one live process holds the id in `armed` |
| `delivered` | The execution was saved, or `submit_command` admitted it. Audit `delivered` |
| `refused` | The submit ended for a reason that will not change on retry: deleted, not found, revoked, unavailable after save, or a caller that cannot be rebuilt |
| `cancelled` | The row ended without a submit: preference off, a later turn, an agent or model change, or deletion of a row that had not entered an armed submit |
| `unconfirmed` | The mutation journal recorded `Attempted` and the invocation was not saved. The effect must not run again. Audit `submission_interrupted` |

### Who owns each transition

| From | Event | Owner | To | Durable write |
| --- | --- | --- | --- | --- |
| none | Preference on, future reset, newest invocation is the limit | `plan_continuation`, then `accept` | `scheduled` | The row and its audit, one transaction |
| `scheduled` | Same causal turn, later future reset | `plan_continuation`, then `move_due` | `scheduled` | Due time and audit, one transaction |
| `scheduled` or `delivering` | Preference off, a later invocation, agent or model change | `plan_continuation`, `set_preference`, `cancel_open`, `defer_claim` | `cancelled` or `scheduled` | Status and audit, one transaction |
| `scheduled` | Due, and still the open row | `claim_due` | `delivering` | Status only. The claim is not an audit row |
| `delivering` | `begin_submit` compare-and-set | the store | `submitting` | Status commit. The in-memory arm is inserted only after that commit |
| `submitting` | Execution already saved, or `submit_command` admits | `finish_fenced_submit`, then `record_outcome` | `delivered` | Status and audit, after the observation. The clock is read then |
| `submitting` | `submit_command` returns a terminal refusal, or the caller cannot be rebuilt | `finish_fenced_submit` | `refused` | Status and audit, after the observation |
| `submitting` | The mutation journal returns `Interrupted` | `finish_fenced_submit` | `unconfirmed` | Status and audit `submission_interrupted`. No second submit |
| `submitting` | Preference off, and this process does not hold the arm | `set_preference` or `retire_unfenced_submissions` | `cancelled` | Status and audit |
| `submitting` | Preference off, and this process holds the arm | the in-flight submit | `submitting` | The submit finishes into `delivered`, `refused`, or `unconfirmed` |
| any row that still holds a prompt | Conversation deletion | `erase_conversation` | `cancelled` for `scheduled`, `delivering`, and an unarmed submit; the same status otherwise | Payload blanked and one new audit, one transaction |

The gateway preference is a separate row. `UsageLimitPreferencePolicyEvaluator`
allows or refuses the change before any write. The facts are the caller's
current principal, organization, role, whether the membership is active, and
each grant record. Cedar forbids a member, an inactive membership, and a
caller from another organization. The permit is an active admin whose grants
contain `credential.manage` on this gateway. The socket's grant check is not
this decision.

Turning the preference on and scanning the catalogue are one operation. The
scan asks every catalogue owner, then every page of that owner's
conversations. The catalogue is another database, so the two writes are not
one SQLite transaction. The operation's result is the pair: the preference
and a finished scan, or the previous preference when the scan fails.

### Crash between durable writes

| After this write | Before the next | What is durable | What recovery does |
| --- | --- | --- | --- |
| nothing | `accept` commits | no row | The plan runs again from the saved usage limit |
| `accept` commits | `claim_due` | `scheduled`, with its audit | The next due pass claims it |
| `claim_due` commits | `begin_submit` commits | `delivering`, no claim audit | The next `claim_due` returns the row. `begin_submit` is still required before a submit |
| `begin_submit` commits | the id is inserted in `armed` | `submitting` | The crashed process is gone, so `armed` is empty. The next process adopts the row and submits the same execution id once |
| `begin_submit`'s transaction fails | the arm insert | `delivering`, id not armed | The next `begin_submit` tries the compare-and-set again |
| `submit_command` has not recorded `Attempted` | the invocation is saved | `submitting`, same execution id | Retry `submit_command` with that id. The effect has not been recorded as attempted |
| the journal records `Attempted` | the invocation is saved | `submitting`, journal `Attempted` | The retry returns `Interrupted` and does not run the effect. One audit moves the row to `unconfirmed`. `next_due` stops returning it |
| the invocation is saved | `record_outcome` commits | `submitting`, execution present | The next pass records `delivered` from the saved execution and does not enqueue |
| `record_outcome` fails | its commit | the previous status | The next pass records the same outcome. It does not mint an execution id |
| preference turns on | the owner scan finishes | preference on, and any rows the scan had accepted | If the scan returns an error and the previous value was off, the operation sets the preference back off as the system, which cancels those rows. If it was already on, it stays on and the error is returned. If the process dies during the scan, the next start of the delivery loop scans every owner before it waits |
| preference turns off | an unfenced submit retires | preference off; `scheduled`, `delivering`, and unarmed `submitting` cancelled | An armed submit finishes. A later start cancels a `submitting` row nobody holds |
| `erase_conversation` commits | the tombstone is marked erased | prompt and identifying fields blank, one new audit per row that still held them | A crash before the commit leaves the payload. Deletion stays unfinished and the next attempt scrubs again. A second scrub writes no audit. Older audit rows stay |
| the outcome is observed | the audit timestamp is chosen | the clock is read when the saved execution or the `submit_command` result is in hand | The audit names that observation. A reading taken before the submit is not reused |

The wakeup is not a durable write. The loop creates the notification future
before `deliver_due_scheduled` reads the schedule. A continuation accepted
during that read notifies a waiter that already exists, so the loop does not
sleep through it. `notify_waiters` does not keep a permit for a waiter
created afterwards.

A past reset does not schedule. An overdue row that was accepted while the
reset was still in the future is delivered after the process starts. Nothing
promises that minute while the process is down. Two overlapping loops in one
process, or two processes on the file, take the compare-and-set; the loser
does not submit.

A pass claims at most eight due rows. A scheduled row still due at that
clock, or a row left delivering or submitting, is due on the next pass. The
loop is already waiting on the wakeup it registered before the scan, and a
row due later also waits until that time. The usage limit on the schedule is the
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
   exists per conversation. `unconfirmed` is terminal, not open. The open
   row's execution id is minted once.
5. Only `begin_submit`, the compare-and-set from `delivering` to
   `submitting`, may call `submit_command`. The id enters `armed` only after
   that transaction commits. A failed commit leaves the id unarmed. A second
   scanner does not submit.
6. Schedule and cancellation audits commit with the status change. The
   outcome audit commits after the observation, with the clock read at that
   observation. If that audit fails, the row stays `submitting` and the next
   pass records the audit without a new execution id.
7. The card copies "Auto-resuming" only when the open row is `scheduled`,
   `delivering`, or `submitting` and the preference is on. `unconfirmed` does
   not say auto-resuming.
8. The preference defaults to off. Cedar allows the change before the store
   is written; an active member is refused and the store is unchanged.
   Turning it on scans every catalogue owner and every page, and schedules
   each currently blocked eligible turn. The newest-first list bound does
   not apply. A scan that fails restores the previous off value, as the
   system. Turning it off cancels open rows that are not inside a submit
   that has already been armed. The delivery loop, on start, scans every
   owner again while the preference is on, so a crash during an earlier scan
   is finished.
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
    later scheduled time waits. `unconfirmed` is not due.
14. `begin_submit` is the only permission to submit. It moves `delivering` to
    `submitting`, or adopts a `submitting` row this process is not inside,
    and arms the id only after the commit. Cancel, defer, and a model change
    match `scheduled` and `delivering` only.
15. `MutationFailure::Interrupted` settles the row to `unconfirmed` once.
    The same execution id is not submitted again.
16. Deleting a conversation blanks `prompt` and the identifying fields on
    every schedule row for it, cancels a row that is not inside an armed
    submit, and writes one audit. A second scrub writes nothing. The audit
    table's older rows are not deleted or rewritten.
17. The delivery loop registers its wakeup before it reads the schedule. A
    continuation accepted during that read is waited on by the same pass.

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
