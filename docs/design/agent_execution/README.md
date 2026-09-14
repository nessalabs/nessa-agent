# Agent execution design

Start with the [implemented SDK guides](../../../crates/nessa-sdk/docs/agent_execution/README.md)
for the current public contract. Design proposals do not authorize implementing
unrelated future systems.

| Document | Status and purpose |
| --- | --- |
| [Ownership and consolidation](consolidation.md) | Implemented ownership, naming decisions, and operational boundaries |
| [Runtime classes and sequences](runtime-classes-and-sequences.md) | Proposed conversation coordinator and durable-event design supplement to ADR 0008 |
| [SDK runtime shape](sdk-shape.md) | Nessa server ownership and proposed conversation delivery contract |
| [Session and stream contracts](../session-and-stream-contracts.md) | Proposed gateway/history delivery contract, broader than the implemented execution binding |
| [Dependency injection](../dependency-injection.md) | Current composition rules shared across repository contexts |

```text
current Agent --> local session snapshots + scheduling --> provider harness
future conversation coordinator --> execution SDK + durable record store
```

Arrows show calls. The first path is implemented. The second remains proposed;
SDK snapshots and scheduling do not implement the proposed shared gateway event stream.
