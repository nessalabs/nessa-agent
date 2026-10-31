# Agent execution design

Start with the [implemented SDK guides](../../../crates/nessa-sdk/docs/agent_execution/README.md)
for the current public contract. Design proposals do not authorize implementing
unrelated future systems.

| Document | Status and purpose |
| --- | --- |
| [Durable sessions and resume](../durable-sessions-resume.md) | Four-issue implementation plan; #1164 independent, #1003/#762/#934 wait for #981 |
| [Ownership and consolidation](consolidation.md) | Implemented ownership, naming decisions, and operational boundaries |
| [Runtime classes and sequences](runtime-classes-and-sequences.md) | Proposed conversation coordinator and durable-event design supplement to ADR 0008 |
| [SDK runtime shape](sdk-shape.md) | Nessa server ownership and proposed conversation delivery contract |
| [Subagent ownership](../../adr/todo/329-subagents.md) | Proposed ordinary child Agents, durable parent relationships, approval inheritance, recursive closure, recovery and desktop implementation sequence |
| [Session and stream contracts](../session-and-stream-contracts.md) | Proposed gateway/history delivery contract, broader than the implemented execution binding |
| [Dependency injection](../dependency-injection.md) | Current composition rules shared across repository contexts |
| [Streaming message commit](../streaming-message-commit.md) | Implemented local message cadence, timer seam, and write orderings for #294 |
| [Nessa-owned policy hooks](../../adr/todo/0014-nessa-owned-policy-hooks.md) | Proposed decision for hook boundaries, disclosure, failure and attribution; implementation remains tracked by #130 |
| [Invocation window](../../adr/todo/133-hook-invocation-context.md) | Proposed slice of 0014 for #133: a permission-review hook sees every event kind, thinking included, inside the same evaluation budget |
| [Hook capability survey](hook-capabilities.md) | Pinned Claude/Codex/Opencode source evidence, unpinned Kiro observations, and explicit limits on Nessa enforcement |
| [Telemetry](telemetry.md) | Proposed research behind [ADR 195](../../adr/todo/195-tracing-is-the-telemetry-port.md): what exists, the `tracing`-as-port design, its growth costs, and the slices tracked under #195 |
| [GenAI traces](gen-ai-traces.md) | Implemented spans for #898: GenAI semantic-convention attributes derived from events the worker already sees, the `nessa.*` attributes, and the local read API |

```text
current Agent --> local session snapshots + scheduling --> provider harness
future conversation coordinator --> execution SDK + durable record store
```

Arrows show calls. The first path is implemented. The second remains proposed;
SDK snapshots and scheduling do not implement the proposed shared gateway event stream.
