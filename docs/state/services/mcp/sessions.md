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
