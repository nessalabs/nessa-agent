# Agent execution

The implemented SDK execution slice coordinates an external harness through
application-owned ports behind the public `Agent` entry point. Domain objects protect identities and transitions;
adapters own provider translation, subprocesses, and delivery. Durable conversation
acceptance through a shared event stream, replay, gateway RPCs, and UI wiring remain
separate proposed work. Local snapshots and submission retry recovery are implemented.

| Guide | Responsibility |
| --- | --- |
| [Agent](agent.md) | Entry point, identity, model/operation capabilities, automatic storage, and UI flow |
| [Lifecycle](lifecycle.md) | Live sessions, execution boundaries, cleanup, and provider-context restoration |
| [Scheduling](scheduling.md) | FIFO follow-ups, boundary/native steering, withdrawal, idempotent retries, and retained evidence |
| [Hooks](hooks.md) | Typed before/after invocation callbacks, ordering, and failure semantics |
| [Permissions](permissions.md) | Offered choices, attribution, cancellation causes, and audit evidence |
| [Prompts](prompts.md) | Immutable system instructions, contribution provenance, and new user input |
| [Tools](tools.md) | Immutable sparse updates, observed snapshots, and review input |
| [Transport](transport.md) | ACP dispatch, framing, event delivery, limits, and process cleanup |
| [Claude provider](../claude-acp.md) | Supported native profile, installation, composition, and live verification |
| [Codex provider](../codex-acp.md) | What is Codex's own: ordered session configuration, its read-only preset, and what that preset does not buy |

## Composition and public modules

```text
host --> Agent --> SessionManager --> SessionStorageLease
           |-----> AgentProvider --> ProviderSession --> ACP adapter
           |-----> InvocationQueue --> invocation hooks
           |-----> native steering + optional subscribers
```

Arrows show calls. Agent owns invocation and controls. Composition injects the
provider, manager/storage, and required permission audit sink.
Application feature modules are `agents`, `providers`, `sessions`, `hooks`,
`executions`, `permissions`, and `tools`. Domain features retain their existing
live session, execution, tool, permission, and prompt boundaries.

`ExecutionEvent` and `ExecutionUpdate` are application projections saved in session
snapshots before live publication; they are not domain events. `MessageChunk` is an immutable streamed fragment with a `MessageKind` (text or thought) and compact text storage;
a complete conversation message entity requires transcript identity and lifecycle
that this slice does not own.

See the [design index](../../../../docs/design/agent_execution/README.md) for
implemented ownership and separately labeled proposals. Start source review with
[SDK structure](../../README.md#ddd-layers).
