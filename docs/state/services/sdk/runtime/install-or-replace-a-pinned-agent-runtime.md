---
id: "sdk-runtime-install-or-replace-a-pinned-agent-runtime"
title: "install or replace a pinned agent runtime"
kind: "operation"
status: "mixed"
summary: "Installation downloads and publishes the agent version Nessa has tested."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/agent_install/application/install.rs"
  - "crates/nessa-server/src/agent_install/domain/release_selection.rs"
  - "crates/nessa-server/src/agent_install/domain/value_objects/pinned_release.\
    rs"
  - "crates/nessa-server/src/agent_install/domain/entities/install_attempt.rs"
  - "crates/nessa-server/src/agent_install/infrastructure/delivery/journal.rs"
  - "crates/nessa-server/src/agent_install/infrastructure/managed_runtimes.rs"
  - "crates/nessa-server/src/agent_install/application/reclamation.rs"
  - "docs/adr/done/173-fetch-agent-runtimes.md"
  - "docs/codebase-structure.md"
  - "crates/nessa-server/tests/agent_install/install.rs"
  - "crates/nessa-server/tests/agent_install/publication_delivery.rs"
  - "crates/nessa-server/tests/agent_install/delivery.rs"
  - "crates/nessa-server/tests/agent_install/managed_runtimes.rs"
  - "crates/nessa-server/tests/agent_install/reclamation.rs"
  - "crates/nessa-server/tests/agent_install/runtime_reclamation.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/sessions/executable_use_cleanup.\
    rs"
diagramLinks: {}
model:
  spec: tla/services/sdk/RuntimeInstall.tla
  config: tla/services/sdk/RuntimeInstall.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:18.545Z
  config: tla/services/sdk/RuntimeInstall.cfg
  modelDigest: 15ad1e2be6d473430ede7d3cc40f21f3bc8f3d079b728278dafff8fca2e9393e
  states: 12
  held:
    - TypeOK
    - PublicationNeedsPin
    - NoReclaimLiveUse
    - ReclaimedNeedsRelease
  broke: []
  unchecked: []
---
# install or replace a pinned agent runtime

Installation downloads and publishes the agent version Nessa has tested. Launch resolves the current installed version; an old printed path is not a promise that it remains safe to use.

Before spawning, Nessa records that the process owns a use of that version. Replacement cannot remove it until those uses are confirmed released. A dropped guard or process-exit message alone is insufficient. A file found after a crash does not invent missing proof that publication completed.

```mermaid
stateDiagram-v2
    [*] --> Reconciling: Lock account / recover prior delivery
    Reconciling --> Reported: Exact complete pin already installed
    Reconciling --> Downloading: New artifact required
    Downloading --> Verifying: Bound staged bytes / measure digest
    Verifying --> Rejected: Pin rejects digest / do not unpack
    Verifying --> Prepared: Verified audit / durable publication preparation
    Prepared --> PublicationObserved: Publish under lock / retain exact terminal
    PublicationObserved --> AwaitingSettlement: Audit terminal and retained replacement obligation
    AwaitingSettlement --> AwaitingSettlement: Live or uncertain predecessor use / retain cleanup
    AwaitingSettlement --> Reported: Required acknowledgements and exact receipt settled
    note right of PublicationObserved
        Publication, delivery acknowledgement and
        predecessor reclamation are distinct facts.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/agent_install/application/install.rs) · [Related source](../../../../../crates/nessa-server/src/agent_install/domain/release_selection.rs) · [Related tests](../../../../../crates/nessa-server/tests/agent_install/install.rs)

## Model coverage

Direct model: One pinned runtime install with verified bytes, durable preparation, publication and predecessor use-release before physical reclamation.

The finite model checks safety only. Download signatures, platform unpacking, executable path leases, delivery receipt recovery and multi-version SQL are source contracts; process exit alone is not release. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.

The model's Reclaimed terminal means physical predecessor reclamation, not the API reporting installation success. An installed runtime can be returned while a cleanup obligation is durably deferred, with reclamation warnings; the Published model state permits that outcome. A live or uncertain predecessor use still prevents physical reclamation.
