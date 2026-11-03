---
id: sdk-history-erasure
title: History erasure and incarnation recovery
kind: statechart
status: implemented
summary: "One retained erase operation identity and incarnation; physical reset
  may precede its reply, and later load/save reconciles reset plus
  retired-content cleanup before access."
parent: sdk
sources:
  - crates/nessa-sdk/src/infrastructure/session_storage/record.rs
  - crates/nessa-sdk/src/application/agent_execution/sessions/storage.rs
model:
  spec: tla/services/sdk/HistoryErasure.tla
  config: tla/services/sdk/HistoryErasure.cfg
diagramLinks: {}
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:39.591Z
  config: tla/services/sdk/HistoryErasure.cfg
  modelDigest: 3935a603235c9ecfc2d89e7802021fb98beb11a0d3b8c9828989a4e9f80d252d
  states: 7
  held:
    - TypeOK
    - AccessAfterErasure
    - AcknowledgedNewIncarnation
    - OneResetIdentity
  broke: []
  unchecked: []
---
One retained erase operation identity and incarnation; physical reset may precede its reply, and later load/save reconciles reset plus retired-content cleanup before access.

Actual SQLite secure deletion, all retired payload rows, event-stream reset implementation and external provider memory are boundaries; no secure-erasure or eventual cleanup guarantee.

```mermaid
stateDiagram-v2
    [*] --> Loaded
    Loaded --> ResetPending: Retain exact erase operation under lease
    ResetPending --> ResetObserved: Durable stream incarnation reset
    ResetObserved --> ResetPending: Reply lost / retain original identity
    ResetObserved --> CleanupPending: Replacement stream reconciled
    ResetPending --> CleanupPending: Retry confirms original reset
    CleanupPending --> Ready: Retired payload cleanup confirms no remaining work
    Ready --> Ready: Later load or save under replacement writer
```

## Model coverage

Direct finite safety model. One retained erase operation identity and incarnation; physical reset may precede its reply, and later load/save reconciles reset plus retired-content cleanup before access.

Actual SQLite secure deletion, all retired payload rows, event-stream reset implementation and external provider memory are boundaries; no secure-erasure or eventual cleanup guarantee. Terminal states may stutter. This scoped review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7` does not claim implementation proof or an executed source test.
