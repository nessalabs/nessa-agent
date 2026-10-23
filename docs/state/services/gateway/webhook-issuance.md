---
id: gateway-webhook-issuance
title: Webhook credential issuance
kind: operation
status: mixed
summary: The existing auth registry applies Cedar webhook restrictions and binds issuance purpose to its durable receipt; endpoint registration is still pending.
sourceRevision: "8e9f8c93277f4654f15632c89503d37be6d8a2e8"
parent: gateway
sources:
  - crates/nessa-auth/src/application/credential_admin.rs
  - crates/nessa-auth/src/application/ports.rs
  - crates/nessa-auth/src/adapters/cedar/attributes.rs
  - crates/nessa-auth/src/adapters/cedar/policies.cedar
  - crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs
  - crates/nessa-auth/src/adapters/local/registry.rs
  - crates/nessa-auth/tests/adapters/local/webhook_issue.rs
  - crates/nessa-server/src/product/socket.rs
diagramLinks: {}
diagramDetails:
  - diagram: 0
    node: Refused
    section: Refusal and recovery
  - diagram: 0
    node: Original
    section: Refusal and recovery
---

# Webhook credential issuance

The auth boundary can issue a read-only integration credential through the
existing registry. This checkpoint implements that issuing path and receipt
identity. The product handler still selects ordinary issuance: there is no
webhook configuration field or composed endpoint registration yet. Application
installation (#1052) remains separate.

## Guards and effects

```mermaid
stateDiagram-v2
    [*] --> Receipt: Registry derives path from server-selected purpose
    Receipt --> Refused: Request identity conflicts with an earlier command
    Receipt --> Original: Matching committed request
    Receipt --> Validate: No earlier receipt
    Validate --> Refused: Invalid linkage, resource, capacity or Cedar refusal
    Validate --> Commit: Cedar permits integration member and conversation.read
    Commit --> Refused: Durable write unavailable
    Commit --> Issued: Credential, verifier, receipt and audit commit together
    Original --> [*]: Original metadata and transitions without secret
    Issued --> [*]: One-time bearer evidence
    Refused --> [*]: No successful issuance response
```

The server-selected `CredentialIssuePurpose::Webhook` maps to `WebhookIssue`.
The existing `GrantPolicyEvaluator` asks Cedar who may receive the credential
and which grants it may hold. The webhook forbids permit only an integration
member with `conversation.read`; the existing member/admin policies own the role
decision. The registry owns identity linkage, audience, resource validation,
receipt capacity, durable mutation and transition evidence. Destination or MAC
key possession supplies no authority.

## Refusal and recovery

The existing fingerprint includes the webhook purpose. Changing between ordinary
and webhook issuance under the same issuer/request refuses before a registry
write, including after restart. A matching retry returns the original selected
credential ID and immutable transitions, even if a new ID or issuance time was
proposed. It cannot recover the bearer secret; the existing explicit recovery is
revoke and reissue. Configuration intent and endpoint installation must correlate
that returned identity before installing a destination; that coordinator is
still pending.

The fresh-command refusal test compares registry bytes before and after each
denied request. Storage failure remains a typed failure, not proof of a remote
effect. This operation makes no HTTP request and does not claim delivery or
exactly-once remote effects. Attempt admission and durable delivery recovery are
owned by the [admission](webhook-admission.md) and [outbox](webhook-outbox.md)
pages.

## Evidence and limits

At this exact source, the full auth library suite passed **212 tests**. The
[registry tests](../../../../crates/nessa-auth/tests/adapters/local/webhook_issue.rs)
exercise policy refusal before writes, cross-purpose receipt substitution in
both directions, cold replay, original transitions and unavailable secret.
The [Cedar matrix](../../../../crates/nessa-auth/src/adapters/cedar/evaluator_tests.rs)
checks principal kinds, actions and issuing paths. Six fail/restore probes caught
removal of the relevant kind/action restrictions, path mapping/use and receipt
purpose binding; every restored control passed with byte-identical source.

Required seven-package all-target Clippy with `-D warnings`, server all-target
check and the architecture checker passed. The first architecture invocation
could not find Cargo on PATH; the corrected invocation passed. The pre-existing
lease const-evaluation notice remains in check/Clippy output.

Registration, W1 coordination across ordinary issuance, revocation/expiry stop
composition, source capture, supervised sending and actual recipient E2E remain
unfinished. Canonical scripted Chromium/WebKit and feature stress/chaos remain
applicable and unqualified. A documentation render does not qualify those gates.
