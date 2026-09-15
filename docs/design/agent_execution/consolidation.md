# Agent execution ownership and consolidation

This record describes the implemented consolidation. Start with the
[current SDK guides](../../../crates/nessa-sdk/docs/agent_execution/README.md)
for API examples and behavior.

## Entry point and ownership

`Agent` is the public chat entry point; provider-handle dispatch methods are crate-private. It coordinates session persistence,
invocation hooks, queueing, steering, and the injected provider. The provider
currently owns model calls, tool execution, and model context.

```text
caller -> Agent -> SessionManager -> SessionStorageLease -> storage
            |  +-> InvocationQueue + invocation hooks
            v
       ProviderSession -> ACP worker / profile -> JSON-RPC + process
                               |
                               v
                      execution controller
                               |
                               v
                       ExecutionSession
                               |
                               v
                       ActiveExecution
                         tools + permissions
```

Arrows mean calls or ownership of subordinate state. `ActiveExecution` is private
state inside the session aggregate, not an independently addressable aggregate.
The controller is synchronous; the adapter's serialized worker calls it.

| Concept | Owner and lifetime | Boundary |
| --- | --- | --- |
| Local session and snapshots | `SessionManager`, holding one storage lease | Provider closure does not delete history or release the lease. |
| Queue and submission receipts | `Agent` scheduling | One dispatched invocation at a time; retries retain submission identity and original results. |
| Model capabilities | Shared `effective_capabilities` domain context | Declared model requirements and estimated budgets, not measured provider usage. |
| Operation capabilities | Provider session, exposed by `Agent` | Negotiated steering and restoration support, separate from model facts. |
| Live provider attachment | `ExecutionSession` aggregate | Closure is final for that instance; restoring the provider context constructs a fresh aggregate. |
| Active execution | Private `ActiveExecution` inside the aggregate | Owns tool calls, pending reviews, and seen permission IDs; matching completion releases them together. |
| Tool call | `ToolCall` entity within the active execution | Validates identity and replaces its immutable observation; does not execute or authorize tools. |
| Tool update / observation | Immutable sparse input / accumulated snapshot | Omission means unchanged; an explicitly empty collection clears that field. |
| Permission request | Entity scoped to an execution and tool | Resolves once; its identity stays reserved until execution completion. |
| Review and audit evidence | Application controller and audit ports | Retains exact arguments, decision, cause, and attribution; wire delivery remains a separate fact. |
| Provider session and RPC IDs | ACP adapter | Correlation is limited to the owning context or connection. |

Closing the live aggregate cancels pending reviews with the supplied lifecycle
reason and returns their evidence. It retains the active execution until matching
completion. Answered and cancelled reviews leave the pending map immediately;
`seen_permission_ids` prevents their re-admission for the rest of that execution.
This is replay protection, not an age-based stale-answer threshold.

## Naming decisions

| Name | Meaning |
| --- | --- |
| `ExecutionEventStream` | Single-reader provider observation stream consumed by `Agent`; persistence has separate contracts. |
| `ToolReviewInput` | Validated tool name and complete arguments presented for review. |
| `PermissionOfferPolicy` | Permitted effect/scope choices a request may offer; never an approval or reusable grant. |
| `ExecutionRequest::estimated_input_tokens` | Caller estimate of total context use, including history and tool material; not exact hidden provider occupancy. |
| `ToolCall` and `ToolObservation` | Identity-bearing entity and immutable observed value, respectively. |
| `ActionContext` | Verified attribution shared by approvals, queueing, steering, and explicit closure. |

An `Execution` is one accepted provider invocation and may include several model
calls and tool operations. `Turn` remains product conversation terminology.
There are no compatibility aliases for renamed SDK types.

## Organization

| Layer | Responsibilities |
| --- | --- |
| `domain/agent_execution/` | Sessions protect the live consistency boundary; executions hold identities/outcomes and queue invariants; tools hold entities and immutable values; permissions own choices/transitions; prompts hold attributed instructions. |
| `application/agent_execution/` | Agents coordinate calls; sessions own snapshot/lease contracts; providers own adapter ports; scheduling coordinates admission and receipts; executions project events; permissions preserve attribution/evidence; tools carry review input. |
| `infrastructure/acp/` | Sessions manage connection/restoration, executions supervise the wire loop, and permissions/tools translate messages through the shared profile. |
| `infrastructure/claude_acp/` | Concrete provider composition and supported tool schemas. |
| `infrastructure/session_storage/` | Snapshot encoding and memory/file storage adapters. |
| `tests/{domain,application,infrastructure}/` | Invariants, public orchestration, and storage substitution under matching feature names; ACP contracts share this tree and are included by the library through a test-only path declaration for crate-private controls. |

Effective capability values remain a shared context. ACP, JSON-RPC, and process
supervision remain in this crate; no extra registry, event bus, or repository is
needed for these responsibilities.

## Operational boundaries

Pending permission cancellations are recorded and their wire responses handled
before execution state is released and successful completion is settled. Audit
or delivery failure is surfaced while necessary cleanup still runs. Keep the
terminal-with-pending-review and audit-failure regressions when changing this order.

Resource limits have different units: the queue limits pending input count,
channels limit event count, frames limit bytes, and controller limits cover tool
count, review identities, and retained payload bytes separately. None is a total
session-memory limit. See [scheduling](../../../crates/nessa-sdk/docs/agent_execution/scheduling.md)
and [transport limits](../../../crates/nessa-sdk/docs/agent_execution/transport.md).

The current ACP profile supports its declared text/file tool surface. Test profile
substitution does not claim universal provider compatibility. Request-scoped
permission resolution does not implement reusable grants. Mandatory permission audit
retains answer selection, response-write observations, and cancellation evidence. Local SDK snapshots do
not implement the shared gateway conversation/event contract described in
[ADR 0008](../../adr/todo/0008-agent-client-api.md) and
[ADR 0011](../../adr/todo/0011-nessa-session-protocol-and-authorities.md).
