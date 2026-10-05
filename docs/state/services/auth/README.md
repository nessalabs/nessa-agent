---
id: auth
title: Identity and access
kind: service
status: implemented
summary: "Nessa checks who can access a gateway and its conversations."
parent: nessa
sources:
  - crates/nessa-auth/README.md
diagramLinks:
  Owner0: "auth-admission"
  Owner1: "auth-credential"
  Owner2: "auth-registry-refusal"
---

# Identity and access

Nessa checks who can access a gateway and its conversations. Credentials, current membership and permissions determine whether an action is allowed.

These pages explain credential lifetime, sign-in checks and safe refusal of damaged credential storage.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Owner0["Authentication admission and credential receipts"]
    Owner1["Credential lifecycle and access"]
    Owner2["Registry refusal evidence"]
```

## Browse this area

- [Authentication admission and credential receipts](auth-admission.md)
- [Credential lifecycle and access](auth-credential.md)
- [Registry refusal evidence](auth-registry-refusal.md)
