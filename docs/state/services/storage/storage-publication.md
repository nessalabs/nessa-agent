---
id: storage-publication
title: Private publication and acknowledgement
kind: operation
status: implemented
summary: "Publishing here means putting a prepared private file at its intended location."
parent: storage
sources:
  - crates/nessa-local-storage/src/retained_directory.rs
diagramLinks: {}
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
