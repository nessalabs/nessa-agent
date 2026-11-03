---
id: gateway-tunnel-connectivity
title: Reach a gateway through a tunnel
kind: operation
status: implemented
summary: A ready tunnel forwards native TLS but grants no session authority.
parent: gateway
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
sources:
  - crates/nessa-server/src/tunnel/application.rs
  - crates/nessa-server/src/tunnel/infrastructure/ingress.rs
  - crates/nessa-server/tests/tunnel/ingress.rs
  - crates/nessa-server/tests/composition/tunnel.rs
  - docs/design/auth/tunnel.md
model:
  spec: tla/services/gateway/TunnelConnectivity.tla
  config: tla/services/gateway/TunnelConnectivity.cfg
  steps:
    Init: Begin this bounded operation.
    ProviderReady: Publish the provider address only after provider readiness.
    ProviderFails: Retain a typed provider failure without fabricating readiness.
    Retry: Retry the supervised provider only while its lifetime is not draining.
    NativeAdmission: Admit a connection through the separate pinned native TLS and
      credential owner.
    Dispatch: Dispatch product work only under native session admission.
    Drain: Fence new provider attempts and begin the existing tunnel lifetime drain.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:24.001Z
  config: tla/services/gateway/TunnelConnectivity.cfg
  modelDigest: 1cbb52c1e363e1808425ca9f90c0df09ede5c0ac72d9f74a921304e695831c2e
  states: 12
  held:
    - TypeOK
    - ForwardingIsNotAuthority
    - NoRetryAfterDrain
    - DrainStopsPublication
  broke: []
  unchecked: []
---
# Reach a gateway through a tunnel

A configured provider forwards WebSocket bytes to a loopback ingress. Those bytes still pass through the native TLS listener. Quick mode receives a provider address at startup; named mode uses its configured hostname and separately stored provider token.

The tunnel publishes Starting, Ready or a typed failure. Provider readiness is reachability, not pairing or product-session authentication. Supervision can retry failures until the owning lifetime drains. Already admitted work and new provider attempts have separate lifetimes.

```mermaid
stateDiagram-v2
    [*] --> Starting
    Starting --> Ready: provider ready
    Starting --> Failed: typed launch failure
    Ready --> Failed: provider ends
    Failed --> Starting: supervised retry before drain
    Ready --> Stopped: drain fences attempts
    Failed --> Stopped: drain fences attempts
```

## Bounded rule model

Provider readiness and native admission are separate facts. One supervised failure/retry lifetime checks that dispatch requires native authority and that draining cannot restart publication.

The configured safety rules are `ForwardingIsNotAuthority`, `NoRetryAfterDrain`, `DrainStopsPublication`. TLC explores the authored abstraction; it does not execute or prove the implementation.

| Model action | Source behavior abstracted |
| --- | --- |
| `ProviderReady` | Publish the provider address only after provider readiness. |
| `ProviderFails` | Retain a typed provider failure without fabricating readiness. |
| `Retry` | Retry the supervised provider only while its lifetime is not draining. |
| `NativeAdmission` | Admit a connection through the separate pinned native TLS and credential owner. |
| `Dispatch` | Dispatch product work only under native session admission. |
| `Drain` | Fence new provider attempts and begin the existing tunnel lifetime drain. |

## Verification limits

Native TLS cryptography, paired credential recovery, pending-secret filesystem receipts, WebSocket parsing, provider download verification and token rotation are not represented. A tunnel result does not grant a role or certify external network availability. The cited code and regression sources were inspected at the page checkpoint; their product test suites were not run in this modeling pass.

## Further reading

[application.rs](../../../../crates/nessa-server/src/tunnel/application.rs) · [ingress.rs](../../../../crates/nessa-server/src/tunnel/infrastructure/ingress.rs) · [ingress.rs](../../../../crates/nessa-server/tests/tunnel/ingress.rs)
