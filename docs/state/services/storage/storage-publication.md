---
id: storage-publication
title: Private publication and acknowledgement
kind: operation
status: implemented
summary: "Publishing here means putting a prepared private file at its intended
  location."
parent: storage
sources:
  - crates/nessa-local-storage/src/retained_directory.rs
diagramLinks: {}
model:
  spec: tla/services/storage/Publication.tla
  config: tla/services/storage/Publication.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:22.275Z
  config: tla/services/storage/Publication.cfg
  modelDigest: 96b85adc702f02cdb90dd74bf53127b8ed060538a77406b35bd9e874689960bc
  states: 9
  held:
    - TypeOK
    - RenameDisarmsReservation
    - NoNewerRemoval
    - AcknowledgementDoesNotErase
  broke: []
  unchecked: []
---
# Private publication and acknowledgement

Publishing here means putting a prepared private file at its intended location. Nessa first prepares and syncs the file, replaces the destination, then confirms the containing directory was synced.

The new file can already be visible when a later sync fails. Error therefore does not mean nothing changed. Cleanup has its own result and must not remove a newer publication. Platform limits remain part of the storage contract.

```mermaid
stateDiagram-v2
    [*] --> Reserved: reserve_temp / retain origin authority
    Reserved --> Published: publish_new or replace [binding and identity match] / flush and rename
    Reserved --> Refused: validation or rename fails / attempt origin-only cleanup
    Reserved --> Discarded: discard / remove matching reservation
    Published --> Acknowledged: flush and binding checks pass / sync directory
    Published --> Unacknowledged: acknowledgement fails / retain published identity and handle
```

## Further reading

[Source](../../../../crates/nessa-local-storage/src/retained_directory.rs)

## Model coverage

Direct model: One origin-bound reservation, rename before directory acknowledgement, retained published identity on failure and newer destination replacement.

The finite model checks safety only. OS inode/handle binding checks and permissions are trusted boundaries; multi-process adversaries, platform durability and every caller cleanup policy are not proven. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.
