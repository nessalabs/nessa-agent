---
id: "gateway-record-reads"
title: "Bounded physical history reads"
kind: "operation"
status: "implemented"
summary: "An authorized reader can page through saved conversation records without starting an agent."
parent: "gateway"
sources:
  - "crates/nessa-server/src/product/socket.rs"
  - "crates/nessa-server/src/product/record_read/dispatch.rs"
  - "crates/nessa-server/src/conversation/application/record_read/read.rs"
  - "crates/nessa-server/src/conversation/infrastructure/record_read/operation.rs"
  - "crates/nessa-client-core/tests/read_only_sync/gateway/deadline_stream.rs"
diagramLinks: {}
---

# Bounded physical history reads

An authorized reader can page through saved conversation records without starting an agent. Nessa limits both the number of records and the bytes in each response.

The read holds its source and capacity until completion, timeout and cleanup have settled. Closing the socket does not instantly make unfinished read resources available. Downloading records and applying them to a receiver's local history are separate steps.

```mermaid
stateDiagram-v2
    [*] --> Reserved: Obtain socket and global capacity
    Reserved --> Admitted: Fresh access, receiver and conversation checks
    Reserved --> Refused: Admission rejected
    Admitted --> Reading: Hand capacity lease to tracked source thread
    Reading --> Encoding: Source joined / return result and lease
    Reading --> TimedOutRetained: Caller deadline / source still owns capacity
    TimedOutRetained --> Released: Source exits and joins
    Encoding --> Queued: Exact scope and bounded JSON verified
    Encoding --> Released: Invalid or oversized result
    Queued --> Delivering: Writer selects response before its retained deadline
    Queued --> Released: Deadline or socket teardown
    Delivering --> Released: Physical send completed or socket torn down
    note right of TimedOutRetained
        Returning a timeout does not release the source permit.
        The tracked source owns it until joined.
    end note
```

## Further reading

[Source](../../../../crates/nessa-server/src/product/socket.rs) · [Related source](../../../../crates/nessa-server/src/product/record_read/dispatch.rs) · [Related tests](../../../../crates/nessa-client-core/tests/read_only_sync/gateway/deadline_stream.rs)
