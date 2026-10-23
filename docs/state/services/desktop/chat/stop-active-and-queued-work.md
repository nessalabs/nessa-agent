---
id: "desktop-chat-stop-active-and-queued-work"
title: "stop active and queued work"
kind: "flow"
status: "mixed"
summary: "Stop targets the captured turn: withdraw it from the queue or cancel it if active."
parent: "desktop-chat"
sources:
  - "src/panel/ui/app.tsx"
  - "src/conversation/adapters/store/slice.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/lifecycle.rs"
  - "crates/nessa-server/src/conversation/application/service/mutation.rs"
  - "crates/nessa-server/tests/conversation/mutation_commands.rs"
  - "src/conversation/application/usecases/control-failure.ts"
  - "src/conversation/adapters/store/control-failure.test.ts"
  - "crates/nessa-server/tests/conversation/application.rs"
  - "crates/nessa-server/tests/conversation/close_release.rs"
  - "crates/nessa-server/tests/conversation/attachments.rs"
diagramLinks: {}
---

# stop active and queued work

Stop targets the captured turn: withdraw it from the queue or cancel it if active. Other queued turns and the agent attachment remain available.

The panel captures the running turn, or its latest unsettled submission, together with its conversation identity. A delayed Stop cannot target a different conversation after tab rebinding. Stop can overtake a pending send; the prepared operation remembers it and asks again after admission. Closing the attachment, deleting a conversation and detaching a tab are separate operations. Stop does not release uploaded images or prove external tool effects were reversed.

```mermaid
stateDiagram-v2
    [*] --> Captured: Capture conversation and turn
    Captured --> Requesting: Stop / retain prepared stop intent
    Requesting --> Withdrawn: Named turn still queued / remove only that turn
    Requesting --> CancelRequested: Named active turn / provider accepts cancellation
    Requesting --> AlreadyFinal: Named turn has ended / send no cancel
    Requesting --> Refused: Missing target, unsupported cancel or typed refusal
    Requesting --> Unknown: Reply lost or original attempt interrupted
    Unknown --> Reconciling: Panel follows current view / preserve uncertainty
    Withdrawn --> Refreshed: Follow authoritative view
    CancelRequested --> Refreshed: Follow actual turn settlement
    AlreadyFinal --> Refreshed: Follow authoritative view
    Refused --> Refreshed: Show failure and refresh
    Reconciling --> Refreshed: Preserve known or uncertain outcome
    note right of CancelRequested
        Cancellation acknowledgement is not process cleanup.
        Other queued turns and image holds are not closed.
    end note
```

The [mutation command regressions](../../../../../crates/nessa-server/tests/conversation/mutation_commands.rs) cover queue-to-active races, a concurrent close, unsupported cancellation, finished turns and receipt recovery. They distinguish the durable Stop command from the eventual turn outcome. The panel refreshes the current view after failure; it does not automatically look up or replay a Stop receipt. Explicit command clients can read the original receipt with its exact binding.

## Further reading

[Source](../../../../../src/panel/ui/app.tsx) · [Related source](../../../../../src/conversation/adapters/store/slice.ts) · [Related tests](../../../../../src/conversation/adapters/store/control-failure.test.ts)
