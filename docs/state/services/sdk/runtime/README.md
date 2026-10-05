---
id: "sdk-runtime"
title: "Runtime and tools"
kind: "feature"
status: "mixed"
summary: "These flows follow work after it reaches the gateway: opening an agent, executing input, answering requests, saving history and cleaning up."
parent: "sdk"
sources:
  - "crates/nessa-sdk/docs/agent_execution/README.md"
  - "docs/guides/gateway-chat.md"
  - "docs/ARCHITECTURE.md"
  - "crates/nessa-sdk/docs/agent_execution/agent.md"
  - "crates/nessa-server/src/product/conversation.rs"
  - "crates/nessa-sdk/src/application/agent_execution/sessions/message_commit_clock.rs"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "packages/nessa-client/src/application/conversation-mutation-error.ts"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
diagramLinks:
  F0: "sdk-runtime-a-conversation-opens-its-selected-provider"
  F1: "sdk-runtime-the-first-agent-launch-is-warmed-in-the-background"
  F2: "sdk-runtime-select-a-model-approval-preset-or-reasoning-effort"
  F3: "sdk-runtime-an-admitted-message-streams-through-the-sdk-and-settles"
  F4: "sdk-runtime-approve-deny-withdraw-or-answer-an-agent-s-review"
  F5: "sdk-runtime-an-mcp-tool-shares-its-upstream-session-with-its-app"
  F6: "sdk-runtime-an-app-calls-a-tool-through-gateway-policy-and-review"
  F7: "sdk-runtime-read-an-app-resource-once-and-release-its-ticket"
  F8: "sdk-runtime-an-app-sends-a-message-and-gives-the-model-context"
---

# Runtime and tools

These flows follow work after it reaches the gateway: opening an agent, executing input, answering requests, saving history and cleaning up.

The SDK coordinates those steps but the external agent owns its own process and context. Acceptance, provider completion, saved results and confirmed cleanup must remain distinct.

## Feature overview

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Feature["Browse this feature"]
    F0["A conversation opens its selected provider"]
    Feature --> F0
    F1["The first agent launch is warmed in the background"]
    Feature --> F1
    F2["Select a model, approval preset, or reasoning effort"]
    Feature --> F2
    F3["An admitted message streams through the SDK and settles"]
    Feature --> F3
    F4["Approve, deny, withdraw, or answer an agent's review"]
    Feature --> F4
    F5["An MCP tool shares its upstream session with its app"]
    Feature --> F5
    F6["An app calls a tool through gateway policy and review"]
    Feature --> F6
    F7["Read an app resource once and release its ticket"]
    Feature --> F7
    F8["An app sends a message and gives the model context"]
    Feature --> F8
```

```mermaid
flowchart TB
    Opening[Open selected provider]
    Warm[Warm first launch]
    Configuration[Select model or approval preset]
    Execute[Stream and settle admitted message]
    Review[Review or answer]
    Relay[MCP relay session]
    AppCall[App tool policy and review]
    Resource[One-use app resource]
    AppMessage[App message and model context]
    Install[Install pinned runtime]
    Stop[Stop and confirm cleanup]
    Restore[Restore committed history]
    Read[Authorized physical history read]
    Opening --> Execute
    Warm --> Opening
    Configuration --> Opening
    Execute --> Review
    Execute --> Relay
    Relay --> AppCall
    Relay --> Resource
    AppMessage --> Execute
    Install --> Opening
    Execute --> Stop
    Restore --> Opening
    Restore --> Read
```

## Browse flows

- [A conversation opens its selected provider](a-conversation-opens-its-selected-provider.md)
- [An admitted message streams through the SDK and settles](an-admitted-message-streams-through-the-sdk-and-settles.md)
- [An app calls a tool through gateway policy and review](an-app-calls-a-tool-through-gateway-policy-and-review.md)
- [An app sends a message and gives the model context](an-app-sends-a-message-and-gives-the-model-context.md)
- [An authorized receiver reads physical history without opening an agent](an-authorized-receiver-reads-physical-history-without-opening-an-agent.md)
- [An MCP tool shares its upstream session with its app](an-mcp-tool-shares-its-upstream-session-with-its-app.md)
- [Approve, deny, withdraw, or answer an agent's review](approve-deny-withdraw-or-answer-an-agent-s-review.md)
- [Install or replace a pinned agent runtime](install-or-replace-a-pinned-agent-runtime.md)
- [Read an app resource once and release its ticket](read-an-app-resource-once-and-release-its-ticket.md)
- [Restart and restore committed conversation history](restart-and-restore-committed-conversation-history.md)
- [Select a model, approval preset, or reasoning effort](select-a-model-approval-preset-or-reasoning-effort.md)
- [Stop cancels owned work and confirms process cleanup](stop-cancels-owned-work-and-confirms-process-cleanup.md)
- [The first agent launch is warmed in the background](the-first-agent-launch-is-warmed-in-the-background.md)
