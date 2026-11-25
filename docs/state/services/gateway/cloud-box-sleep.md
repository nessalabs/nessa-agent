---
id: "gateway-cloud-box-sleep"
title: "Put a persistent cloud gateway to sleep"
kind: "operation"
status: "mixed"
summary: "Sleep checks gateway activity before stopping the VM and recording its
  new state."
parent: "gateway"
sourceRevision: "33da134e2a334e05c385d54cca6f6284acc0448e"
sources:
  - "crates/nessa-conversation/src/workspaces/application/cloud_boxes.rs"
  - "crates/nessa-conversation/src/workspaces/infrastructure/cloud_boxes.rs"
  - "docs/guides/headless-gateway/nessa-gateway-bootstrap"
diagramLinks: {}
model:
  spec: tla/services/gateway/CloudBoxSleep.tla
  config: tla/services/gateway/CloudBoxSleep.cfg
  steps:
    Init: Start with a running gateway.
    Observe: Read an activity snapshot.
    Activity: Gateway work starts or finishes.
    ProviderReply: The provider returns its stop outcome.
    Persist: Try to record the sleeping receipt.
    Audit: Try to deliver the audit event.
modelCheck:
  sourceRevision: "33da134e2a334e05c385d54cca6f6284acc0448e"
  checker: tlc-1.7.4
  date: 2026-11-25T05:04:53.037Z
  config: tla/services/gateway/CloudBoxSleep.cfg
  modelDigest: 44c8d40e0f304a5ef29831d7a6f79c8655fb0fa34e7104a383f8261bbf501ecb
  states: 20
  held:
    - IdleEvidenceBeforeStop
    - EffectBeforeReceipt
    - SuccessRequiresReceipt
  broke: []
  unchecked: []
---
# Put a persistent cloud gateway to sleep

The per-box lock serializes box lifecycle calls on this service instance. Conversation and terminal admission happen on the gateway. A successful busy probe supplies a snapshot. It gives the caller no reservation against new work.

~~~mermaid card=lifecycle
stateDiagram-v2
    [*] --> Probe
    Probe --> Refused: Busy or unknown
    Probe --> Stopping: Idle snapshot
    Stopping --> Failed: Provider reports failure
    Stopping --> Recording: Provider confirms stop
    Recording --> Unrecorded: Receipt write fails
    Recording --> Auditing: Sleeping receipt saved
    Auditing --> AuditFailed: Evidence delivery fails
    Auditing --> Done: Evidence delivered
~~~

The model starts with one running box and one sleep request. Activity can change between any two steps while the provider is running. Provider success, receipt persistence and audit delivery are separate transitions. A receipt failure can leave the VM stopped and its latest receipt running. An audit failure can leave a sleeping receipt despite a caller-visible error. The model omits concurrent processes, uncertain provider effects, restarts, already-sleeping requests and the individual list reads inside a probe. It makes no progress claim.

The baseline checks the ordering guarantees for a reliable idle observation. The bootstrap change accompanying this page tightens what counts as reliable for both activity lists and an unhealthy systemd unit. The checkpoint identifies the base reviewed.

To check the stronger sleep guarantee, copy CloudBoxSleep.cfg to a temporary file, append the line **INVARIANT NoWorkAtStop**, and run TLC with that config on CloudBoxSleep.tla. The shortest witness observes idle, admits work, then stops. The prospective fix needs the gateway to close admission atomically with confirming that work has drained. A second poll still leaves that interval open. Provider stop failures and a lost reply need an explicit recovery path before deploying such a change.
