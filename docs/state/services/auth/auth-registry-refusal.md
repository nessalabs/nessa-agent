---
id: auth-registry-refusal
title: Registry refusal evidence
kind: operation
status: implemented
summary: "Nessa refuses to use a credential file if it is damaged, too large, has an unsupported format, or is unsafe to open."
parent: auth
sources:
  - crates/nessa-auth/src/application/credential_registry.rs
diagramLinks: {}
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
