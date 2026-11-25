---
id: "gateway-cloud-box-sleep"
title: "Put a persistent cloud gateway to sleep"
kind: "operation"
status: "mixed"
summary: "Sleep reads gateway activity before asking the cloud provider to stop
  the VM. The receipt can be saved before the VM stops."
parent: "gateway"
sourceRevision: "a7f1c4920f6623e6b1733734896a39cb0318f3e8"
sources:
  - "crates/nessa-conversation/src/workspaces/application/cloud_boxes.rs"
  - "crates/nessa-conversation/src/workspaces/infrastructure/cloud_boxes.rs"
  - "crates/nessa-conversation/src/workspaces/infrastructure/agent37.rs"
  - "crates/nessa-conversation/src/workspaces/infrastructure/boat.rs"
  - "docs/guides/headless-gateway/nessa-gateway-bootstrap"
  - "Cargo.lock"
diagramLinks: {}
model:
  spec: tla/services/gateway/CloudBoxSleep.tla
  config: tla/services/gateway/CloudBoxSleep.cfg
  steps:
    Init: Start with a running gateway.
    Observe: Read an activity snapshot.
    Activity: Gateway work starts or finishes.
    RequestStop: Ask the cloud provider to stop the VM.
    ProviderAccepted: Receive a successful reply from the provider.
    ProviderError: Receive an error that may leave a stop pending.
    StopEffect: The VM stops.
    Persist: Try to record the sleeping receipt.
    Audit: Try to deliver the audit event.
    Wake: Wake succeeds and saves a running receipt.
modelCheck:
  sourceRevision: "a7f1c4920f6623e6b1733734896a39cb0318f3e8"
  checker: tlc-1.7.4
  date: 2026-11-25T06:37:44.819Z
  config: tla/services/gateway/CloudBoxSleep.cfg
  modelDigest: 523694fb803f824220a81977cd56562f8f44354761f2f79aa5275ab3d98e1428
  states: 52
  held:
    - IdleEvidenceBeforeStop
    - AcknowledgementBeforeReceipt
    - SuccessRequiresReceipt
  broke: []
  unchecked: []
---
# Put a persistent cloud gateway to sleep

When you put a cloud box to sleep, the box service asks the gateway whether it is busy and sends a stop request if it reads idle. The box service serializes sleep and wake within its own process. The gateway can admit a turn or terminal while the provider is processing the stop.

## Current behavior

~~~mermaid card=lifecycle
stateDiagram-v2
    [*] --> Probe
    Probe --> Refused: Busy or unknown
    Probe --> Requesting: Idle snapshot
    Requesting --> AwaitingReply: Ask the provider to stop
    AwaitingReply --> Failed: Error or lost reply
    AwaitingReply --> Recording: Provider acknowledges request
    Recording --> Unrecorded: Receipt write fails
    Recording --> Auditing: Sleeping receipt saved
    Auditing --> AuditFailed: Evidence delivery fails
    Auditing --> Done: Evidence delivered
    Failed --> Woke: Wake returns a running endpoint
    Unrecorded --> Woke: Wake returns a running endpoint
    AuditFailed --> Woke: Wake returns a running endpoint
    Done --> Woke: Wake returns a running endpoint
~~~

The VM can stop before or after the provider reply, including after a lost reply. A receipt failure can leave the VM stopped with a running receipt. An audit failure can leave a sleeping receipt despite a caller-visible error. The pinned workspace SDK's Boat adapter returns on the stop API response. Its Agent37 adapter returns on lifecycle acknowledgement and can retry a transient or unknown response. Both readiness calls can accept a VM that still reports running. [Boat lifecycle](https://github.com/nessalabs/workspace-sdk/blob/7aea78061bdde4fa1f600674e6f37fa951f162f3/src/providers/sandbox/machines/account.rs#L281-L430) · [Agent37 lifecycle](https://github.com/nessalabs/workspace-sdk/blob/7aea78061bdde4fa1f600674e6f37fa951f162f3/src/providers/agent37/machines.rs#L240-L343)

## Sleep and wake ordering to enforce

| Ordering | Required outcome |
| --- | --- |
| A turn or terminal reservation wins before the gateway closes admission | Refuse sleep and keep the gateway available. |
| The gateway closes admission while idle before work arrives | Refuse new work while a stop can still take effect. |
| The gateway is idle and admission is closed | Save the stop attempt and its target before asking the provider. |
| The provider confirms that the request was refused without effect | Reopen admission for that stop attempt. |
| The provider acknowledges the request but the VM is running | Keep admission closed and wait for the stop to settle before recording sleep. |
| The stop reply is lost, including across a service restart | Recover the original attempt and query its terminal outcome before reopening admission. |
| Stop settles but saving the receipt fails | Recover the settled stop before another lifecycle operation and keep the receipt failure visible. |
| Wake succeeds after a settled stop | Reopen admission after the provider makes the admitted VM ready. |

The gateway must close admission at the points that accept internal work and reserve terminals, and restore that decision before accepting work on restart. The provider needs a way to prove that a pending stop has settled or can no longer take effect against the VM after wake. A stop attempt identity and a query for that attempt's terminal outcome would supply that proof. The current Boat stop call takes a VM identifier and returns a response without an operation identity. The Agent37 call discards the acknowledgement body. A fresh running status can precede a delayed stop, and another stop would introduce another outstanding request. [Boat stop request](https://github.com/nessalabs/workspace-sdk/blob/7aea78061bdde4fa1f600674e6f37fa951f162f3/vendor/sandbox-client/src/client.rs#L152-L165) · [Agent37 lifecycle request](https://github.com/nessalabs/workspace-sdk/blob/7aea78061bdde4fa1f600674e6f37fa951f162f3/src/providers/agent37/api.rs#L252-L264)

## Model coverage

The model covers one running box, one sleep request, and one possible wake following a submitted stop through the same service. Gateway work can change before the VM stops. The model omits concurrent box-service processes, process restarts, repeated and compensating provider stop requests, already-sleeping requests, and the reads inside the probe. Deadlock checking is disabled so the run may end after its last modeled event. The checked rules concern safety within those bounds.

The baseline and its negative controls check the ordering of idle evidence, provider acknowledgement, receipt writes, and reported success. To reproduce the remaining defects, run the same spec with a temporary copy of the config and append one of these invariants per run.

| Invariant | Counterexample |
| --- | --- |
| `NoWorkAtStop` | The gateway reads idle, then starts work before the VM stops. |
| `PhysicalStopBeforeReceipt` | The provider acknowledges the request and the service saves sleep while the VM runs. |
| `NoEarlierStopAfterWake` | The caller receives a stop error, wake succeeds while the VM still runs, and the earlier stop then takes effect. |

TLC 1.7.4 explored 52 distinct states in the baseline. The three safety rules held, their three negative controls each broke the expected rule, and each invariant in the table produced the listed counterexample. An independent source review checked the transitions and refined the scope to one wake after a submitted stop. These runs cover the abstract model. The bootstrap tests use stubbed commands, and no live provider was exercised.
