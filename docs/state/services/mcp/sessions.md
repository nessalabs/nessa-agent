---
id: "mcp-sessions"
title: "Conversation grants and upstream sessions"
kind: "statechart"
status: "implemented"
summary: "An agent and its app need to talk to the same configured tool server."
parent: "mcp"
sources:
  - "crates/nessa-server/src/mcp_servers/infrastructure/grants.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/servers.rs"
  - "crates/nessa-sdk/src/infrastructure/mcp/stand_in.rs"
  - "crates/nessa-server/tests/mcp_servers/mod.rs"
  - "crates/nessa-sdk/tests/infrastructure/mcp/sessions.rs"
diagramLinks: {}
model:
  spec: tla/services/mcp/Sessions.tla
  config: tla/services/mcp/Sessions.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:12.344Z
  config: tla/services/mcp/Sessions.cfg
  modelDigest: 8899c729db6a01708279645cb598e03d5796bbd9d0802a525c3aa8ee0b3f6039
  states: 12
  held:
    - TypeOK
    - NoOpenAfterRevoke
    - PhysicalReleaseAfterLogicalClose
  broke: []
  unchecked: []
---
# Conversation grants and upstream sessions

An agent and its app need to talk to the same configured tool server. Nessa gives each provider opening a fresh access grant and routes their requests through a local relay rather than letting a tool choose an executable.

The grant is a secret token tied to that conversation's current opening. Revoking it prevents new use and closes logical sessions, but does not itself prove the external process stopped. An app can see only tools allowed for apps; model-visible tools can have different visibility. A failed tool-list refresh can leave the previous accepted list available.

```mermaid
stateDiagram-v2
    [*] --> Granted: Mint token / register conversation owner
    Granted --> Revoked: Grant owner dropped / remove digest
    note right of Revoked
        Refuse later hello/open under this owner.
        Close connections before returning; process stopping may follow.
    end note
```

```mermaid
stateDiagram-v2
    [*] --> Opening: Valid live grant and configured server
    Opening --> Open: initialize accepted / register under grant lock
    Opening --> Closed: Handshake failure, stop or revoked grant
    state Open {
        [*] --> Unlisted
        Unlisted --> Listed: Complete bounded tools list accepted
        Unlisted --> Unlisted: List fails / no cached tool authority
        Listed --> Listed: New complete list accepted / replace by order
        Listed --> Listed: List refresh fails / retain prior list and log
    }
    Open --> Closing: Stand-in ends, revoke, stop or explicit close
    Closing --> Closed: Close stdin / exit or kill process group
    Open --> Closed: Last owning handle dropped / immediate process kill
```

## Further reading

[Source](../../../../crates/nessa-server/src/mcp_servers/infrastructure/grants.rs) · [Related source](../../../../crates/nessa-sdk/src/infrastructure/mcp/servers.rs) · [Related tests](../../../../crates/nessa-server/tests/mcp_servers/mod.rs)

## Model coverage

Direct model: One conversation grant and upstream handshake, registration versus revocation under the grant gate, cached list retention and separate physical release.

The finite model checks safety only. Tool visibility/validation is a contract; multiple servers, transport frames, bounded list pagination and process group cleanup are outside this model. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.
