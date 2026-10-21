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
| Scheduled-chat-message store | Due time, one row per causal turn, the submitting lease, cancellation, delivery receipt, the preference decision, deletion scrub | Authorization. `ConversationService::submit` owns delivery, Cedar included. The preference change has its own policy |
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
| `submitting` | `begin_submit` committed. Cancel and defer no longer match. `fence_owner` holds the lease until `fence_until_ms`. `retry_at_ms` is 0, or already past, when the row may be attempted. A later `retry_at_ms` is the next attempt, and the row is not due before it. The in-memory `armed` set only stops this process entering twice |
| `delivered` | The execution was saved, or `submit_command` admitted it. Audit `delivered` |
| `refused` | The submit ended for a reason that will not change on retry: deleted, not found, revoked, unavailable after save, or a caller that cannot be rebuilt |
| `cancelled` | The row ended without a submit: preference off, a later turn, an agent or model change, or deletion of a row whose lease has expired |
| `unconfirmed` | The mutation journal recorded `Attempted` and the invocation was not saved. The effect must not run again. Audit `submission_interrupted` |

### Who owns each transition

| From | Event | Owner | To | Durable write |
| --- | --- | --- | --- | --- |
| none | Preference on, future reset, newest invocation is the limit | `plan_continuation`, then `accept` | `scheduled` | The row and its audit, one transaction, only while the preference revision that read is still enabled |
| `scheduled` | Same causal turn, later future reset | `plan_continuation`, then `move_due` | `scheduled` | Due time and audit, one transaction |
| `scheduled` or `delivering` | Preference off, a later invocation, agent or model change | `plan_continuation`, `set_preference`, `cancel_open`, `defer_claim` | `cancelled` or `scheduled` | Status and audit, one transaction |
| `scheduled` | Due, and still the open row | `claim_due` | `delivering` | Status only. The claim is not an audit row |
| `delivering` | `begin_submit` compare-and-set | the store | `submitting` | Status, `fence_owner`, and `fence_until_ms` in one update. The in-memory arm is inserted only after that commit |
| `submitting` | Lease expired, or this process already owns it and is not inside it | `begin_submit` | `submitting` | One update replaces `fence_owner` and `fence_until_ms`. A live lease owned by someone else matches nothing |
| `submitting` | `renew_fence` succeeds | the owning process | `submitting` | `renew_fence` extends `fence_until_ms` for `fence_owner` only |
| `submitting` | `renew_fence` returns an error | the owning process | `submitting` | The lease stays. Renewal tries again after one second. A `submit_command` already running keeps running |
| `submitting` | `renew_fence` returns false | no longer this process | `submitting`, or the status the owner left | Renewal stops and `lost` is set. `submit_command` does not start. A submit that already ran still records its outcome |
| `submitting` | Execution already saved, or `submit_command` admits | `finish_fenced_submit`, then `record_outcome` | `delivered` | Status and audit, after the observation. The clock is read then |
| `submitting` | `submit_command` returns a terminal refusal, or the caller cannot be rebuilt | `finish_fenced_submit` | `refused` | Status and audit, after the observation |
| `submitting` | The mutation journal returns `Interrupted` | `finish_fenced_submit` | `unconfirmed` | Status and audit `submission_interrupted`. No second submit |
| `submitting` | `submit_command` returns a retryable or unclassified failure, or the command store is absent | `finish_fenced_submit`, then `defer_submit` | `submitting` | `retry_at_ms`, `retry_failures`, and audit `submission_retry`, one transaction. The status stays `submitting`. `begin_submit` does not adopt the row before `retry_at_ms`. A lost lease does not write this retry |
| none | Settlement's plan returns an error, and catch-up is already complete | `consider_usage_limit` | reconcile debt | One `reconcile_retry` row: the conversation, how many attempts have failed, and `retry_at_ms`. The loop retries that plan at that time. A debt the database refuses stays in this process until a later pass writes it or the plan commits. A plan that commits deletes the debt |
| `submitting` | Preference off, and `fence_until_ms` is at or before the clock | `record_preference_decision`, `set_preference`, or `retire_unfenced_submissions` | `cancelled` | Status and audit. A later pass waits until a still-live lease and then cancels it |
| `submitting` | Preference off, and the lease is still live | the owning process | `submitting` | The submit finishes into `delivered`, `refused`, or `unconfirmed` |
| `scheduled` or `delivering` | An applied model change | `commit_applied_model_plan`, from `change_model` or from `recover_model_change` | `cancelled` or `scheduled` | An applied transition returns only after the plan commits, including one recovery finishes from a stopped request. A failed plan stays owed. The next open read or submit runs it again. A refusal is not reconciled. Delivery does not submit while the plan returns an error |
| preference | Cedar allow or deny | `record_preference_decision` | the stored boolean, unchanged on a deny | One audit with the policy ids, in the same transaction as any preference write. A failed audit is an error and leaves the preference as it was. An allow publishes every followed conversation. A deny does not |
| any row that still holds a prompt | Conversation deletion | `erase_conversation` | `cancelled` for `scheduled`, `delivering`, and a `submitting` row whose lease has expired; the same status otherwise | Payload blanked and one new audit, one transaction |

The gateway preference is a separate row. `UsageLimitPreferencePolicyEvaluator`
allows or refuses the change before any write. The facts are the caller's
current principal, organization, role, whether the membership is active, and
each grant record. Cedar forbids a member, an inactive membership, and a
caller from another organization. The permit is an active admin whose grants
contain `credential.manage` on this gateway. The socket's grant check is not
this decision.

Every allow and every deny is one audit row before the command result: the
person, the preference before and after, the policy ids, and the clock.
Allow writes that audit in the same transaction as the preference and the
cancellations it causes. Deny writes only the audit. If the audit does not
commit, the command is an error and the preference is unchanged. An allow
whose stored value did not change is still audited. The system remains the
initiator of a failed-scan rollback and of provider-limit rows; those audits
carry no Cedar policy ids.

An allow publishes every followed conversation, including when the caller
turns the preference off and when a failed scan rolls it back. A deny does
not publish. Subscribers then re-read `usageLimitAutoContinue` and
`autoResuming`.

Turning the preference on and scanning the catalogue are one operation. The
scan asks every catalogue owner, then every page of that owner's
conversations. The catalogue is another database, so the two writes are not
one SQLite transaction. The preference row carries a `revision` and a
`settled` flag. Only the transaction that changes the boolean from off to on
installs an enable: it increments `revision`, leaves `settled` at 0, and
returns that revision as the one this request may roll back. An allow that
finds the preference already on does not bump the revision and installs
nothing. A finished scan settles the revision it observed. A failed scan
reverts only while that same revision is still enabled and unsettled, as the
system, in one compare-and-set. A later request that already settled it is
left on. A direct preference write bumps the revision and settles it, so it
supersedes an unsettled enable.

### Preference change and acceptance

The planner reads the preference and its revision, then accepts. A preference
change can commit in that gap. The card can show the checkbox before the
confirmed view catches up. Both follow this order.

| What has happened | What is still in flight | What is true |
| --- | --- | --- |
| The planner read the preference on at revision R | `accept` has not committed | The insert runs only in a transaction that still sees revision R enabled. A disable, or a later enable, has another revision. The stale accept writes nothing, so a later enable does not deliver a row whose reset is already past |
| An authorized disable committed | A planner that read the previous revision calls `accept` | Open rows from the disable are cancelled. The new insert matches nothing and the preference stays off |
| The person turned the preference on and the command has not finished | The checkbox is pending. The confirmed preference is still off. No row is auto-resuming | The checkbox shows on. The line under the title follows the confirmed preference and says when the limit resets |
| The command has settled | The confirmed preference is not the pending choice, including when another client left it unchanged | The checkbox releases and shows the confirmed value. A later click is accepted. The line still follows the confirmed preference |
| A conversation view is assembled | The schedule store's preference or open read fails | The view is an error. It does not say the preference is off, and it does not drop auto-resuming. No schedule database stays off and has no row |
| The confirmed preference is on, the reset is still ahead, and no row is auto-resuming | Nothing | The line says scheduling failed |

### Crash between durable writes

| After this write | Before the next | What is durable | What recovery does |
| --- | --- | --- | --- |
| nothing | `accept` commits | no row | The plan runs again from the saved usage limit |
| `accept` commits | `claim_due` | `scheduled`, with its audit | The next due pass claims it |
| `claim_due` commits | `begin_submit` commits | `delivering`, no claim audit | The next `claim_due` returns the row. `begin_submit` is still required before a submit |
| `begin_submit` commits | the id is inserted in `armed` | `submitting`, with this process's lease | `armed` is empty after a crash, and the lease is not. Another process submits only after `fence_until_ms`. One update adopts an expired lease |
| `begin_submit`'s transaction fails | the arm insert | `delivering`, id not armed, no lease | The next `begin_submit` tries the compare-and-set again |
| `submit_command` has not recorded `Attempted` | the invocation is saved | `submitting`, same execution id | Retry `submit_command` with that id. The effect has not been recorded as attempted |
| the journal records `Attempted` | the invocation is saved | `submitting`, journal `Attempted` | The retry returns `Interrupted` and does not run the effect. One audit moves the row to `unconfirmed`. `next_due` stops returning it |
| the invocation is saved | `record_outcome` commits | `submitting`, execution present | The next pass records `delivered` from the saved execution and does not enqueue |
| `defer_submit` commits | the process continues | `submitting`, `retry_at_ms` still ahead, audit `submission_retry` | The next pass waits until `retry_at_ms`. It does not submit before then. The same process does not reacquire the row in a tight loop |
| `defer_submit`'s transaction fails | the audit | `submitting`, `retry_at_ms` unchanged | The pass waits one second and tries again. The timestamp and the audit roll back together |
| a settlement plan fails | the reconcile row commits | no schedule change yet; `reconcile_retry` when the write commits | The loop retries that conversation at `retry_at_ms` even though catch-up is complete. A crash drops a debt the database never stored. The next start's catch-up plans every owner. A debt this process kept in memory is written on a later pass |
| `record_outcome` fails | its commit | the previous status | The next pass records the same outcome. It does not mint an execution id |
| preference turns on | the owner scan finishes | preference on at the revision this request installed, unsettled, and any rows the scan had accepted | A scan error reverts that revision only while it is still unsettled, as the system, which cancels those rows. A scan that finishes settles the revision. A request that did not install it does not turn it off. If the process dies during the scan, or the next start's scan fails, catch-up stays pending until a later pass traverses every owner |
| preference turns off | a live lease expires | preference off; `scheduled`, `delivering`, and expired `submitting` cancelled | A live lease finishes its submit. The off path waits until that lease and then cancels it, so a later enable does not deliver it |
| `erase_conversation` commits | the tombstone is marked erased | prompt and identifying fields blank, one new audit per row that still held them | A crash before the commit leaves the payload. Deletion stays unfinished and the next attempt scrubs again. A second scrub writes no audit. Older audit rows stay |
| the outcome is observed | the audit timestamp is chosen | the clock is read when the saved execution or the `submit_command` result is in hand | The audit names that observation. A reading taken before the submit is not reused |
| an applied model change finishes | the continuation plan commits | the new model is durable; an open row may still name the previous model | This process keeps the conversation owed. The next open read or submit runs the plan. A crash drops that memory. Catch-up, while the preference is on, traverses every owner and runs the same plan against the model at that later pass |

The wakeup is not a durable write. The loop creates the notification future
before it reads the preference for the catch-up scan and before
`deliver_due_scheduled` reads the schedule. A continuation accepted during
that read notifies a waiter that already exists, so the loop does not sleep
through it. `notify_waiters` does not keep a permit for a waiter created
afterwards.

### Delivery loop and the lease

Who holds a `submitting` lease, what a transient error does, and when the
loop tries again. `None` from a delivery pass is an unbound store, or a
schedule with nothing due and no live lease. A store error is never that.

| State | Who owns the fence | Transient error | What the pass does | When it tries again |
| --- | --- | --- | --- | --- |
| No open submit | Nobody. `fence_owner` is empty | `preference`, `retire_unfenced_submissions`, `next_live_fence`, `claim_due`, or `next_due` returns an error | The pass waits one second (`SCHEDULE_RETRY_MS`). It does not park until a notification storage recovery will not send | That one second, or sooner when a wakeup fires |
| Preference off, lease still live | `fence_owner` until `fence_until_ms` | `retire_unfenced_submissions` or `next_live_fence` returns an error | Same one-second wait. A successful read waits until the live lease and then cancels it | `SCHEDULE_RETRY_MS`, or until `fence_until_ms` when the read succeeded |
| `submitting`, this process inside `finish_fenced_submit` | This process, while `renew_fence` returns true | `renew_fence` returns an error, such as SQLite busy | Renewal continues. `lost` stays false. `submit_command` is not abandoned | `FENCE_RENEW_RETRY_MS` (one second) until a renew succeeds, then every ten seconds |
| `submitting`, renew reports this process does not own the row | Whoever the store now names, or nobody | `renew_fence` returns false | `lost` is set and renewal stops. `submit_command` does not start. A submit that already ran still records its outcome. This process does not write a retry time on a row it does not own | No further renew from this process. The next due pass, or the process that adopted the expired lease, continues the row |
| `submitting`, retry time still ahead | This process until the lease, then whoever adopts it | A retryable submit failure already wrote `retry_at_ms` | The pass does not claim the row and does not call `submit_command` | `retry_at_ms`. The first failure waits one second. Each later failure doubles that wait, at most one minute. A shorter delivery or catch-up wait wins |
| Reconcile debt, catch-up already complete | The delivery loop | The plan failed at settlement | The debt stays. The log line is not the end of the attempt | `retry_at_ms` on the debt, on the same curve. The loop wakes for that debt. A later successful plan deletes it |
| Enable in progress, this request installed the revision | The decision transaction, then the scan | The catalogue scan fails | Revert that revision only while it is still enabled and unsettled. The audit cause is `preference` and the initiator is the system | The revert is one compare-and-set. A revision a later request already settled matches nothing and stays on |
| Enable in progress, this request did not install the revision | The earlier installer until a scan settles it | This request's scan fails | This request does not turn the preference off. A scan that finishes settles the revision it observed, so the installer's later revert matches nothing | None for this request. The command returns the error. It does not record the scan as finished |
| Catch-up pending, preference on | The delivery loop | `schedule_every_owner` fails, including a catalogue page that cannot be read | Catch-up stays pending. Owners and pages already planned stay planned. The pass does not park on an empty schedule | One second, then double, at most one minute (`CATCH_UP_RETRY_MAX_MS`). A shorter delivery wait wins. The next pass traverses every owner again |

A past reset does not schedule. An overdue row that was accepted while the
reset was still in the future is delivered after the process starts. Nothing
promises that minute while the process is down. Two overlapping loops in one
process, or two processes on the file, take the compare-and-set. A live lease
has one owner. The loser does not submit. The owner renews the lease until
the outcome is recorded. A renew that returns an error is tried again after
one second and does not drop the lease. The lease is thirty seconds, renewed
every ten while the renew succeeds.

A pass claims at most eight due rows. A scheduled row still due at that
clock, a delivering row, or a submitting row whose retry time has arrived,
is due on the next pass. A submitting row whose retry time is still ahead
waits until that time. The loop is already waiting on the wakeup it
registered before the scan, and a row due later also waits until that time. The usage limit on the schedule is the
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
5. Only `begin_submit` may call `submit_command`. It moves `delivering` to
   `submitting` and writes `fence_owner` and `fence_until_ms` in that update,
   or it adopts a `submitting` row whose lease has expired or that this
   process already owns and is not inside. The id enters `armed` only after
   that transaction commits. `armed` is this process's re-entry guard. A
   failed commit leaves the id unarmed and the row `delivering`. A second
   process does not submit while the lease is live. A `renew_fence` error
   does not drop the lease or set `lost`: renewal tries again after one
   second until the store extends it or reports that this process does not
   own the row. `lost` is that false result. It stops a `submit_command`
   that has not started. A submit that has already run still records its
   outcome.
6. Schedule and cancellation audits commit with the status change. The
   outcome audit commits after the observation, with the clock read at that
   observation. If that audit fails, the row stays `submitting` and the next
   pass records the audit without a new execution id. A retryable submit
   failure audits `submission_retry` in the same transaction as
   `retry_at_ms`, with the status staying `submitting`. If that audit fails,
   the timestamp rolls back and the pass waits one second.
7. The card copies "Auto-resuming" only when the open row is `scheduled`,
   `delivering`, or `submitting` and the preference is on. `unconfirmed` does
   not say auto-resuming.
8. The preference defaults to off. Cedar decides the change. The allow and
   the deny are each one audit of the person, the preference before and
   after, and the policy ids, committed before the command result. A failed
   audit is an error and the preference stays as it was. An active member is
   refused, the refusal is that audit, and the store's boolean is unchanged.
   Turning it on scans every catalogue owner and every page, and schedules
   each currently blocked eligible turn. The newest-first list bound does
   not apply. A scan that fails reverts only the unsettled revision this
   request installed, as the system. A scan that finishes settles the revision
   it observed, so a later failure cannot turn that enable off. A request that
   found the preference already on does not revert it. Turning it off cancels
   open rows whose submit lease has expired.
   An allow publishes every followed conversation. A deny does not. The
   delivery loop's catch-up is complete only after a full traversal while the
   preference is on, or when the preference is off. A failed scan, including
   one that already planned earlier owners, leaves catch-up pending. The next
   pass traverses every owner again after a wait that starts at one second
   and doubles up to one minute. A failed preference read leaves catch-up
   pending the same way. The enable command returns a scan error and does not
   record that scan as finished.
9. OpenCode frames are not classified as usage limits.
10. Audit initiator for a provider limit is the system. The person is the
    initiator of the preference decision, allow and deny, and of the
    cancellations that decision causes. The system is the initiator of a
    failed-scan rollback.
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
    before the clock is due now. A delivering row is due now. A submitting
    row is due now only when its retry time has arrived (`retry_at_ms` is 0
    or at or before the clock). A submitting row whose retry time is still
    ahead waits until that time, and `begin_submit` does not adopt it before
    then. The first failed attempt waits one second. Each later failure
    doubles that wait, at most one minute. `unconfirmed` is not due. A
    settlement whose plan did not commit is a reconcile debt with its own
    retry time on that same curve. The loop retries that conversation when
    the time arrives. Catch-up being complete does not erase the debt. A
    debt the database refuses stays in this process until a later pass
    writes it or the plan commits.
14. `begin_submit` is the only permission to submit. It moves `delivering` to
    `submitting` under a new lease, or adopts a `submitting` row whose lease
    has expired, and arms the id only after the commit. Cancel, defer, and a
    model change match `scheduled` and `delivering` only. An applied model
    change is not complete until `apply_usage_limit_plan` commits.
    `commit_applied_model_plan` is the only caller, from the command that
    returns the selection and from recovery of a stopped request. A failed
    plan stays owed. The next open read or submit runs it again, including
    when the request is no longer pending. A refusal is not reconciled.
    Delivery does not submit when that plan fails. A crash drops the owed
    set; catch-up runs the plan again while the preference is on.
15. `MutationFailure::Interrupted` settles the row to `unconfirmed` once.
    The same execution id is not submitted again.
16. Deleting a conversation blanks `prompt` and the identifying fields on
    every schedule row for it, cancels a row that is not inside a live
    submit lease, and writes one audit. A second scrub writes nothing. The
    audit table's older rows are not deleted or rewritten.
17. The delivery loop registers its wakeup before it reads the schedule. A
    continuation accepted during that read is waited on by the same pass.
18. A store error on a delivery pass waits one second and passes again.
    `None` is an unbound store, or a schedule with nothing due and no live
    lease. Catch-up is complete only after every owner has been traversed, or
    when the preference is off. A failed scan leaves it pending and the loop
    retries after a bounded wait, including when nothing is due. A preference
    read that fails leaves catch-up pending; it does not treat the preference
    as off for the life of the process.
19. `accept` writes only while the preference revision the planner read is
    still enabled, in that same transaction. A preference change that commits
    first leaves no new row. The card's line follows the confirmed preference.
    The checkbox may show a pending choice until the command settles. Once it
    has settled, a confirmed value that is not that choice releases the
    checkbox. Scheduling failed is the line only when that confirmed
    preference is on, the reset is still ahead, and no row is auto-resuming.
20. A failed turn may carry `requiredWorkFailed` beside a usage limit or an
    authentication refusal. The turn is failed and the diagnostic is present.
    The client accepts that view. A required-work flag with neither typed
    refusal is rejected.
21. A conversation view that cannot read the schedule store is an error.
    Unbound storage, with no schedule database, is preference off and no open
    row. A failed preference or open read is neither of those.

## Card

The transcript card is titled **Usage limit reached**. The checkbox label is
**Auto-continue this project's threads when limits reset**. A schedule that
has been accepted shows **Auto-resuming** plus the local date and time. The
local zone is the environment zone, the same way other desktop labels format
time. The checkbox is the gateway preference, not a per-turn flag. While a
choice has not landed, the checkbox shows it and the line under the title
keeps the confirmed preference. After the command has settled, a confirmed
value that is not the pending choice releases the checkbox, so another
client's preference is what the next click changes.

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
