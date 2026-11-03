---
id: "host-startup-bootstrap-and-diagnose-access-through-the-cli"
title: "bootstrap and diagnose access through the CLI"
kind: "operation"
status: "implemented"
summary: "The CLI can diagnose an existing gateway, issue access tokens and
  perform explicit local setup or recovery."
parent: "host-startup"
sources:
  - "docs/guides/local-auth.md"
  - "crates/nessa-server/src/cli/entrypoint/arguments.rs"
  - "crates/nessa-server/src/cli/application/commands.rs"
  - "crates/nessa-server/src/composition/cli.rs"
  - "crates/nessa-server/src/composition/auth_command.rs"
  - "crates/nessa-server/src/composition/local_auth.rs"
  - "crates/nessa-server/src/composition/credential_registry.rs"
  - "crates/nessa-server/tests/cli/application.rs"
  - "crates/nessa-server/tests/cli/gateway.rs"
  - "crates/nessa-server/tests/composition/local_auth.rs"
  - "crates/nessa-server/tests/credential_registry_refusal.rs"
  - "scripts/smoke-auth.mjs"
diagramLinks: {}
model:
  spec: tla/services/host/startup/CliTokenLifetime.tla
  config: tla/services/host/startup/CliTokenLifetime.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:28.782Z
  config: tla/services/host/startup/CliTokenLifetime.cfg
  modelDigest: ed4afac84b1efda2b748b31d18fcd0be3d3c3386fdffac27aeed93997a8b4944
  states: 22
  held:
    - LifetimeCapped
    - NoExpiredIssue
  broke: []
  unchecked: []
---
# bootstrap and diagnose access through the CLI

The CLI can diagnose an existing gateway, issue access tokens and perform explicit local setup or recovery. Starting the server does not silently create or replace its identity.

Online commands use authenticated gateway access. Offline recovery needs exclusive access to the credential file and preserves the gateway's identity. An unsafe file is left unchanged. A newly issued token is revealed once; issuing it is not automatically retried. Revocation survives restart and blocks later access checks.

```mermaid
stateDiagram-v2
    [*] --> Unprovisioned
    Unprovisioned --> Provisioning: auth init --local or server --provision-local
    Provisioning --> Provisioned: Exclusive registry write confirmed
    Provisioning --> Refused: Unsafe or unavailable registry / preserve bytes
    Provisioned --> Serving: server / open existing namespace
    Provisioned --> RecoveringOwner: recover-owner --local [gateway stopped]
    RecoveringOwner --> Provisioned: Retain gateway identity / revoke prior owner
    RecoveringOwner --> Refused: Exclusive recovery refused
    Serving --> Serving: doctor / authenticated diagnosis
    Serving --> Serving: auth token / once-only secret delivery
    Serving --> Provisioned: Stop gateway
    note right of Serving
        Namespace provisioning lifecycle projection.
        Doctor and token issuance are separate commands.
        Plain server does not bootstrap; cloud is refused.
    end note
```

## Further reading

[Source](../../../../guides/local-auth.md) · [Related source](../../../../../crates/nessa-server/src/cli/entrypoint/arguments.rs) · [Related tests](../../../../../crates/nessa-server/tests/cli/application.rs)

## Bounded rule model

Owns browser-token lifetime selection capped by the authenticated caller expiry and refusal when already expired. Current time is one and expiries range one through three. Offline credential recovery, exclusive locks, gateway identity and secret disclosure are excluded. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `crates/nessa-server/src/cli/application/commands.rs`.
