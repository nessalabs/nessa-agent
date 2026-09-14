# Agent runtime

The public Agent owns invocation, queued admission, steering, controls, hooks, and
session evidence through injected provider/storage ports. This slice includes
memory/file storage and test provider substitution. Host composition supplies a
concrete execution provider; no gateway chat RPCs are introduced.

- [Agent and persistence](agent.md)
- [Queueing, steering, and retries](scheduling.md)
- [Invocation hooks](hooks.md)

```text
host -> Agent -> SessionManager -> SessionStorageLease
          |---> invocation queue + hooks
          |---> AgentProvider -> provider implementation
```

Arrows show calls. The domain owns invariants; adapters own external effects.
