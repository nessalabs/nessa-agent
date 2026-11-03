---
id: client-event-registration
title: Event handler registration generations
kind: statechart
status: implemented
summary: "One event-handler pair with two registration generations; a stale
  unsubscribe cannot remove its replacement, and reconnect installs only the
  retained generation."
parent: client
sources:
  - packages/nessa-client/src/application/managed-session.ts
  - packages/nessa-client/src/application/managed-session.test.ts
model:
  spec: tla/services/client/EventRegistration.tla
  config: tla/services/client/EventRegistration.cfg
diagramLinks: {}
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:43.549Z
  config: tla/services/client/EventRegistration.cfg
  modelDigest: e2533618b23eea7e4836b2339138c2da9353cce151e635e9d642d92edf37da27
  states: 13
  held:
    - TypeOK
    - StaleNeverRemovesReplacement
    - ReinstallCurrent
    - ReplacementSurvivesOldHandle
  broke: []
  unchecked: []
---
One event-handler pair with two registration generations; a stale unsubscribe cannot remove its replacement, and reconnect installs only the retained generation.

Transport callback reentrancy and independent event/handler identities require TypeScript tests; shared handles refer to one registration rather than independent reference counts.

```mermaid
stateDiagram-v2
    [*] --> Unregistered
    Unregistered --> Registered: Register event and handler / mint generation
    Registered --> Registered: Duplicate registration / share generation
    Registered --> Unregistered: Unsubscribe exact current generation
    Unregistered --> Replacement: Register same function / mint later generation
    Replacement --> Replacement: Old unsubscribe / ignore stale generation
    Registered --> Registered: Reconnect / install retained registration
    Replacement --> Replacement: Reconnect / install replacement generation
```

## Model coverage

Direct finite safety model. One event-handler pair with two registration generations; a stale unsubscribe cannot remove its replacement, and reconnect installs only the retained generation.

Transport callback reentrancy and independent event/handler identities require TypeScript tests; shared handles refer to one registration rather than independent reference counts. Terminal states may stutter. This scoped review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7` does not claim implementation proof or an executed source test.
