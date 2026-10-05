---
id: storage-database
title: Database opening and version refusal
kind: operation
status: implemented
summary: "Nessa opens its private database only when the file and format are acceptable."
parent: storage
sources:
  - crates/nessa-local-database/src/lib.rs
  - crates/nessa-local-database/tests/open.rs
diagramLinks: {}
---

# Database opening and version refusal

Nessa opens its private database only when the file and format are acceptable. It checks the schema version before changes that could affect the database.

An unsupported version is preserved rather than silently migrated or repaired. A private file path alone does not prove every platform permission guarantee. This page follows database opening, not the lifetime of every row stored in it.

```mermaid
stateDiagram-v2
    [*] --> Inspecting: open / validate private directory and file
    Inspecting --> Refused: unsafe, unreadable, unversioned tables or other version
    Inspecting --> Locked: acceptable version / select rollback journal and begin immediate transaction
    Locked --> Checking: current version
    Locked --> Initializing: empty file / execute schema
    Checking --> Open: quick_check passes / return configured connection
    Checking --> Refused: integrity check fails
    Initializing --> Open: applied version agrees / commit
    Initializing --> Refused: schema or version failure / rollback transaction
```

## Further reading

[Source](../../../../crates/nessa-local-database/src/lib.rs)
