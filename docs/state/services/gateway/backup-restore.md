---
id: gateway-backup-restore
title: Verify, quarantine and accept a backup
kind: operation
status: implemented
summary: Backup verification, staged readiness and live activation are separate
  decisions.
parent: gateway
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - crates/nessa-server/src/backup/application/restore.rs
  - crates/nessa-server/tests/backup/restore.rs
  - docs/design/quarantined-restore.md
  - crates/nessa-server/src/backup/application/restore_planning.rs
model:
  spec: tla/services/gateway/BackupRestore.tla
  config: tla/services/gateway/BackupRestore.cfg
  steps:
    Init: Begin this bounded operation.
    Seal: Verify and seal the quarantined files and journal.
    Marker: Publish the staging readiness marker for this generation.
    Ready: Acknowledge ready completion with the matching stage and journal identity.
    PresentReceipt: Present any retained receipt, including a delayed older generation.
    ReplaceStage: A newer stage supersedes prior readiness even when bytes are unchanged.
    ChangeJournal: Observe a changed journal that cannot reuse its previous sealed identity.
    RetireStage: Spend a pending or ready stage so a later receipt cannot revive it.
    Accept: Promote only the current unspent verified stage onto an empty live
      destination.
    AcceptUnsynced: Retain live ownership when rename succeeds but directory sync is
      unconfirmed.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:14.399Z
  config: tla/services/gateway/BackupRestore.cfg
  modelDigest: e25ce0794c82537eba2def63e1044980fe00f1c0d329a408fa28cabb16114944
  states: 244
  held:
    - TypeOK
    - AcceptanceHasCurrentProof
    - DurabilityRequiresPublication
  broke: []
  unchecked: []
---
# Verify, quarantine and accept a backup

The standalone verifier checks supplied bundle bytes with an explicitly trusted key. Offline planning reports missing prerequisites; it cannot activate a gateway. Restore is a different operation: policy admission and its audit precede verification, quarantine staging, sealing and a correlated ready completion.

Acceptance checks the current journal, stage identity, receipts and empty live destination before promotion. A marker or historical seal alone is insufficient. A successful rename can make the destination live even when its later directory sync fails. Deferred conversation fetch has its own exact stream fingerprint and publication identity.

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> Sealed: verified quarantine
    Sealed --> Ready: marker and matching ready completion
    Ready --> Live: current unspent stage accepted
    Ready --> Unconfirmed: rename succeeded but sync failed
    Ready --> Pending: newer stage supersedes readiness
```

## Bounded rule model

Two stage generations and two journal digests explore superseded receipts, changed bytes and spent readiness. Completed receipts remain independently selectable after a later stage replaces them; acceptance checks the selected receipt rather than constructing matching proof from the current state. Acceptance records the identity actually used. Confirmed durability remains separate from a tree already made live.

The configured safety rules are `AcceptanceHasCurrentProof`, `DurabilityRequiresPublication`. TLC explores the authored abstraction; it does not execute or prove the implementation.

| Model action | Source behavior abstracted |
| --- | --- |
| `Seal` | Verify and seal the quarantined files and journal. |
| `Marker` | Publish the staging readiness marker for this generation. |
| `Ready` | Acknowledge ready completion with the matching stage and journal identity. |
| `ReplaceStage` | A newer stage supersedes prior readiness even when bytes are unchanged. |
| `ChangeJournal` | Observe a changed journal that cannot reuse its previous sealed identity. |
| `RetireStage` | Spend a pending or ready stage so a later receipt cannot revive it. |
| `Accept` | Promote only the current unspent verified stage onto an empty live destination. |
| `AcceptUnsynced` | Retain live ownership when rename succeeds but directory sync is unconfirmed. |

## Verification limits

The abstraction assumes successful policy/audit admission before staging. It omits trusted-key recovery, host identity fencing, omission acceptance, file-handle containment, deferred fetch streaming and OS-specific quarantine protection. The offline plan remains informational even though a separate quarantine use case now exists. The cited code and regression sources were inspected at the page checkpoint; their product test suites were not run in this modeling pass.

## Further reading

[restore.rs](../../../../crates/nessa-server/src/backup/application/restore.rs) · [restore.rs](../../../../crates/nessa-server/tests/backup/restore.rs) · [quarantined-restore.md](../../../../docs/design/quarantined-restore.md)
