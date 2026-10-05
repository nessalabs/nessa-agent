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
  - "crates/nessa-server/src/agent_install/domain/value_objects/pinned_release.rs"
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
  - "crates/nessa-sdk/tests/infrastructure/acp/sessions/executable_use_cleanup.rs"
diagramLinks: {}
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
