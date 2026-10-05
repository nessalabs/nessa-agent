---
id: desktop
title: Desktop and panel
kind: service
status: mixed
summary: "Nessa has a floating chat panel and a desktop workspace."
parent: nessa
sources:
  - src/main.tsx
  - src/composition/dependencies.ts
  - src/desktop/main.tsx
  - src/desktop/dependencies.ts
  - src/desktop/workspace/application/ports.ts
  - src/desktop/workspace/application/workspace-state.ts
  - src/desktop/widgets/app/model/lifecycle.ts
  - src/desktop/widgets/app/application/bridge.ts
  - src-tauri/src/desktop_window.rs
  - src-tauri/src/panel.rs
diagramLinks:
  Chat: desktop-chat
  Workspace: desktop-workspace
  App: desktop-mcp-app-view
  Gateway: gateway
  Host: host
---

# Desktop and panel

Nessa has a floating chat panel and a desktop workspace. The panel and native desktop use the local gateway. A browser preview can use an existing gateway session or sample data.

Use Chat for gateway-backed conversation behavior and Desktop workspace for panes, focus, windows and widgets. Gateway-backed workspaces use server-provided MCP Apps. Sample workspaces keep the fixture app. See [connection modes](workspace/README.md#connection-modes).

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Host[Native windows]
    Chat[Floating-panel conversation flows]
    Workspace[Desktop workspace and panes]
    App[MCP App view lifecycle]
    Gateway[Gateway conversation authority]
    Sample[In-memory sample source]
    Host --> Chat
    Host --> Workspace
    Chat -->|product client commands| Gateway
    Workspace -->|native or browser gateway mode| Gateway
    Workspace -->|sample browser preview| Sample
    Workspace -->|places and app mounts| App
```

## Browse this area

- [Chat](chat/README.md)
- [Desktop workspace](workspace/README.md)
