---
id: github-relay-live-selection
title: Select private live event repositories
kind: statechart
status: implemented
parent: github-webhooks
summary: Repository selection and removal evidence commit together in the
  current vault generation.
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - crates/nessa-server/src/github_relay/infrastructure/selection.rs
diagramLinks: {}
model:
  spec: tla/services/github-webhooks/RelaySelection.tla
  config: tla/services/github-webhooks/RelaySelection.cfg
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:05.511Z
  config: tla/services/github-webhooks/RelaySelection.cfg
  modelDigest: c09dfc380bd5a9d27bbd672bccd144225b924288e1e8a01135d19c8275d5c5f6
  states: 6
  held:
    - DeletionEvidence
    - FailedLeavesPrior
  broke: []
  unchecked: []
---
# Select private live event repositories

One transaction replaces the selected repository bindings. Removing a binding records its prior generation and removes its local events with their deletion evidence. Existing bindings retain their identity while a repository name may change. Explicit gap acceptance moves the cursor to at least the retained gap and clears that gap.

The commit runs through the current vault generation owner. A stale generation or failed transaction leaves the prior selection intact. This does not prove GitHub access, fetch events, or start an agent.

```mermaid
stateDiagram-v2
    [*] --> Preparing
    Preparing --> Committed: current generation and transaction commits
    Preparing --> Refused: stale generation or storage failure
```

## Bounded rule model

Owns atomic binding removal, event deletion evidence and current-generation commit fencing. One selected repository bounds the transaction. Addition, name update and gap acceptance are described contracts outside these removal invariants; cloud authorization and catch-up belong to the relay page owner. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `crates/nessa-server/src/github_relay/infrastructure/selection.rs`.
