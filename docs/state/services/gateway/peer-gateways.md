---
id: gateway-peer-gateways
title: Enroll, read and forget peer gateways
kind: operation
status: implemented
summary: Peer commands isolate one peer read while preserving unrelated gateway
  activity.
parent: gateway
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - crates/nessa-server/src/peer_gateways/infrastructure/commands.rs
  - crates/nessa-server/tests/peer_gateways/commands.rs
  - crates/nessa-server/src/peer_gateways/infrastructure/poller.rs
  - docs/design/auth/peer-gateways.md
model:
  spec: tla/services/gateway/PeerGateways.tla
  config: tla/services/gateway/PeerGateways.cfg
  steps:
    Init: Begin this bounded operation.
    IntentFails: Refuse without command admission when intent audit fails.
    BusyAuditFails: Preserve unconfirmed outcome audit for a Busy refusal.
    Acquire: Acquire the record-set command turn before preempting a peer read.
    Busy: Return Busy before interrupting any read when another command owns the set.
    Preempt: Stop only the selected peer cycle and wait for its turn.
    Own: Take the selected peer turn after its read yields.
    Intent: Acknowledge intent before acquiring the record-set turn or interrupting
      reads.
    Change: Apply enrollment, relay or forget through the owned record set.
    Complete: Acknowledge the outcome before returning successful command completion.
    AuditFails: Retain an effect already applied when its outcome audit cannot be
      confirmed.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:00.542Z
  config: tla/services/gateway/PeerGateways.cfg
  modelDigest: 87734a0a5d2c33f37cf806914eab31eb5ff86381378115867881d37046f4fdb4
  states: 11
  held:
    - TypeOK
    - MutationHasIntent
    - SuccessHasBothRecords
    - OtherPeerUninterrupted
    - CommandCannotRaceSelectedRead
    - PreemptionNeedsIntent
  broke: []
  unchecked: []
---
# Enroll, read and forget peer gateways

A gateway can enroll into another gateway invitation, retain the peer identity, read its granted records, change its relay or forget it. Product admission asks Cedar before the command. Commands serialize record-set changes and stop only the selected peer read; unrelated peer cycles keep running.

Intent audit precedes the command turn, including its Busy check and peer-read preemption. A successful reply also waits for outcome audit. If audit fails after mutation, the effect can already stand and the list is the recovery view. Pollers queue change evidence under their peer turn, then release that turn before waiting for its audit acknowledgement.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Requested: intent audit acknowledged
    Idle --> Refused: intent audit fails
    Requested --> SetHeld: command turn acquired
    Requested --> Refused: busy and outcome recorded
    SetHeld --> Preempting: stop selected read
    Preempting --> Owned: selected turn yielded
    Owned --> Changed: intent acknowledged then mutate
    Changed --> Done: outcome acknowledged
    Changed --> Unconfirmed: effect stands but audit fails
```

## Bounded rule model

One command, its selected peer read and an unrelated peer cycle model record-set admission, preemption and audit ordering. The changed flag records an applied effect independently of successful acknowledgement.

The configured safety rules are `MutationHasIntent`, `SuccessHasBothRecords`, `OtherPeerUninterrupted`, `CommandCannotRaceSelectedRead`, `PreemptionNeedsIntent`. TLC explores the authored abstraction; it does not execute or prove the implementation.

| Model action | Source behavior abstracted |
| --- | --- |
| `Acquire` | Acquire the record-set command turn before preempting a peer read. |
| `Busy` | Return Busy before interrupting any read when another command owns the set. |
| `Preempt` | Stop only the selected peer cycle and wait for its turn. |
| `Own` | Take the selected peer turn after its read yields. |
| `Intent` | Acknowledge intent before taking the record-set turn or interrupting any read. |
| `Change` | Apply enrollment, relay or forget through the owned record set. |
| `Complete` | Acknowledge the outcome before returning successful command completion. |
| `AuditFails` | Retain an effect already applied when its outcome audit cannot be confirmed. |

## Verification limits

Enrollment cryptography, deadline clocks, per-peer cached stream cursors, credential rotation, relay races and filesystem durability are abstracted. This does not model every poller recovery path or establish current remote authority. The cited code and regression sources were inspected at the page checkpoint; their product test suites were not run in this modeling pass.

## Further reading

[commands.rs](../../../../crates/nessa-server/src/peer_gateways/infrastructure/commands.rs) · [commands.rs](../../../../crates/nessa-server/tests/peer_gateways/commands.rs) · [poller.rs](../../../../crates/nessa-server/src/peer_gateways/infrastructure/poller.rs)
