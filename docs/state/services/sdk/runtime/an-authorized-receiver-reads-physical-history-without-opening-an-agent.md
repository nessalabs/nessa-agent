---
id: "sdk-runtime-an-authorized-receiver-reads-physical-history-without-opening-an-agent"
title: "an authorized receiver reads physical history without opening an agent"
kind: "operation"
status: "mixed"
summary: "An authorized receiver can read saved records without opening an agent or running a prompt."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/conversation/application/passive_read.rs"
  - "crates/nessa-server/src/conversation/application/record_read/read.rs"
  - "crates/nessa-server/src/conversation/infrastructure/record_read/source.rs"
  - "crates/nessa-server/src/conversation/infrastructure/record_read/operation.rs"
  - "crates/nessa-server/src/product/socket.rs"
  - "crates/nessa-server/src/product/record_read/dispatch.rs"
  - "crates/nessa-protocol/src/product/record_read.rs"
  - "crates/nessa-sdk/src/infrastructure/session_storage/record_source.rs"
  - "crates/nessa-protocol/src/product/generated.rs"
  - "protocol/product/v1.json"
  - "crates/nessa-server/src/product/state.rs"
  - "crates/nessa-server/tests/conversation/passive_read.rs"
  - "crates/nessa-server/tests/conversation/record_read/source.rs"
  - "crates/nessa-client-core/tests/read_only_sync/gateway/session.rs"
  - "crates/nessa-client-core/tests/read_only_sync/gateway/deadline_stream.rs"
  - "crates/nessa-client-core/tests/read_only_sync/infrastructure/cache.rs"
  - "crates/nessa-server/tests/composition/read_only_online.rs"
  - "crates/nessa-server/tests/device_pairing/infrastructure/protected.rs"
  - "docs/design/auth/device-pairing.md"
  - "docs/design/authorized-record-reads.md"
  - "docs/design/read-only-sync-example.md"
  - "crates/nessa-client-core/src/read_only_sync/mod.rs"
diagramLinks:
  Reading: "gateway-record-reads"
---

# an authorized receiver reads physical history without opening an agent

An authorized receiver can read saved records without opening an agent or running a prompt. It pages through a bounded history source and keeps its downloaded position.

Applying those records to its local view is a separate step with a separate checkpoint. The receiver pairs with the device and uses a protected native connection. It checks the pinned TLS identity, opens a product session and authenticates with its issued credential ID. Automatic device discovery and writable collaboration remain outside this read-only path. See [protected reads](../../../../design/auth/device-pairing.md#protected-reads-over-the-native-channel-slice-3). Source cleanup and response delivery must settle before the read capacity is reused.

```mermaid
stateDiagram-v2
    [*] --> Reserved: Socket and global capacity acquired
    Reserved --> Authorized: Fresh credential, policy, receiver and owner accepted
    Reserved --> Rejected: Admission refused
    Authorized --> Reading: Exact scope / tracked physical source
    Reading --> Preparing: Source not ready
    Reading --> Joined: Bounded head or page / join source worker
    Reading --> TimedOutOwned: Deadline / physical work still owns capacity
    TimedOutOwned --> Joined: Outstanding source completes and joins
    Joined --> Delivered: Response sent or dropped / release lease
    Delivered --> Downloaded: Receiver persists physical progress
    Downloaded --> Applied: Fold semantic progress
    Applied --> Reserved: Next page before captured target / fresh admission
    Preparing --> Reserved: Later deliberate read
    note right of Reserved
        A paired, protected product session is already established.
    end note
    note right of TimedOutOwned
        A timeout response alone does not release capacity.
        Downloaded and applied positions are separate.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/conversation/application/passive_read.rs) · [Related source](../../../../../crates/nessa-server/src/conversation/application/record_read/read.rs) · [Related tests](../../../../../crates/nessa-server/tests/conversation/passive_read.rs)
