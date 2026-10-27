---
id: gateway-channel-workspaces
title: Channel workspaces
kind: statechart
status: implemented
sourceRevision: "a162d7ef7cc7abbe5f714f74737b34421ec15e6b"
summary: A channel independently places its main agent and new threads on Local or Boat.
parent: gateway
sources:
  - crates/nessa-server/src/conversation/application/placement.rs
  - crates/nessa-server/src/composition/local_auth.rs
  - crates/nessa-server/src/conversation/infrastructure/ssh_environment/audit.rs
  - crates/nessa-sdk/src/application/agent_execution/sessions/manager.rs
  - crates/nessa-sdk/src/infrastructure/claude_acp/sessions/binding.rs
  - crates/nessa-sdk/src/infrastructure/opencode_acp/sessions/binding.rs
  - crates/nessa-sdk/src/infrastructure/acp/sessions/config.rs
  - crates/nessa-sdk/src/application/agent_execution/providers/input_files.rs
  - crates/nessa-server/src/conversation/application/input_files.rs
  - crates/nessa-server/src/workspaces/infrastructure/settings.rs
  - crates/nessa-server/src/workspaces/infrastructure/release.rs
  - crates/nessa-server/src/workspaces/application/mod.rs
  - crates/nessa-server/src/workspaces/infrastructure/boat.rs
  - crates/nessa-server/src/workspaces/infrastructure/ssh.rs
  - crates/nessa-server/src/conversation/infrastructure/workspaces.rs
  - crates/nessa-server/src/conversation/infrastructure/channels.rs
  - src/desktop/workspace/ui/channel-creation/new-channel.tsx
  - src/desktop/settings/ui/boat-row.tsx
diagramLinks: {}
---

# Channel workspaces

This page reviews the alpha placement, allocation and main-chat lifecycle. It
does not claim deployment, provider availability or completion of cloud wake,
capacity scheduling, provider OAuth or live migration. Local remains the default
for both channel choices. The main agent runs on messages and worker reports.

```mermaid
stateDiagram-v2
    [*] --> Local: Local placement
    [*] --> NeedsConnection: Boat placement
    NeedsConnection --> Intent: unlocked saved token and new thread
    NeedsConnection --> Refused: locked or unavailable configuration
    Intent --> Assigned: persist returned VM identity
    Intent --> Uncertain: response lost or receipt not saved
    Assigned --> Ready: SDK workspace and pinned SSH lease
    Assigned --> Refused: preparation fails or VM missing
    Ready --> Assigned: close harness or gateway restart
    Assigned --> Ready: resume same archived VM and workspace
    Uncertain --> Refused: automatic replacement forbidden
```

A durable receipt holds the thread, original image, environment and VM identity.
A create intent is written first; uncertainty is retained rather than creating a
second resource. An observer disappearing does not cancel the allocation task's
receipt publication. Closing or deleting chat history preserves workspace files
and VM identity. Users manage VM stop/delete in Boat. Runtime TTL is one hour;
alpha has no idle parking or active TTL renewal.

The placement contract owns one retention bound shared by the operational
catalogue on every platform and the Unix connection cache. At capacity, only
unborrowed environments with no lease or command owner can be evicted; otherwise
a new opening is refused. Eviction does not delete the VM assignment.

Boat connects the user's existing provider subscription. Nessa stores only the
infrastructure connection in its encrypted vault. A matching prebuilt worker
image is required. Shipping checks reject a release pin that does not cover the
compiled lease protocol; an incompatible source build refuses token-only setup
until a matching image is published. The local gateway stays reachable and its vault must be
unlocked to provision. The SDK prepares the attached workspace; existing SSH
leases own agent process framing, approvals, accounting and cleanup.

Approved local access uses existing MCP connections. A pinned SSH connection
forwards only the gateway's scoped relay socket; local executable paths and local
provider credentials are omitted from hosted configuration. Removing an MCP
connection applies to new openings. Existing sessions keep theirs until closed;
closing and reopening revokes the old grants and uses the new set. There is no automatic local-folder copy or
mount, and local Nessa GitHub sign-in is not copied into remote Git.

Explicit uploaded-file inputs are a separate delivery contract. Their durable
references stay in conversation history. The resolver bound to that conversation's
opening lease prepares a transient link in the selected Local or remote workspace;
the SDK verifies correlation before dispatch. Preparation spends the original
operation deadline, and required placement evidence remains owned after caller
cancellation. This does not authorize mounting local folders or forwarding local
provider credentials. The shared hosted configuration projection accepts only the
selected host's explicit scoped MCP relay, including for OpenCode.

```mermaid
stateDiagram-v2
    [*] --> InitialMain: derive from channel ID
    InitialMain --> CurrentMain: main conversation opened
    CurrentMain --> CurrentMain: erase worker or retry creation
    CurrentMain --> Replacement: confirmed main erasure
    Replacement --> CurrentMain: read saved replacement generation
```

The channel store journals replacement and removes the erased main's link in one
transaction. The retired conversation links the committed deletion and its real
initiator. Retrying erasure preserves the replacement; permanent conversation
tombstones remain intact. Channel navigation chooses the current main before a
newer worker, and its header displays Local or Boat.

Allocation and encrypted-vault tests are in `tests/workspaces`. Channel migrations
and replacement history are exercised against real SQLite. The opt-in real
provider workflow and its limits are described in
[workspace verification](../../../../scripts/workspaces/README.md). The existing
backup omission of channel-owned data is tracked in
[issue 1161](https://github.com/nessalabs/nessa-agent/issues/1161); allocation receipts
and environment audit files are included in the EnvironmentAudit inventory.

An untouched Claude native session is not persisted until its first prompt. SDK
attachment asks the provider with the saved ID and whether any invocation was
admitted. Claude opens a new native context only for an uninvoked history; the
existing context transition records its replacement. A history with admitted work
still requires successful resume. Missing used native history is refused with
its saved inputs and context retained. VM identity and workspace files do not
change when the untouched native context is replaced.

The empty-history exception does not classify a fresh native context created by
switching agents or changing a workspace after earlier work. That existing cold
context case remains conservatively refused if native resume fails; tracking
admission per context is [issue 1164](https://github.com/nessalabs/nessa-agent/issues/1164).


Receipt-save failure retains the returned VM target in independent recovery
audit evidence before rollback, and records rollback observation afterward.
Failed or panicking audit ports do not skip cleanup; an unconfirmed cleanup or
audit failure remains visible in the typed recovery error with its target.
An accepted allocation task owns these steps after caller loss. Boat receives
`NESSA_THREAD_ID` as a non-secret per-VM correlation marker before creation, so
provider-side inspection still links the VM to the durable intent if both local
stores are unavailable. No automatic replacement is authorized. Recovery audit
files use the existing private, timestamped EnvironmentAudit store and backup
inventory; their thread ID links the original conversation creation/lease actor.
