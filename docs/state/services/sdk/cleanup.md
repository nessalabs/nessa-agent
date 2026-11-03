---
id: "sdk-cleanup"
title: "Work admission and provider cleanup"
kind: "statechart"
status: "implemented"
summary: "When work is stopped, Nessa blocks new work for that owner and
  finishes the accepted work's cancellation and cleanup."
parent: "sdk"
sources:
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/attachment_evidenc\
    e.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/reports.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/review_regressio\
    ns/cleanup_audit.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/coordination/fir\
    st_stop.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/coordination.rs"
sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
model:
  spec: tla/services/sdk/Cleanup.tla
  config: tla/services/sdk/Cleanup.cfg
  steps:
    Init: Start with work admission open.
    Stop: Fence work admission for this stop.
    Release: Confirm physical provider release.
    AuditAck: Acknowledge required cleanup evidence.
    AuditFail: Retain the failed evidence result.
    Retire: Retire the old active work owner.
    Finalize: Finalize the stop after physical and evidence results.
    Reopen: Reopen only after release, audit, retirement and finalization.
diagramLinks: {}
modelCheck:
  sourceRevision: "77a6b620af4d94b234294f6d273fd74b3dfb44c7"
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:37.521Z
  config: tla/services/sdk/Cleanup.cfg
  modelDigest: cf8ca4efc6127a2cfec52c5888d3922c9524af25f0ce10a0ccb0111dfddb0157
  states: 18
  held:
    - TypeOK
    - ReopenRequiresRelease
    - ReopenRequiresAudit
    - ReopenRequiresRetirement
    - ReopenRequiresFinalization
  broke: []
  unchecked: []
---
# Work admission and provider cleanup

When work is stopped, Nessa blocks new work for that owner and finishes the accepted work's cancellation and cleanup. It keeps the first cancellation cause instead of replacing it with a later caller's reason.

Cancelled work, a stopped process, released files and a saved audit record are separate facts. Recovery waits for the required cleanup and settlement. If cleanup is uncertain, Nessa retains the owner and retries rather than assuming the resources are free.

```mermaid
stateDiagram-v2
    [*] --> Open
    Open --> Stopping: start_stop / fence generation and notify affected owners
    Open --> Blocked: CleanupRequired or unconfirmed CleanupReported [current work generation] / fence admission before cleanup starts
    Open --> Blocked: Confirmed CleanupReported [audit failed] / retire provider and fence admission
    Open --> Open: Confirmed CleanupReported [audit ok] / retire provider resources
    Stopping --> Stopping: Concurrent close / join current attempt
    Stopping --> Blocked: Final cleanup unconfirmed
    Blocked --> Stopping: Owned automatic cleanup or explicit close retry [new cleanup attempt] / retain stop cause and resource owner
    Stopping --> Open: Finalized [resource release confirmed and audit ok and recovery allowed and old active owners retired]
    note right of Stopping
        Physical release alone is insufficient.
        Some automatic stops preserve waiting inputs for restoration.
        Explicit close cancels waiting inputs.
    end note
```

## Bounded rule model

This model follows one stop with recovery permitted, one old active work owner,
and one possible reopening. Physical release, evidence acknowledgement and owner
retirement can arrive in different orders. Reopening requires confirmed release,
acknowledged evidence, finalized settlement and retirement of old active work.
A failed audit remains failed even after physical release.

The model abstracts away generation identities, first-caller attribution,
waiting inputs that may survive automatic recovery, retries of uncertain physical
cleanup, sealed ownership lifetimes and descendant closure. It does not claim
that all work permits must retire: the implementation permits waiting work and
current-generation owners. The chart's Blocked paths remain explained above;
this model covers the successful physical result and both audit results. With no
fairness assumption it makes no claim that cleanup or reopening eventually
happens. Source and the linked Rust regressions were reviewed at the page
checkpoint; Rust tests were not run in this refresh.

Run the catalog and models with the [state verification commands](../../../../scripts/state-docs/README.md). A TLC result checks this design abstraction, not the implementation.

## Further reading

[Source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs) · [Related source](../../../../crates/nessa-sdk/src/application/agent_execution/agents/attachment_evidence.rs) · [Related tests](../../../../crates/nessa-sdk/tests/application/agent_execution/agents/review_regressions/cleanup_audit.rs)
