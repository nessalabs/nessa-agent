---
id: endpoint-discovery
title: Publication and discovery
kind: operation
status: implemented
summary: "A caller needs the address of the gateway that is actually running."
parent: endpoint
sources:
  - crates/nessa-gateway-endpoint/src/application/publish.rs
  - crates/nessa-gateway-endpoint/src/infrastructure/file.rs
diagramLinks: {}
model:
  spec: tla/services/endpoint/Discovery.tla
  config: tla/services/endpoint/Discovery.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:04.179Z
  config: tla/services/endpoint/Discovery.cfg
  modelDigest: 87f141fdb06388f2adf5590ab2860b547202a4661dbdb1f79c0a921ec628081f
  states: 10
  held:
    - TypeOK
    - VerifiedIdentity
  broke: []
  unchecked: []
---
# Publication and discovery

A caller needs the address of the gateway that is actually running. It reads Nessa's private address record, then checks the live gateway against that record.

The address, instance and process identity must agree. When managed-service details are present, they must form a complete matching set. This confirms which gateway was found; authentication is still required. Some callers can fall back when no discovery record exists, so absence is not itself a successful validation.

```mermaid
stateDiagram-v2
    [*] --> Reading: discover / read private advertisement
    Reading --> Absent: no record / return None
    Reading --> Probing: valid complete advertisement / request health
    Reading --> Refused: unsafe or malformed record
    Probing --> Verified: every advertised identity field matches / return endpoint
    Probing --> Refused: mismatch, unavailable listener or deadline
```

## Further reading

[Source](../../../../crates/nessa-gateway-endpoint/src/application/publish.rs) · [Related source](../../../../crates/nessa-gateway-endpoint/src/infrastructure/file.rs)

## Model coverage

Direct model: One private advertisement and health probe; complete managed identity and address/instance/process equality are required before discovery succeeds.

The finite model checks safety only. Parsing, deadlines, actual process identity and TCP ownership are external predicates. No authentication or availability guarantee; publication uses storage-publication. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.
