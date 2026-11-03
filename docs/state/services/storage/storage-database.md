---
id: storage-database
title: Database opening and version refusal
kind: operation
status: implemented
summary: "Nessa opens its private database only when the file and format are
  acceptable."
parent: storage
sources:
  - crates/nessa-local-database/src/lib.rs
  - crates/nessa-local-database/tests/open.rs
diagramLinks: {}
model:
  spec: tla/services/storage/DatabaseOpen.tla
  config: tla/services/storage/DatabaseOpen.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:21.648Z
  config: tla/services/storage/DatabaseOpen.cfg
  modelDigest: 525053da7619f316a4022ca4edfbea6af6c4ae1e7079e8dc35a1f274abe353d6
  states: 9
  held:
    - TypeOK
    - NoPartialVersion
    - OpenRequiresVersion
    - RefusalPreserves
    - StagedRequiresValidation
  broke: []
  unchecked: []
---
# Database opening and version refusal

Nessa opens its private database only when the file and format are acceptable. It checks the schema version before changes that could affect the database.

An unsupported version is preserved rather than silently repaired. An older version with an explicit validated forward upgrade chain is upgraded in one transaction; failure rolls back the whole chain. A private file path alone does not prove every platform permission guarantee. This page follows database opening, not the lifetime of every row stored in it.

```mermaid
stateDiagram-v2
    [*] --> Inspecting: open / validate private directory and file
    Inspecting --> Refused: unsafe, unreadable, unversioned tables or unsupported version
    Inspecting --> Locked: acceptable version / select rollback journal and begin immediate transaction
    Locked --> Checking: current version
    Locked --> Initializing: empty file / execute schema
    Locked --> Upgrading: supported older version / validate integrity and apply explicit chain
    Upgrading --> Open: resulting version matches / commit all steps
    Upgrading --> Refused: any step fails / rollback whole transaction
    Checking --> Open: quick_check passes / return configured connection
    Checking --> Refused: integrity check fails
    Initializing --> Open: applied version agrees / commit
    Initializing --> Refused: schema or version failure / rollback transaction
```

## Further reading

[Source](../../../../crates/nessa-local-database/src/lib.rs)

## Model coverage

Direct model: One version-2 database with an explicitly supported upgrade to version 3; staged schema changes commit together or roll back.

The finite model checks safety only. Fresh schema creation, current-version open, multiple migration steps, SQL contents, private path and encrypted-key boundaries remain source contracts; arbitrary older versions are still refused. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.

Optional encrypted opening supplies a validated database key before schema access. Encryption/key management and platform private-file permissions are boundary contracts, not migration states.
