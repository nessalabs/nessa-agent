---
id: sdk-child-preparation
title: Child preparation and initial task
kind: statechart
status: implemented
summary: "One exact child spawn binding: reservation audit, factory preparation,
  durable Ready distinct from task admission, and ancestor seal before new
  effects."
parent: sdk
sources:
  - crates/nessa-sdk/src/application/agent_execution/subagents/coordinator.rs
  - crates/nessa-sdk/src/domain/agent_execution/subagents/graph.rs
  - crates/nessa-sdk/tests/application/agent_execution/subagents/publication.rs
  - crates/nessa-sdk/tests/application/agent_execution/subagents/preparation.rs
diagramLinks: {}
model:
  spec: tla/services/sdk/ChildPreparation.tla
  config: tla/services/sdk/ChildPreparation.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:36.856Z
  config: tla/services/sdk/ChildPreparation.cfg
  modelDigest: a49bcc048047fd4f5676eebb8e39fad5fa7e278622a9f899274abf76ab8ad1c9
  states: 18
  held:
    - TypeOK
    - FactoryAfterAudit
    - TaskNeedsReady
    - ReadyDurable
  broke: []
  unchecked: []
---
One exact child spawn binding: reservation audit, factory preparation, durable Ready distinct from task admission, and ancestor seal before new effects.

Task/request identity equality, capacity, inherited policy, native provider state and recovery of retained output are source contracts. Child cleanup joins sdk-ownership; no uncertain initial submit replay is permitted.

```mermaid
stateDiagram-v2
    [*] --> Reserved
    Reserved --> Prepared: Reservation audit acknowledged and ancestor open
    Prepared --> Ready: Prepared snapshot durably acknowledged
    Ready --> TaskAdmitted: Initial task admitted while ancestor open
    TaskAdmitted --> Uncertain: Outcome not confirmed
    Reserved --> Closing: Ancestor sealed
    Prepared --> Closing: Ancestor sealed
    Ready --> Closing: Ancestor sealed
    Uncertain --> Closing: Drain original owner without replay
```

## Model coverage

Direct model. One exact child spawn binding: reservation audit, factory preparation, durable Ready distinct from task admission, and ancestor seal before new effects.

Task/request identity equality, capacity, inherited policy, native provider state and recovery of retained output are source contracts. Child cleanup joins sdk-ownership; no uncertain initial submit replay is permitted. Safety only; terminal stuttering is allowed and there is no liveness or implementation-proof claim. This scoped source review adds the abstraction at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints remain unchanged. Source tests were inspected, not executed.
