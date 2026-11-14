---
id: gateway-backup-export
title: Export a consistent backup cut
kind: operation
status: implemented
summary: A backup pins the owning stores before it authenticates and publishes
  their cut.
parent: gateway
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - crates/nessa-backup/src/application/export.rs
  - crates/nessa-server/tests/backup/export.rs
  - docs/design/export-cut.md
model:
  spec: tla/services/gateway/BackupExport.tla
  config: tla/services/gateway/BackupExport.cfg
  steps:
    Init: Begin this bounded operation.
    Hold: Acquire mutation admission after checking the writer inventory.
    Pin: Pin each store and require complete outbox rows while admission is held.
    RefusePin: Refuse an incomplete outbox or retiring gateway and release admission.
    Read: Release admission before reading the remaining ordinary pinned records.
    Seal: Authenticate the manifest constructed from the pinned cut.
    RefuseMac: Stop publication when manifest authentication fails.
    Publish: Publish the authenticated cut and acknowledge the result.
    PublishUnconfirmed: Preserve uncertainty when publication does not acknowledge.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:13.690Z
  config: tla/services/gateway/BackupExport.cfg
  modelDigest: 66ebc6ffbd95d11483603bd6bbcfcd6ab3bd71f8885940f36854386c1bf4e1ef
  states: 10
  held:
    - TypeOK
    - AckRequiresCompleteCut
    - ReadingReleasesAdmission
  broke: []
  unchecked: []
---
# Export a consistent backup cut

A backup joins admitted mutations long enough to pin the participating stores. It requires complete device and environment outboxes, captures deletion fences, then releases admission before reading the remaining pinned records, authenticating the manifest and publishing the bundle.

An unavailable ordinary store can be listed as skipped. An incomplete required outbox stops the export. Publication failure does not establish that no bytes reached the destination. Secret redaction and deferred active conversations remain part of the export format.

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Holding: acquire admission
    Holding --> Reading: pin complete cut and release admission
    Holding --> Refused: outbox or retirement refusal
    Reading --> Authenticated: authenticate manifest
    Reading --> Refused: MAC unavailable
    Authenticated --> Published: publication acknowledged
    Authenticated --> Unconfirmed: publication not confirmed
```

## Bounded rule model

One export and two abstract store classes are modeled: ordinary records and required outboxes. The rules separate a pinned consistent cut, released mutation admission, authenticated bytes and publication acknowledgement.

The configured safety rules are `AckRequiresCompleteCut`, `ReadingReleasesAdmission`. TLC explores the authored abstraction; it does not execute or prove the implementation.

| Model action | Source behavior abstracted |
| --- | --- |
| `Hold` | Acquire mutation admission after checking the writer inventory. |
| `Pin` | Pin each store and require complete outbox rows while admission is held. |
| `RefusePin` | Refuse an incomplete outbox or retiring gateway and release admission. |
| `Read` | Release admission before reading the remaining ordinary pinned records. |
| `Seal` | Authenticate the manifest constructed from the pinned cut. |
| `RefuseMac` | Stop publication when manifest authentication fails. |
| `Publish` | Publish the authenticated cut and acknowledge the result. |
| `PublishUnconfirmed` | Preserve uncertainty when publication does not acknowledge. |

## Verification limits

The model omits per-store serialization, secret detection, MAC cryptography, file sizes, deletion-fence reconciliation, concurrent exporters and recovery of partially written bundles. It makes no eventual-publication or power-loss guarantee. The cited code and regression sources were inspected at the page checkpoint; their product test suites were not run in this modeling pass.

## Further reading

[export.rs](../../../../crates/nessa-backup/src/application/export.rs) · [export.rs](../../../../crates/nessa-server/tests/backup/export.rs) · [export-cut.md](../../../../docs/design/export-cut.md)
