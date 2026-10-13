# Invocation hooks

Register callbacks on `Agent`, independent of its selected provider:

```rust,ignore
agent.add_hook(BeforeInvocation, |context: &InvocationContext<'_>| {
    // Inspect the exact submitted input; return an error to reject this attempt.
    Ok(())
});
agent.add_hook(AfterInvocation, |event: &AfterInvocationEvent<'_>| {
    // event.context identifies the input; event.result retains its settlement.
    // event.context.observations is the turn's bounded window.
    Ok(())
});
agent.add_hook(BeforePermissionReview::new(), |call: &ProposedToolCall<'_>, observations: &InvocationObservationWindow<'_>| {
    // `call` borrows the review under judgement.
    // `observations` is the invocation prefix already retained, every kind included.
    let _ = (call.name(), observations.contains_phrase("do not run"));
    Ok(ToolVerdict::Allow)
});
```

`ObservationKind::of` tells thinking, model text, a tool call, and a tool result
apart on each event the window yields. Thinking is visible and spends the same
budget as any other event. A thought does not join neighboring model text.

For a cohesive extension that implements both callbacks, implement
`InvocationHook` and register it with `agent.add_invocation_hook(Arc::new(hook))`.
`InvocationHooks` is the immutable registration snapshot used internally for one
invocation. `Agent` takes that snapshot before dispatch; new registrations apply
to subsequent invocations. No mutable global registry or untyped event bus exists.

```text
Agent::invoke -> capability validation -> save submitted input
    -> before hooks -> provider preparation/execution
    -> permission-gated review -> permission-review hooks -> retain that review
    -> verdict -> save settlement -> after hooks -> result
```

Arrows show call order. The hooks are application extension points around an
invocation attempt. They are not domain events or provider model/tool-step hooks.
Claude Code and Codex reach the permission-review gate through the permission
exchange this application already observes. A tool the harness runs without
asking is not offered to that hook.

`InvocationContext::observations` and the permission-review `observations`
argument are the same `InvocationObservationWindow`. It is a read-only suffix of
`InvocationRecord.events`, capped at 256 events and 64 KiB of payload per
evaluation ([ADR 0014](../../../../docs/adr/todo/0014-nessa-owned-policy-hooks.md),
[ADR 133](../../../../docs/adr/todo/133-hook-invocation-context.md)). Those caps
do not change how much history the session retains. Thinking spends the budget.
An event larger than 64 KiB is omitted whole, and the walk continues past it.
That event still spends one of the 256 event slots, so the walk never sizes or
copies more than 256 events, and sizing a tool event stops once it passes
64 KiB.
The window reports an omitted prefix, omitted text, a cut text sequence, and an
oversized omission. Only the newest 256 omitted events are checked for text; a
longer omitted prefix counts as omitted text. `contains_phrase` treats adjacent chunks of the same kind
and message identity as one sequence, so a phrase split across chunk boundaries
still matches. Messages that both lack an id share that identity. A miss is
`Absent` only when no text was left out; otherwise it is `Inconclusive`. A hook
that needs the whole turn treats `Inconclusive`, and `!is_complete()`, as a no.

The suffix is copied and the evidence lock is released before the callback. A
permission-review hook runs on a blocking thread until the session's
message-commit clock advances two seconds, and it yields when stop arrives.
The hook thread reads stop again as it starts, so a stop sent before the
callback begins means it never runs, however long the thread took to start.
Dropping that wait does not cancel the callback: the blocking task cannot be
cancelled, so a hook that overruns keeps its thread until it returns, its late
verdict is discarded, and it must not re-enter the session. A verdict first
seen after the deadline passed is a timeout, even if the hook had returned. One process runs
at most 32 of those threads. A review that arrives while that budget is held,
or while this agent's earlier hook thread has not returned, is refused
immediately as `"hook failed: busy"`, without another blocking thread.
A hook reviews permissions unless its `reviews_permissions` returns `false`,
as the typed `BeforeInvocation` and `AfterInvocation` registrations do. A hook
that opts out is never called for a review, even beside one that reviews. A
hook whose `reviews_permissions` panics takes part, and each of its reviews is
`"hook failed: panic"`. When
no hook in the invocation reviews permissions, a review goes to the person
exactly as it would with no hooks: no hook thread runs, so no busy or timeout
refusal can deny it. Message text that is still waiting for its commit
deadline is saved before a review hook runs, so the hook's wait does not
delay that save.
`ToolVerdict::Allow` leaves the review for the provider's ordinary answer.
`ToolVerdict::Stopped` means the turn is already stopping: the review is
retained, not published, and not answered. A refused review is also retained
and not published: the hook has already answered or withdrawn it, so there is
nothing left for a person to answer. `ToolVerdict::refuse(_, Continue)` keeps the
review and answers the provider's deny option as the hook
(`permission-review-hook`, `ApprovalBasis::Hook`), so the agent continues. The
basis keeps the registration index when a hook returned, the reason capped
at 1024 bytes, and the outcome that was applied. `ToolVerdict::refuse` applies
that cap when the verdict is built, so no copy of a refusal holds more. A
timeout or a busy refusal has no index. A continue with no deny option is an end-turn when
the binding can cancel one turn, and continue when it cannot. The review is
withdrawn as the hook either way, so the provider is not left waiting.
`ToolVerdict::refuse(_, EndTurn)` withdraws the pending review as the hook,
naming the outcome, and then cancels the turn when the binding can. A turn
the provider already finished while the hook ran, shown by its reply having
arrived, is not cancelled again. A turn that cleanup retired without a reply
is not finished: the session still stops on the failure path. The answer,
withdrawal and cancel run while the execution is still polled, as saves do,
so a provider that completes them through its execution cannot deadlock. The
hook thread receives only the proposed call, not the whole review. A caller
cannot write a `hook-refusal.` code: `CustomPermissionCancellationReason::new`
cannot build one, and the session withdraws with one only the review, in the
session and turn, the runner issued it for. Turn and review ids can repeat in
another session, so the binding names the session too. A copy taken from
recorded evidence, or a saved one, withdraws nothing, whichever path submits it. A withdrawal with a hook-refusal code, live
or loaded from storage, must name one of the three outcomes and carry the hook's
own actor for that review, or it is rejected. Only a
continue is ever an answer, and the execution controller rejects a hook answer
with any other outcome, so no answer carries a terminal outcome to replay.
An end-turn the binding cannot perform is recorded as continue and does not
cancel the turn. It does not close the session. The answer, the withdrawal,
and the turn cancel use the agent's control path. If the answer or the
withdrawal fails, the answer or withdrawal still records the outcome that was
intended, and the turn is cancelled. The session stays usable, but the
invocation returns `AgentError::ExecutionObservation` carrying that failure
beside the provider result. Like any failed invocation, that stops queued
work behind it. If the turn cancel fails too, both failures are
returned, delivery first, and the session stops on the observation-failure
path. A withdrawal's cancellation code names that
outcome: `hook-refusal.continue`, `hook-refusal.end_turn`, or
`hook-refusal.close_session`. `ToolVerdict::refuse(_, CloseSession)` withdraws the review as the hook, recording
`hook-refusal.close_session` and the hook's reason, then closes with
`SessionCloseRequest::PolicyRefusal`. It does not select an offered option.
A hook that returns `Stopped` is deny-and-continue; only the runner uses
`Stopped`, when the turn is already stopping. A hook error, panic, timeout,
or busy guard is deny-and-continue with the reason
`"hook failed: error"`, `"hook failed: panic"`, `"hook failed: timeout"`, or
`"hook failed: busy"`.
An incomplete window is reported to the hook. It does not by itself reject the tool.

- Capability rejection runs no callbacks. A before failure stops the remaining
  before callbacks and prevents provider dispatch. No after callbacks then run;
  the rejected attempt and failure are retained by the manager.
- After callbacks run in registration order and receive the same original result,
  including provider preparation/execution, observation persistence, audit, and
  cleanup failures. Input validation or initial admission-save failure happens
  before callbacks and runs no after hook. Every
  after callback runs even if an earlier callback fails. `AfterInvocationHooks`
  retains the underlying result plus all callback failures and registration indices.
  Direct `invoke` returns this wrapper after saving the backend settlement; its
  final hook error is not automatically saved as the invocation result. Queued
  invocation receipts retain the final hook outcome through scheduling persistence.
  Recovery follows the [submission retry contract](scheduling.md#idempotent-submission-retries).
- Unwinding callback panics on the permission-review gate become deny-and-continue.
  A before-invocation panic is `HookError::Panicked` and rejects the attempt.
  An aborting process cannot be recovered. Callbacks must return promptly: no
  blocking I/O, waiting for agent work, or synchronously starting another
  invocation. Notify an independently owned coordinator through a nonblocking
  port when asynchronous work is needed.
- Hooks receive borrowed input/results. They cannot mutate domain aggregates,
  replace validated input, grant permission, or convert failure into success through
  the hook interface. Composition must register trusted callbacks because they can
  inspect sensitive input, including thinking.
- Dropping an unpolled direct `invoke` runs nothing. Once its first poll acquires
  the invocation slot, Agent supervises admission, provider execution, observation
  persistence, and after callbacks independently of the waiting caller. Dropping
  that waiter does not cancel the work or skip callbacks. Keep the Tokio runtime
  alive until settlement or awaited close completes. After callbacks are not
  durable notifications and do not replace the required permission audit.
- Subscribers may still have queued output when invocation returns. Streaming text
  is published immediately; tool/review/terminal and settlement boundaries save it.
  A process failure can lose unfinished text. Consequential updates are saved
  before publication; hooks do not depend on a UI reader.

The SDK queue dispatches through the same before/after invocation hooks. Queue
order, bounded admission, attribution, and cancellation are protected by the
scheduler and retained storage evidence; optional callbacks cannot disable those
rules. Register hooks before admitting work when every dispatched invocation must
see them. The hook list is captured at dispatch, not at enqueue time.

Native steering adds input to an existing invocation and does not emit a second
before/after invocation pair. Its delivery acknowledgement is retained separately
from the original invocation's completion. These SDK callbacks do not run inside
an external provider's model/tool loop. Provider hook APIs require a supported
transport or in-process integration; ACP update notifications alone are not
interception hooks. See [scheduling](scheduling.md).
