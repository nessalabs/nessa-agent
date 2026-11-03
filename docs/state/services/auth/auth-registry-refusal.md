---
id: auth-registry-refusal
title: Registry refusal evidence
kind: operation
status: implemented
summary: "Nessa refuses to use a credential file if it is damaged, too large,
  has an unsupported format, or is unsafe to open."
parent: auth
sources:
  - crates/nessa-auth/src/application/credential_registry.rs
diagramLinks: {}
model:
  spec: tla/services/auth/RegistryRefusal.tla
  config: tla/services/auth/RegistryRefusal.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:26.269Z
  config: tla/services/auth/RegistryRefusal.cfg
  modelDigest: aaf1a8f2207b1252635862a09eb7e2f15961e364d494a56390e381709bad5514
  states: 7
  held:
    - TypeOK
    - RefusalPreserves
    - UnsafeLockBeforeRead
    - RecordedNeedsAudit
  broke: []
  unchecked: []
---
# Registry refusal evidence

Nessa refuses to use a credential file if it is damaged, too large, has an unsupported format, or is unsafe to open. It leaves the file unchanged so the problem can be investigated without destroying the original.

The refusal record describes the problem without exposing secrets. Failure to save that record does not make the file safe or replace the original error. This chart follows a refusal, not every attempt to open a credential file.

```mermaid
stateDiagram-v2
    [*] --> Observed: attempt registry open
    Observed --> RegistryPreserved: registry fault / retain file and refusal evidence
    Observed --> LockPreserved: unsafe lock / refuse before registry read
    RegistryPreserved --> Recorded: refusal audit succeeds / report original fault
    LockPreserved --> Recorded: refusal audit succeeds / report original fault
    RegistryPreserved --> AuditUnavailable: audit fails / retain original fault
    LockPreserved --> AuditUnavailable: audit fails / retain original fault
```

## Further reading

[Source](../../../../crates/nessa-auth/src/application/credential_registry.rs)

## Model coverage

Direct model. One rejected lock or registry observation; preserve original authority-bearing file and fault while reporting independent refusal-audit outcome.

OS safety checks and secret-free diagnostic construction are pure boundary contracts; no repair, credential revocation or eventual audit delivery is modeled. Safety only; terminal stuttering is allowed and there is no liveness or implementation-proof claim. This scoped source review adds the abstraction at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints remain unchanged. Source tests were inspected, not executed.
