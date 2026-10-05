---
id: "nessa"
title: "Nessa system"
kind: "system"
status: "mixed"
summary: "These pages explain what happens when you use Nessa and which part handles each step."
parent: null
sources:
  - "docs/ARCHITECTURE.md"
  - "src/main.tsx"
  - "src/desktop/main.tsx"
  - "crates/nessa-server/src/composition/mod.rs"
diagramLinks:
  Host: "host"
  Desktop: "desktop"
  Client: "client"
  Gateway: "gateway"
  SDK: "sdk"
  MCP: "mcp"
  Auth: "auth"
  Storage: "storage"
  Images: "images"
  Endpoint: "endpoint"
  Credentials: "credentials"
  Extensions: "extensions"
  Tooling: "tooling"
---

# Nessa system

These pages explain what happens when you use Nessa and which part handles each
step. Choose a diagram box to open a service, then a feature and its individual
flows. The sidebar gives another route through the same pages.

## System overview

This is a **component map**, not a state machine: these components coexist.
Arrows describe selected composition and data relationships, not exclusive states.
The external provider process is intentionally a boundary, not a fictional
Nessa service. Some desktop flows are fixture-backed; their pages retain that fact.

```mermaid
flowchart TB
    Host[Native host]
    Desktop[Desktop and panel]
    Client[Product client]
    Gateway[Local gateway]
    SDK[Agent SDK]
    Provider[External provider runtime]
    MCP[MCP sessions and tools]
    Auth[Identity and access]
    Storage[Local storage and database]
    Images[Image normalization]
    Endpoint[Endpoint discovery]
    Credentials[Agent credential values]
    Extensions[Extensions and UI integration]
    Tooling[Developer and release tooling]
    Host -->|mounts surfaces| Desktop
    Host -->|manages release gateway| Gateway
    Desktop -->|product commands| Client
    Client -->|authenticated session| Gateway
    Gateway -->|agent lifecycle| SDK
    SDK -->|ACP| Provider
    Gateway -->|access decisions| Auth
    Gateway -->|retained evidence| Storage
    Gateway -->|image pipeline| Images
    Gateway -->|session grants| MCP
    Host -->|published listener| Endpoint
    Gateway -->|configured secrets| Credentials
    Extensions -->|host integration| Desktop
    Tooling -->|build artifacts| Host
```

## Opening Nessa and sending a message

Nessa lets a person work with an external agent through a conversation. In the native panel, the host starts or checks the gateway and the panel signs in. Send accepts the input before the agent necessarily starts it. The panel then reads updated conversation views.

```mermaid
sequenceDiagram
    actor User
    participant Host as Native host
    participant Panel as Floating panel
    participant Client as Product client
    participant Gateway as Local gateway
    participant SDK as Agent SDK
    participant Provider as External agent
    User->>Host: Open Nessa
    Host->>Gateway: Start or validate configured gateway
    Host->>Panel: Supply host state and surface credential
    Panel->>Client: Establish authenticated product session
    Client->>Gateway: Authenticate native session
    Note over Host,Gateway: See Startup and access
    User->>Panel: Compose and send
    Panel->>Client: Create/read conversation
    Client->>Gateway: Authorized conversation command
    opt New conversation
        Gateway->>SDK: Create Agent and begin background attachment
        SDK->>Provider: Open provider session
    end
    Panel->>Client: Submit stable input ID
    Client->>Gateway: Authorized send
    Gateway->>SDK: Admit work to conversation's Agent
    Note over Panel,Gateway: See Chat and Attachments
    Note over SDK,Provider: Admission may finish before attachment, invocation waits for readiness
    SDK->>Provider: Invoke admitted input once attached
    Provider-->>SDK: Text, tools, permission requests, outcome
    Note over SDK,Provider: See Runtime and tools
    Panel->>Client: Poll current conversation view
    Client->>Gateway: Read current conversation view
    Gateway-->>Panel: Replacement view through client
    Panel-->>User: Render output and available controls
```

The browser connects to an existing gateway using its browser session. The native desktop also reads the local gateway; an ordinary browser preview uses sample data. This native panel sequence does not describe every screen. See [desktop connection modes](services/desktop/workspace/README.md#connection-modes).

## Browse the system

| Area | Start here |
| --- | --- |
| Native startup, onboarding, windows and managed gateway | [Native host](services/host/README.md) |
| Conversation surface, tabs, workspace, panes and widgets | [Desktop and panel](services/desktop/README.md) |
| Connection authority, wire requests and uncertainty | [Product client](services/client/README.md) |
| Product admission, current views, attachments and app calls | [Local gateway](services/gateway/README.md) |
| Provider attachment, scheduling, execution and durable history | [Agent SDK](services/sdk/README.md) |
| Upstream sessions, relay grants, app tools and local shell | [MCP](services/mcp/README.md) |
| Credentials, membership, access and refusal evidence | [Identity and access](services/auth/README.md) |
| Publication, durability and database opening | [Local storage](services/storage/README.md) |
| Stateless image processing | [Images](services/images/README.md) |
| Endpoint discovery and listener publication | [Endpoint](services/endpoint/README.md) |
| Validated agent secret namespaces | [Credential values](services/credentials/README.md) |
| MCP Apps and external UI package boundaries | [Extensions and UI](services/extensions/README.md) |
| Runtime assembly and release operations | [Tooling](services/tooling/README.md) |

## How this stays useful

- [Repository sweep](inventory.md): what was inspected and where state is owned.
- [Gaps and improvements](gaps.md): evidence-backed limits and next work.
- [Authoring and maintenance](authoring.md): document contract and statechart notation.
- [Risk evidence](risks.md): existing reproductions, corrections and exclusions.
- [Flow-map provenance](provenance.md): historical baselines, contributing PRs and original inspection limits.

Charts are **reading models**. Source types and their owning transitions remain
runtime authority; linked tests are evidence to inspect, not a claim they ran in
this sweep. New pages distinguish implemented, fixture, proposed, mixed, and
reference content. The old flow maps have moved into this hierarchy; they no
longer have a separate authoritative location.
