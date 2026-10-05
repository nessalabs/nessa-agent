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
