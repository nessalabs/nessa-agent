# Invocation hooks

Register callbacks on `Agent`, independent of its selected provider:

```rust,ignore
agent.add_hook(BeforeInvocation, |context: &InvocationContext<'_>| {
    // Inspect the exact submitted input; return an error to reject this attempt.
    Ok(())
});
agent.add_hook(AfterInvocation, |event: &AfterInvocationEvent<'_>| {
    // event.context identifies the input; event.result retains its settlement.
    Ok(())
});
```

For a cohesive extension that implements both callbacks, implement
`InvocationHook` and register it with `agent.add_invocation_hook(Arc::new(hook))`.
`InvocationHooks` is the immutable registration snapshot used internally for one
invocation. `Agent` takes that snapshot before dispatch; new registrations apply
to subsequent invocations. No mutable global registry or untyped event bus exists.

```text
Agent::invoke -> capability validation -> save submitted input
    -> before hooks -> provider preparation/execution + observation persistence
    -> save settlement -> after hooks -> result
```

Arrows show call order. The hooks are application extension points around an
invocation attempt. They are not domain events or provider model/tool-step hooks.

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
- Unwinding callback panics become `HookError::Panicked`. An aborting process cannot
  be recovered. Callbacks must return promptly: no blocking I/O, waiting for agent
  work, or synchronously starting another invocation. Notify an independently
  owned coordinator through a nonblocking port when asynchronous work is needed.
- Hooks receive borrowed input/results. They cannot mutate domain aggregates,
  replace validated input, grant permission, or convert failure into success through
  the hook interface. Composition must register trusted callbacks because they can
  inspect sensitive input.
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
