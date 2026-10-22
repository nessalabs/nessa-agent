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
An instant is kept when it falls on 2024-01-01 through 2100-01-01 UTC,
both ends included. The scheduler then requires it to be after the gateway
clock. A past instant does not schedule. The card compares that same
gateway instant: the view's `serverNowMs` and the client's receipt time
are the two clocks, and the reset is moved onto the client clock the same
way as a permission deadline. The words under the title still format the
absolute instant.

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
there, not as the owner. Deletion scrubs prompt and payload/grant identifiers. Where a submission fence
captured it, minimal canonical command evidence remains for journal recovery.
The scrub appends a new audit; existing audit rows stay unchanged.

### Command evidence retained through deletion

`begin_submit` stores the command principal, surface, stable request identity and
canonical SHA-256 fingerprint in the same transaction as the submitting fence.
The existing conversation and execution identities supply the target and turn.
The application owns that canonical encoding, shared with ordinary submit.
This immutable evidence retains no prompt, images, file paths, or grant content;
its identity and fingerprint are also retained by the command journal. Deletion
scrubs the payload but leaves submitting rows for leased receipt recovery.

| Ordering | Durable result | Recovery |
| --- | --- | --- |
| Submission fence commits before effect | Canonical non-content evidence and fence commit together | A malformed retained identity rejects store decoding and keeps recovery retryable rather than inventing a binding |
| Deletion scrubs a live or expired submitting row | Payload blank; status and command evidence retained | Adopt the lease and read the original command journal |
| Terminal command receipt committed before deletion or failed outcome audit | Original dispatched/refused result remains authoritative | Record its exact outcome and cause after deletion/reopen |
| Attempted receipt has no terminal after history erasure | Admission remains uncertain | `unconfirmed`, `submission_interrupted` |
| No receipt or Accepted only after history erasure | No confirmed effect remains | `unconfirmed`, `conversation_deleted`; do not submit |
| Command journal cannot be read or is busy | No terminal guess | Bounded retry retains command evidence |
| Outcome audit fails after receipt recovery | Submitting and retained evidence stay | Recover the same original receipt on the next pass |

### Statuses

`delivered`, `refused`, and `unconfirmed` are terminal. They are not open,
`next_due` ignores them, `open` does not return them, and `accept` does not
move them. `cancelled` is not open either, so a later causal turn can still
be accepted beside it. The same causal turn can leave `cancelled` and become
`scheduled` again when `accept` still sees the stale-work token it observed:
the preference revision enabled, the history generation, and the agent and
model. A cancellation for an agent or model change ends that causal
continuation; a fresh scan, restart, preference toggle, or switch back does
not reactivate it. `accept` checks the committed cancellation audit. A later
causal turn remains eligible. The partial unique index covers only `scheduled`, `delivering`, and
`submitting`.

| Status | What is true |
| --- | --- |
| `scheduled` | Accepted. No submit has been claimed. Cancel, defer, and a model change match it |
| `delivering` | `claim_due` won. Cancel, defer, and a model change can still win. It may not call `submit_command` |
| `submitting` | `begin_submit` committed. Cancel and defer no longer match. `fence_owner` holds the lease until `fence_until_ms`. `retry_at_ms` is 0, or already past, when the row may be attempted. A later `retry_at_ms` is the next attempt, and the row is not due before it. The in-memory `armed` set only stops this process entering twice |
| `delivered` | The terminal journal says dispatched, or execution was saved without a terminal refusal. Audit `delivered` |
| `refused` | The submit ended for a reason that will not change on retry: deleted, not found, revoked, unavailable after save, or a caller that cannot be rebuilt |
| `cancelled` | The row ended without a submit: preference off, a later turn, an agent or model change, or deletion before submit. The same causal turn can be scheduled again unless an agent or model change cancelled it. A finished submit cannot |
| `unconfirmed` | Admission cannot be confirmed: the journal recorded `Attempted` without a saved invocation, or deletion erased history and the retained journal has no terminal evidence. The effect must not run again. Audit `submission_interrupted` or `conversation_deleted` |

### Who owns each transition

**If it fails** is the only retry for a submit, an outcome write, reconciliation, and a delivery whose plan does not commit. `next_attempt` is that rule. The first failure waits one second. Each later failure doubles that wait, at most one minute. The durable record is `retry_at_ms`: audit `submission_retry` on the submitting row, or one `reconcile_retry` row for a plan. The attempt is not entered again before that time. When the durable record does not commit, this process keeps the same failure count and `retry_at_ms` and waits it; a later stored row does not replace that later attempt. A lost lease writes nothing. An unread execution, and a `begin_submit` whose transaction does not commit, write no `retry_at_ms` and still keep the attempt while the row is `delivering` or `submitting`. A missed `begin_submit` compare-and-set writes nothing. A committed outcome, a cancelled row, and a missing row drop the attempt this process kept. A status read that fails leaves the attempt. A crash drops what only this process kept. Catch-up, a store read, and lease renewal are not this column.

| From | Event | Owner | To | Durable write | If it fails |
| --- | --- | --- | --- | --- | --- |
| none | Preference on, future reset, newest invocation is the limit | `plan_continuation`, then `accept` | `scheduled` | The row and its audit, one transaction, only while the preference revision that read is still enabled | Not this column. A miss writes nothing |
| `cancelled` | The same causal turn is still the limit, no agent or model change cancelled it, the preference revision is enabled, and the history generation is current | `accept` | `scheduled` | Status, due time, and the planned payload, with the fence and retry cleared, and one audit. The execution id stays. A `submitting` row for the conversation matches nothing. `delivered`, `refused`, and `unconfirmed` match nothing | Not this column. A miss writes nothing |
| `scheduled` | Same causal turn, later future reset | `plan_continuation`, then `move_due` | `scheduled` | Due time and audit, one transaction that still sees the history generation and the preference revision that plan read | Not this column. A miss writes nothing |
| `scheduled` or `delivering` | Preference off, a later invocation, agent or model change, or a different causal turn | `plan_continuation`, `set_preference`, `cancel_open`, `defer_claim`, or `accept` | `cancelled` or `scheduled` | Status and audit, one transaction against that history generation and preference revision. A switch is `accept`: it cancels the open row and inserts the replacement together | Not this column. A miss writes nothing |
| `scheduled` | Due, and still the open row | `claim_due` | `delivering` | Status only. The claim is not an audit row | Not this column |
| `delivering` | The delivery plan does not commit | `deliver_scheduled`, then `note_reconcile_failure` | `delivering` | The row stays `delivering` and is not submitted. `claim_due` and `next_due` leave it until the debt's `retry_at_ms` | `next_attempt` on the reconcile debt |
| `delivering` | `begin_submit` compare-and-set | the store | `submitting` | Status, `fence_owner`, `fence_until_ms`, and immutable canonical command evidence in one transaction. The in-memory arm is inserted only after that commit | A missed compare-and-set writes nothing. A store error is the failure column, kept in this process while the row stays `delivering`. A cancel or a missing row drops that attempt. The row stays `delivering` and unarmed until then |
| `submitting` | Lease expired, or this process already owns it and is not inside it | `begin_submit` | `submitting` | One update replaces `fence_owner` and `fence_until_ms`. A live lease owned by someone else matches nothing | Not this column |
| `submitting` | `renew_fence` succeeds | the owning process | `submitting` | `renew_fence` extends `fence_until_ms` for `fence_owner` only | Not this column |
| `submitting` | `renew_fence` returns an error, and the lease this process wrote is still ahead | the owning process | `submitting` | The lease stays. A `submit_command` already running keeps running | Not this column. Renewal tries again after one second while that lease is ahead |
| `submitting` | The clock reaches the `fence_until_ms` this process wrote, or `renew_fence` returns false | no longer this process | `submitting`, until another process adopts it | Renewal stops and `lost` is set. An expired lease is not extended. `submit_command` does not start. The process that adopts the row records a saved execution without submitting again | Not this column. A submit already running writes no outcome and no retry |
| `submitting` | The saved execution cannot be read | `finish_fenced_submit` | `submitting` | No status, no outcome, and no `retry_at_ms`. The execution may already be saved | `next_attempt`, kept only in this process. The later pass reads again and does not submit |
| `submitting` | Terminal journal says dispatched, or execution already saved without a terminal refusal, or `submit_command` admits | `finish_fenced_submit`, then `record_outcome` | `delivered` | Status and audit, after the observation. The clock is read then | Not this column. A write that does not commit is the outcome row. A committed outcome publishes the conversation |
| `submitting` | Payload was scrubbed by deletion, the saved execution is absent, and retained journal lookup found no receipt or Accepted only | `finish_fenced_submit` after lease adoption and canonical receipt lookup | `unconfirmed` | Audit `conversation_deleted`; no effect is admitted from scrubbed payload | Unread execution retains the submission for bounded retry |
| `submitting` | `submit_command` returns a terminal refusal, or the caller cannot be rebuilt | `finish_fenced_submit` | `refused` | Status and audit, after the observation | Not this column. A write that does not commit is the outcome row. A committed outcome publishes the conversation, including when the conversation itself did not change |
| `submitting` | The mutation journal returns `Interrupted` | `finish_fenced_submit` | `unconfirmed` | Status and audit `submission_interrupted`. No second submit | Not this column. A write that does not commit is the outcome row. A committed outcome publishes the conversation |
| `submitting` | `submit_command` returns a retryable or unclassified failure, or the command store is absent | `finish_fenced_submit`, then `defer_submit` | `submitting` | `retry_at_ms`, `retry_failures`, and audit `submission_retry`, one transaction. The status stays `submitting`. `begin_submit` does not adopt the row before `retry_at_ms` | `next_attempt` on `submission_retry`. A lost lease writes nothing |
| `submitting` | An observed outcome's `record_outcome` does not commit | `finish_fenced_submit`, then `defer_submit` | `submitting` | The status and the outcome audit stay unwritten. The next pass records that same outcome and does not mint an execution id. A missing row writes nothing and does not publish | `next_attempt` on `submission_retry` |
| none | Settlement's plan returns an error, and catch-up is already complete | `consider_usage_limit` | reconcile debt | One `reconcile_retry` row. A plan that commits deletes the debt | `next_attempt` on the reconcile debt |
| `submitting` | Preference off; lease can be adopted | `begin_submit`, then saved-execution and command-receipt recovery | Saved execution or dispatched receipt: `delivered`; attempted receipt without saved execution: `unconfirmed`; confirmed no-effect receipt: terminal `refused` with original `revoked` cause; recovery refusal before any attempt: terminal `refused` with `preference_disabled` cause; absent or merely accepted receipt: seal `Settled(RefusedBeforeAttempt)` under the command lease, then terminal `refused` | Recovery retains command evidence before considering cancellation. The command coordinator serializes retirement with the original submit: a delayed submit observes the saved refusal-before-attempt receipt and cannot enqueue. A command retired this way cannot be rearmed for the same causal turn: its stable command identity is sealed. Earlier scheduled/delivering cancellations remain reversible. Disabling alone does not prove no effect | Unread execution, receipt, or preference leaves `submitting` and waits for bounded retry |
| `submitting` | Preference off, and the lease is still live | the owning process | `submitting` | The submit finishes into `delivered`, `refused`, or `unconfirmed` | Not this column |
| any or no row | An acknowledged applied model request is still pending in metadata, including while the preference is off | `invalidate_usage_limit` | Current causal limit invalidated; an open row cancelled | A per-request receipt records the current causal execution and incarnation, including explicit absence. The token compare, invalidation audit, and open-row cancellation commit together before metadata marks the model request applied; a crash before this write leaves a pending request for recovery. A retry of that request reads its receipt and never invalidates a later limit. `accept` refuses the invalidated causal execution in that incarnation | Model reconciliation stays owed |
| `cancelled` | A fresh scan, restart, preference toggle, or switch back after an agent or model change cancelled this causal turn | `accept` | `cancelled` | The committed cancellation audit refuses reactivation without a row, fence, or audit write. A later causal turn remains eligible | Not this column |
| `scheduled` or `delivering` | An applied model change | `commit_applied_model_plan`, from `change_model` or from `recover_model_change` | `cancelled` or `scheduled` | The plan seals the new agent and model as the next history generation even when the session position does not move, in the same transaction as the row change. A plan that read the previous generation writes nothing. An applied transition returns only after the plan commits, including one recovery finishes from a stopped request. A refusal is not reconciled. Delivery does not submit while the plan returns an error | `next_attempt` on the reconcile debt |
| preference | Cedar allow or deny | `record_preference_decision` | the stored boolean, unchanged on a deny | One audit with the policy ids, and the request's receipt, in the same transaction as any preference write. An enable's receipt is `pending` until the scan settles or the rollback commits. A disable and a deny finish in that transaction. A failed audit leaves the preference and writes no receipt. A retry of a finished request returns the receipt and does not apply again. A retry of `pending` resumes the scan or the rollback. A different input for the same request is a conflict and writes nothing. An allow publishes every followed conversation. A deny does not | Not this column |
| any row that still holds a prompt | Conversation deletion | `erase_conversation` | `cancelled` for `scheduled` and `delivering`; the same status for every `submitting` row, retaining canonical command evidence for receipt recovery | Payload blanked and one new audit, one transaction | Not this column |

The gateway preference is a separate row. `UsageLimitPreferencePolicyEvaluator`
allows or refuses the change before any write. The facts are the caller's
current principal, organization, role, whether the membership is active, and
each grant record. Cedar forbids a member, an inactive membership, and a
caller from another organization. The permit is an active admin whose grants
contain `credential.manage` on this gateway. The socket's grant check is not
this decision.

Every allow and every deny is one audit row before the command result: the
person, the preference before and after, the policy ids, and the clock.
Allow writes that audit in the same transaction as the preference, the
request receipt, and the cancellations it causes. Deny writes the audit and
the receipt. If the audit does not commit, the command is an error, the
preference is unchanged, and there is no receipt. An allow whose stored
value did not change is still audited. The caller's action is checked before
Cedar or any of that write: a blank, oversized, or control-character
request id is rejected. The system remains the initiator of a failed-scan
rollback and of provider-limit rows; those audits carry no Cedar policy ids.
The rollback marks that request's receipt failed in the same transaction.
Until that commit, or until a finished scan settles the revision, the
receipt stays `pending` and a retry resumes the unfinished work. A revert
that does not commit leaves `pending`. Success is not recorded before then.

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
| A toggle completion resolves or rejects | A newer click, or a confirmed view, already superseded that request | The completion applies only while its request is still the pending one. An older resolution does not settle the newer choice, and an older rejection does not clear it. Another click stays blocked |
| A conversation view is assembled | The schedule store's preference or open read fails | The view is an error. It does not say the preference is off, and it does not drop auto-resuming. No schedule database stays off and has no row |
| A conversation view is assembled | The preference could change between two reads | The view reads the boolean and the open row from one snapshot. Off has no auto-resuming row in that snapshot |
| An enable with request A has committed | A later disable with request B has committed, and A is retried | A is one command. The retry returns A's result and does not turn the preference back on. The same request with a different enabled value, or a different person, is a conflict and writes nothing |
| The person turns the preference off, then on, before the reset | The causal row is `cancelled`. The enable scan plans `Accept` | `accept` schedules that row again, so the card does not say scheduling failed. A `delivered`, `refused`, or `unconfirmed` row for that turn stays as it is |
| The confirmed preference is on, the reset is still ahead, and no row is auto-resuming | Nothing | The line says scheduling failed |

### Stale work

A completion applies only when the token it observed is still current. The
token is everything that can make that work stale. For a plan it is the
history generation, the preference revision, and the agent and model, read
before the session and compared in the same transaction as the row change.
The same session incarnation and position are not current by themselves. A
different agent or model, observed at the stored generation, seals the next
generation without a new session position. An older generation writes
nothing, including when the model string matches again. For a toggle the
token is the request that created the pending choice. A miss writes no
status and no audit.

| Work | Token it observed | What makes that work stale | Completion while the token is current | A miss, a replay, or a superseded completion |
| --- | --- | --- | --- | --- |
| Enable | Revision R, generation G, the agent, and the model, plus this request's receipt | A later revision, a later generation, or a different agent or model sealed by a plan this scan did not read | The scan accepts, moves, defers, or cancels against that token. The receipt stays `pending` until the scan settles R or the rollback of this install commits | No row and no audit. A retry of `pending` resumes the scan or the rollback. It does not report success and does not apply a second change. A request that did not install this revision does not turn it off |
| Catch-up | The token this pass reads for each owner | A preference change, a newer history, or a model change that commits during the walk | Every owner is planned against the token read for that conversation. Pages already planned stay planned. The wakeup is registered before the read | A failed page leaves catch-up pending. The next pass traverses every owner again. The preference is not treated as off. A plan that still holds an older generation writes nothing |
| Model change | Generation G and the agent and model now on the conversation. The session incarnation and position may be unchanged | A plan that read an older generation, including the plan that read the previous model, and including that plan after the model changes back | The plan that observed G and a different agent or model stores that config, advances the generation by one, and cancels or replaces the open row in that transaction | The stale plan does not recreate the cancelled row and does not submit it. A later plan that reads the new generation may schedule the config stored then |
| Submit | `fence_owner` and `fence_until_ms` for the lease. The plan token for cancel, defer, move, and accept. A delivery whose plan did not commit also carries that conversation's reconcile retry | A live lease owned by another process, a `submitting` row, a plan token that is no longer current, or a reconcile retry that is still ahead | `begin_submit` moves `delivering` to `submitting` and is the only permission to call `submit_command`. A current plan cancels or defers `scheduled` and `delivering` only. A plan that does not commit records the debt and leaves the row `delivering` | A stale plan does not change a row a newer token kept. `begin_submit` does not enter a live foreign lease. A `submitting` row blocks a reopen of that conversation. A `delivering` row with a future reconcile retry is not claimed and is not due. The wait is the failure column. The plan is not read again before then |
| Outcome-write failure | The execution id, status `submitting`, and this process's `fence_owner` | The lease was lost, or the row is gone | `record_outcome` writes the outcome and its audit, then publishes the conversation. A write that does not commit defers that same outcome: `retry_at_ms` and audit `submission_retry`, and no new execution id. A refusal that did not change the conversation still publishes once the outcome commits, so auto-resuming is re-read | A missed compare-and-set writes no status and no audit and does not publish. A missing row is not a debt and does not publish. A defer that does not commit is the failure column. The later pass records the same outcome |
| Toggle completion | The request that created the pending choice | The confirmed view caught up to that choice, or a newer click issued another request | The completion settles that request. The checkbox shows the pending choice until then. The line under the title follows the confirmed preference | An older resolution does not settle the newer choice. An older rejection does not clear it. Another click stays blocked while the newer request is pending |
| Replay | The receipt's request id. A plan replay reads the token again | The receipt is already `stored`, `refused`, or `failed`. A plan that still holds a generation, revision, or config it did not just read | A `pending` receipt resumes the scan or the rollback. A plan completes only against the token this pass read | A finished receipt is returned and is not applied again. An older generation writes nothing, including when the agent and model strings match the ones now stored |

### Crash between durable writes

| After this write | Before the next | What is durable | What recovery does |
| --- | --- | --- | --- |
| nothing | `accept` commits | no row | The plan runs again from the saved usage limit |
| `accept` commits | `claim_due` | `scheduled`, with its audit | The next due pass claims it |
| `claim_due` commits | the delivery plan commits | `delivering`, no claim audit. `reconcile_retry` when that plan's failure commits | The row is not submitted. The wait is the failure column. A crash drops a debt the database never stored. Catch-up, while the preference is on, plans every owner. A debt this process kept is waited out and written on a later pass |
| `claim_due` commits | `begin_submit` commits | `delivering`, no claim audit | The next `claim_due` returns the row. `begin_submit` is still required before a submit |
| `begin_submit` commits | the id is inserted in `armed` | `submitting`, with this process's lease | `armed` is empty after a crash, and the lease is not. Another process submits only after `fence_until_ms`. One update adopts an expired lease |
| `begin_submit`'s transaction fails | the arm insert | `delivering`, id not armed, no lease. No `retry_at_ms` | The failure column is kept in this process. A crash drops that attempt and the next start tries `begin_submit` again |
| `submit_command` has not recorded `Attempted` | the invocation is saved | `submitting`, same execution id | Retry `submit_command` with that id. The effect has not been recorded as attempted |
| the journal records `Attempted` | the invocation is saved | `submitting`, journal `Attempted` | The retry returns `Interrupted` and does not run the effect. One audit moves the row to `unconfirmed`. `next_due` stops returning it |
| the invocation is saved | `record_outcome` commits | `submitting`, execution present | The next pass records `delivered` from the saved execution and does not enqueue |
| the invocation may be saved | `read_committed` fails | `submitting`, the execution unread. No `retry_at_ms` | The pass does not submit and does not record `delivered`, `refused`, or `unconfirmed`. The failure column is kept in this process. The later pass reads again. A crash drops that attempt and the next start reads immediately |
| `defer_submit` commits | the process continues | `submitting`, `retry_at_ms` still ahead, audit `submission_retry` | The next pass waits until `retry_at_ms`. It does not submit before then. The same process does not reacquire the row in a tight loop |
| `defer_submit`'s transaction fails | the audit | `submitting`, `retry_at_ms` unchanged | The failure column is kept in this process. The timestamp and the audit roll back together |
| a settlement plan fails | the reconcile row commits | no schedule change yet; `reconcile_retry` when the write commits | The loop retries that conversation at the failure column even though catch-up is complete. A crash drops a debt the database never stored. The next start's catch-up plans every owner. A debt this process kept is written on a later pass |
| `record_outcome` fails | `defer_submit` commits, or that write fails too | `submitting`. `retry_at_ms` is still ahead when the defer commits; otherwise the previous retry time | The next pass waits out the failure column, then records the same outcome. It does not mint an execution id. A missing row is not retried |
| preference turns on | the owner scan finishes, or the rollback commits | preference on at the revision this request installed, unsettled, the decision audit, and the request receipt `pending` | A retry resumes the scan. It does not report success and it does not apply a second change. A scan error reverts that revision only while it is still unsettled, as the system, which cancels those rows and marks the receipt failed in that same transaction. A revert that does not commit leaves the receipt `pending`, and the next retry resumes. A scan that finishes settles the revision and only then marks the receipt stored. A request that did not install it does not turn it off; its receipt becomes failed when that write commits, and stays `pending` when the write does not. If the process dies during the scan, or the next start's scan fails, catch-up stays pending until a later pass traverses every owner |
| preference turns off | a live lease expires | preference off; unsubmitted rows cancelled, submitting rows retain command evidence | A live lease finishes its submit. After expiry the off path recovers the original receipt or retires an unattempted command under its command lease |
| `erase_conversation` commits | the tombstone is marked erased | Prompt and delivery/grant identifiers blank; immutable command binding retained for receipt recovery; one new audit per row that still held payload | A crash before the commit leaves the payload. Deletion stays unfinished and the next attempt scrubs again. A second scrub writes no audit. Older audit rows stay |
| the outcome is observed | the audit timestamp is chosen | the clock is read when the saved execution or the `submit_command` result is in hand | The audit names that observation. A reading taken before the submit is not reused |
| an applied model change finishes | the continuation plan commits | the new model is durable; the fence generation has advanced when that plan committed; an open row may still name the previous model until it does | This process keeps the conversation owed. The next open read or submit runs the plan against the token it reads then. A plan that observed the previous generation does not recreate the row, including after the model changes back. A crash drops the owed set. Catch-up, while the preference is on, traverses every owner and runs the same plan |

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
| No open submit | Nobody. `fence_owner` is empty | `preference`, `claim_due`, or `next_due` returns an error | The pass waits one second (`SCHEDULE_RETRY_MS`). It does not park until a notification storage recovery will not send | That one second, or sooner when a wakeup fires |
| Preference off, lease still live | `fence_owner` until `fence_until_ms` | `claim_due` or `next_due` returns an error | Same one-second wait. A successful read waits until the live lease and then recovers its saved execution and command evidence | `SCHEDULE_RETRY_MS`, or until `fence_until_ms` when the read succeeded |
| `submitting`, this process inside `finish_fenced_submit`, lease still ahead | This process, while `renew_fence` returns true and `fence_until_ms` is ahead of the clock | `renew_fence` returns an error, such as SQLite busy, before the deadline | Renewal continues. `lost` stays false. `submit_command` is not abandoned | `FENCE_RENEW_RETRY_MS` (one second) until a renew succeeds, then every ten seconds, and only while the deadline is still ahead |
| `submitting`, the lease this process wrote has expired | Whoever adopts it after `fence_until_ms` | Renew keeps returning an error through the deadline, or `renew_fence` returns false because the deadline has passed | `lost` is set and renewal stops. The expired lease is not extended. `submit_command` does not start. A submit already running does not record an outcome or a retry | No further renew from this process. The next due pass adopts the row. A saved execution is recorded as delivered and is not submitted again |
| `submitting`, renew reports this process does not own the row | Whoever the store now names, or nobody | `renew_fence` returns false | `lost` is set and renewal stops. `submit_command` does not start. A submit already running does not record an outcome. This process does not write a retry time on a row it does not own | No further renew from this process. The next due pass, or the process that adopted the expired lease, continues the row |
| `submitting`, retry time still ahead | This process until the lease, then whoever adopts it | A retryable submit failure, or an observed outcome whose audit did not commit | The pass does not claim the row and does not call `submit_command`. The later pass records the outcome already observed when the execution is saved or the refusal is stable, and does not mint an execution id | The failure column. A shorter delivery or catch-up wait wins |
| `submitting`, another process holds a live lease | `fence_owner` until `fence_until_ms` | `begin_submit` would return false | `claim_due` does not return the row. `next_due` waits until the later of `retry_at_ms` and `fence_until_ms`. The pass does not run again after one millisecond | That deadline. The owner may attempt the row when `retry_at_ms` has arrived |
| Reconcile debt, catch-up already complete | The delivery loop | The plan failed at settlement or at delivery | The debt stays. The log line is not the end of the attempt | The failure column. The loop wakes for that debt. A later successful plan deletes it |
| Enable in progress, this request installed the revision | The decision transaction, then the scan. The receipt is `pending` | The catalogue scan fails, or the revert does not commit | Revert that revision only while it is still enabled and unsettled. The audit cause is `preference` and the initiator is the system. The receipt becomes failed in that transaction. A revert that returns an error leaves the receipt `pending` | The next retry of this request resumes the scan or the revert. It does not report success. A revision a later request already settled matches nothing and stays on |
| Enable in progress, this request did not install the revision | The earlier installer until a scan settles it. This receipt is `pending` | This request's scan fails, or the failure write does not commit | This request does not turn the preference off and does not revert the other revision. Its own receipt is marked failed when that write commits, and stays `pending` when it does not. A scan that finishes settles the revision it observed and then marks this receipt stored, so the installer's later revert matches nothing | A retry while the receipt is `pending` resumes the scan. A retry of the failed receipt returns the failure and does not scan again |
| Catch-up pending, preference on | The delivery loop | `schedule_every_owner` fails, including a catalogue page that cannot be read | Catch-up stays pending. Owners and pages already planned stay planned. The pass does not park on an empty schedule | One second, then double, at most one minute (`CATCH_UP_RETRY_MAX_MS`). A shorter delivery wait wins. The next pass traverses every owner again |

A past reset does not schedule. An overdue row that was accepted while the
reset was still in the future is delivered after the process starts. Nothing
promises that minute while the process is down. Two overlapping loops in one
process, or two processes on the file, take the compare-and-set. A live lease
has one owner. The loser does not submit. The owner renews the lease until
the outcome is recorded or the lease it wrote expires. A renew that returns
an error is tried again after one second only while that deadline is still
ahead. At the deadline, renewal stops. The lease is thirty seconds, renewed
every ten while the renew succeeds and the previous deadline has not passed.

### Lock and lease lifecycle

Four locks order a continuation. Each names one owner, when it expires, what
renews it, and what releases it.

| Lock | Owner | Expires | Renewal | Release | What another writer may do |
| --- | --- | --- | --- | --- | --- |
| Submit lease `fence_owner` / `fence_until_ms` | The process whose `begin_submit` committed | `fence_until_ms`. Thirty seconds, from the clock of that write | `renew_fence` extends it only while `fence_until_ms` is still ahead and `fence_owner` is this process. An error retries after one second until that deadline. At the deadline, `lost` is set and renewal stops. An expired lease is not extended | `record_outcome` only while `fence_owner` is still this process. A missed compare-and-set writes no status and no audit. Adoption is `begin_submit` after expiry. `release_submit` drops only this process's in-memory arm | Wait until `fence_until_ms`. Then adopt and submit, or record a saved execution without submitting again. It does not submit while the lease is live, and it does not record the former owner's outcome |
| Preference revision | The decision transaction that changed the boolean | When a later decision commits a higher revision | An enable that finds the preference already on does not bump it. A finished scan settles the revision it observed | A disable, a direct write, or the revert of an unsettled enable | Accept, cancel, move, defer, and a dismissing hold match only while the revision the planner read is still current, in the same transaction as the row change. Accept also requires that revision to be enabled |
| History generation `history_fence` | The plan that observed `(incarnation, position, generation, agent, model)` | When a later plan that observed the stored `generation` seals a higher position, a different incarnation, or a different agent or model. The stored generation then advances by one | Every plan reads `generation` before the session. Accept, cancel, move, defer, and a hold that dismisses a row compare that generation, the preference revision, and the agent and model in the same transaction as the row change. The revision is compared first, so a miss does not advance the fence. The same incarnation and position stay current only while the generation and the agent and model still match | A seal that observed the generation now stored replaces it and advances the number. A hold that found no row also cancels an open `scheduled` or `delivering` row in that transaction, when the token still matches | A stale plan writes no status and no audit. An older generation writes nothing, including the same incarnation and position and including a model that changed back. A switch does not cancel in one transaction and accept in another |
| Preference request receipt | The request id, in the same transaction as its decision audit | When that receipt is marked `failed`, or when an enable's scan has settled and the receipt is `stored` | A retry of a finished receipt returns it and does not apply again. A retry of `pending` resumes the scan or the rollback | A failed audit writes no receipt. An enable stays `pending` until the scan settles or the rollback commits. A revert or a failure write that does not commit leaves `pending`. Success is not recorded before then | A different enabled value, or a different person, for the same request writes nothing |

The submit lease is the only one with a clock. The other three expire when a newer writer commits, not when time passes. A submit whose lease expires while `submit_command` is still inside it does not record the outcome and does not write a retry time. The adopter owns that row.

The history fence is what acceptance adds to the preference revision and the agent and model. `generation` is a number this store advances, not a clock and not the session incarnation. A plan reads it before the session. A later turn advances `position` inside one incarnation. A reset replaces the incarnation. A model or agent change does neither, and still advances the generation when the plan observed the generation stored then. Any of those replaces the fence only when the plan observed that generation, and the number then advances. An accept, cancel, move, or defer that observed an older generation writes nothing, including when its incarnation and position still match. The row a newer plan already accepted stays scheduled. The row does not stay visible as auto-resuming until its due time. The transitions are the stale-work table.

A pass claims at most eight due rows. A scheduled row still due at that
clock, a delivering row, or a submitting row whose retry time has arrived,
is due on the next pass. A submitting row whose retry time is still ahead
waits until that time. A submitting row another process still leases waits
until that lease's deadline. The loop is already waiting on the wakeup it
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
   exists per conversation. `delivered`, `refused`, and `unconfirmed` are
   terminal. `cancelled` is not open, and the same causal turn can become
   `scheduled` again with the execution id it already has, unless an agent or
   model change cancelled it. That cancellation stays final for this causal
   execution across fresh scans, restart, preference toggles, and switching
   back, enforced by `configuration_cancellation_survives_restart_and_preference_toggles`
   and `an_equal_history_after_a_model_change_does_not_restore_the_cancelled_row`. A finished submit
   cannot. The open row's execution id is minted once.
5. Only `begin_submit` may call `submit_command`. It moves `delivering` to
   `submitting` and writes `fence_owner` and `fence_until_ms` in that update,
   or it adopts a `submitting` row whose lease has expired or that this
   process already owns and is not inside. The id enters `armed` only after
   that transaction commits. `armed` is this process's re-entry guard. A
   failed commit leaves the id unarmed and the row `delivering`. A second
   process does not submit while the lease is live. A `renew_fence` error
   retries after one second only while the `fence_until_ms` this process
   wrote is still ahead. At that deadline, or when `renew_fence` returns
   false, `lost` is set and renewal stops. An expired lease is not extended.
   `lost` stops a `submit_command` that has not started. A submit already
   running does not record an outcome or a retry time. `record_outcome`
   changes a `submitting` row only when `fence_owner` is this process, and a
   missed compare-and-set writes no audit. The process that adopts the row
   records a saved execution without submitting again.
6. Schedule and cancellation audits commit with the status change. The
   outcome audit commits after the observation, with the clock read at that
   observation. If that audit fails, the row stays `submitting` and the next
   pass records the audit without a new execution id. A retryable submit
   failure audits `submission_retry` in the same transaction as
   `retry_at_ms`, with the status staying `submitting`. If that audit fails,
   the timestamp rolls back and the pass waits the failure column.
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
    failed-scan rollback. A model-request invalidation and its cancellations
    retain that request's original organization, person, surface, and request
    id, including recovery from another surface; the recovery caller does not
    replace that initiator.
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
    before the clock is due now. A delivering row is due now, except while
    its conversation owes a reconcile retry that is still ahead: `claim_due`
    does not return it and `next_due` does not report it due. The loop waits
    until that retry, or until another row is sooner. A submitting row is
    due now only when its retry
    time has arrived (`retry_at_ms` is 0 or at or before the clock) and its
    lease has expired or belongs to this process. A live lease owned by
    another process waits until `fence_until_ms`, or until `retry_at_ms`
    when that is later. A submitting row whose retry time is still ahead
    waits until that time, and `begin_submit` does not adopt it before then.
    That wait is the failure column. `unconfirmed` is not due. A settlement
    whose plan did not commit, and a delivery whose plan did not commit,
    are the same reconcile debt on that column. The loop retries that
    conversation when the time arrives and does not claim the delivering row
    before then. Catch-up being complete does not erase the debt. A debt the
    database refuses stays in this process until a later pass writes it or
    the plan commits, and the loop waits the failure column instead of
    reading the plan again. A committed terminal outcome
    publishes the conversation, including a refusal that did not change the
    conversation. A write that does not commit does not publish.
14. `begin_submit` is the only permission to submit. It moves `delivering` to
    `submitting` under a new lease, or adopts a `submitting` row whose lease
    has expired, and arms the id only after the commit. Cancel, defer, and a
    model change match `scheduled` and `delivering` only. An applied model
    change is not complete until `apply_usage_limit_plan` commits.
    `commit_applied_model_plan` is the only caller, from the command that
    returns the selection and from recovery of a stopped request. A failed
    plan stays owed. The next open read or submit runs it again, including
    when the request is no longer pending. A refusal is not reconciled.
    Delivery does not submit when that plan fails. The failure is the
    reconcile debt on the failure column, and the delivering row is not due
    until that retry. A
    crash drops the owed set; catch-up runs the plan again while the
    preference is on. A committed `delivered`, `refused`, or `unconfirmed`
    outcome publishes that conversation so a subscriber re-reads
    auto-resuming. An outcome that does not commit does not publish.
15. `MutationFailure::Interrupted` settles the row to `unconfirmed` once,
    and only after the saved execution was read and was absent. A
    `read_committed` error leaves the row `submitting`. That pass does not
    submit and does not record an outcome or `retry_at_ms`. The failure
    column is kept in this process, and the later pass reads again after
    it. The same execution id is not submitted on that failed read.
16. Deleting a conversation blanks prompt and payload/grant identifiers on
    every schedule row. It retains non-content canonical command correlation
    evidence, cancels unsubmitted rows, leaves submitting rows for recovery,
    and writes one audit. A live submitting row
    retains its status until recovery: a terminal journal result takes precedence
    over saved history; otherwise a saved execution confirms delivery;
    after erasure the original command receipt is recovered before admission is left unconfirmed. A second scrub writes nothing. The
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
19. Every completion in the stale-work table applies only while the token
    it observed is still current, in the same transaction as the write it
    finishes. For a plan that token is the history generation, the preference
    revision, and the agent and model. `accept` writes only while that
    revision is still enabled and the generation, agent, and model it
    observed are still stored. Cancel, move, defer, and a hold that dismisses
    an open row compare the revision first and then the generation and the
    config, and write the row in that same transaction. A different revision
    does not advance the fence. The preference does not have to be on for a
    cancel: the revision the plan read is part of the token. The same
    incarnation and position are current only while the generation and the
    agent and model still match. A later position, a different incarnation,
    or a different agent or model is sealed only when the plan observed the
    generation stored then, and the number then advances. An older generation
    writes nothing, including when the model string matches again. A miss
    writes no status and no audit. A preference change, a newer history, or a
    newer config that commits first leaves the newer row as it is. A switch
    is one `accept`: that transaction cancels the open row and inserts the
    replacement. A plan does not seal the token in one transaction and change
    the row in another. A hold that found no row cancels an open row the
    newer token does not keep.
    The card's line follows the confirmed preference. The checkbox may show
    a pending choice until that request's completion is still current. A
    completion applies only for the request that created the pending choice.
    A confirmed view, or a newer click, supersedes it. An older resolution
    does not settle the newer choice, and an older rejection does not clear
    it. Once the current request has settled, a confirmed value that is not
    that choice releases the checkbox. Scheduling failed is the line only
    when that confirmed preference is on, the reset is still ahead, and no
    row is auto-resuming.
20. A failed turn may carry `requiredWorkFailed` beside a usage limit or an
    authentication refusal. The turn is failed and the diagnostic is present.
    The client accepts that view. A required-work flag with neither typed
    refusal is rejected.
21. A conversation view that cannot read the schedule store is an error.
    Unbound storage, with no schedule database, is preference off and no open
    row. A failed preference or open read is neither of those. The view reads
    the boolean and the open row from one snapshot. Off has no auto-resuming
    row in that snapshot.
22. A preference request is one transition. Its receipt commits in the same
    transaction as the decision audit. An enable's receipt is `pending`
    until the scan settles the revision or the rollback commits. A retry of
    `pending` resumes that scan or that rollback and does not report success.
    A revert, or a write that marks the receipt failed, that does not commit
    leaves `pending`. `stored` is written only after the scan has settled, or
    when a disable or a deny finished in the decision transaction. A retry of
    a finished request returns the receipt and does not apply the change
    again, including an enable retried after a later disable. A different
    input for the same request writes nothing. A blank, oversized, or
    control-character request id is rejected before Cedar and before any
    write. A scan that reverts the enable marks that receipt failed in the
    revert's transaction. A scan that fails for a request that installed
    nothing marks that request's receipt failed when that write commits, and
    does not revert another request's revision. A settle that loses to a
    newer revision marks this request's receipt failed and leaves that
    revision. A retry of the failed receipt returns the failure. The
    continuation prompt is non-blank and no longer than the input bound. A
    saved usage limit is read by `AgentError::usage_limit`. A provider report
    owns that walk when one was saved. Without one, the walk includes every
    aggregate position.
23. An observed outcome whose audit does not commit is the failure column,
    the same retry as a submit that can be tried again. The row stays
    `submitting`. `defer_submit` records `retry_at_ms` and the
    `submission_retry` audit. The pass does not enter the row again before
    that time, and it does not mint an execution id when it records the
    outcome. A missing row is not a debt. A `defer_submit` that does not
    commit keeps the failure column in this process. A compare-and-set that
    does not match writes no status and no audit.
    The card's scheduling-failure line uses the gateway's reset. The absolute
    instant is what the line formats. Whether that instant is still ahead is
    decided on the client clock after the same translation as a permission
    deadline, from the view's `serverNowMs` and the time the view arrived.

## Card

The transcript card is titled **Usage limit reached**. The checkbox label is
**Auto-continue this project's threads when limits reset**. A schedule that
has been accepted shows **Auto-resuming** plus the local date and time. The
local zone is the environment zone, the same way other desktop labels format
time. Whether the reset is still ahead is the gateway's clock, translated
onto the client the same way as a permission deadline. A client clock that
disagrees with the gateway does not move that decision. The checkbox is the gateway preference, not a per-turn flag. While a
choice has not landed, the checkbox shows it and the line under the title
keeps the confirmed preference. After the command has settled, a confirmed
value that is not the pending choice releases the checkbox, so another
client's preference is what the next click changes. A completion is the
request that created the pending choice. A confirmed view or a newer click
supersedes that request. An older resolution or rejection does not change
the choice that replaced it.

Both gateway adapters complete a preference command only after their existing
subscription owners have applied a fresh snapshot for every conversation
followed when the RPC succeeds. The RPC receipt is not a conversation frame.
Reopening omits the old cursor and fences callbacks from the previous open.
With no followed conversation, there is no visible projection to reconcile.

| Command outcome | Subscription outcome | Completion and display |
| --- | --- | --- |
| RPC rejects | Any | Reject through the existing typed control failure; release this pending choice. |
| RPC succeeds | Fresh snapshot applied | Resolve after publication; display the snapshot's preference. |
| RPC succeeds | Fresh snapshot says the opposite choice | Resolve after publication; preserve the newer external winner. |
| RPC succeeds | Refresh fails or its owner stops | Reject through the existing typed failure; release this pending choice. |
| Any | Callback from an older open | Ignore it; it cannot publish or settle the current refresh. |

The adapters do not synthesize a view from the requested value or wait for
that value to match. Request tokens on the card continue to fence older
command completions from a newer pending choice.
While a choice is pending, the native checkbox is disabled to reflect its
existing admission guard. Confirmation, current completion, or current
failure releases it; an older completion cannot enable a newer pending choice.

## Initial design review (2026-10-20)

This records the original documents-only review. Current implementation scope
and verification are tracked in [PR 1076](https://github.com/nessalabs/nessa-agent/pull/1076).

Review the design note and ADR 1075 before implementation. Base is
`160d994ab1455153d7207a33def78a45cf041c99`. The tree under review is this
branch with only these documents. Intended behavior is the state table above.
Scope is the decision, the state table, and the wire mapping. Do not treat
later code as in scope. Standards: `CODING_STANDARDS.md` gates 1, 12, 13, 15,
and 16, audit evidence, and Cedar-at-delivery. Known non-goals: OpenCode,
forking Codex to expose `resetsAt`, negotiating Claude session-failure
settlement. Unverified: no runtime test has been run; the wire rows are from
the pinned package sources read on 2026-10-20.
