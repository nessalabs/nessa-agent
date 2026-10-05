---
id: "desktop-chat-recover-a-refused-or-unconfirmed-send"
title: "recover a refused or unconfirmed send"
kind: "flow"
status: "mixed"
summary: "A known refusal means the send was not accepted."
parent: "desktop-chat"
sources:
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/send-draft.ts"
  - "src/conversation/ui/notification.ts"
  - "packages/nessa-client/src/application/conversation-mutation-error.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/submissions.rs"
  - "crates/nessa-sdk/docs/agent_execution/scheduling.md"
  - "src/conversation/adapters/store/unsent.test.ts"
  - "src/conversation/ui/notification.test.ts"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "crates/nessa-sdk/tests/application/agent_execution/scheduling/retries.rs"
diagramLinks: {}
---

# recover a refused or unconfirmed send

A known refusal means the send was not accepted. Nessa can restore the unsent draft if doing so would not overwrite a newer draft.

A missing reply means the outcome is unknown. The original message may already be running. Explicit Retry uses its original IDs and content to recover the same submission. Reconnecting alone never resends it. Changing the content is a new action, not recovery of the old one.

```mermaid
stateDiagram-v2
    state "Awaiting send receipt" as Sending
    state "Message not sent" as Refused
    state "Delivery unknown" as Unknown
    state "Retrying original submission" as Retrying
    state "Original receipt or result recovered" as Recovered
    state "Conflict or unresolved evidence" as Unresolved
    [*] --> Sending
    Sending --> Refused: Known refusal or admission never attempted
    Refused --> [*]: Recover draft if not replaced
    Sending --> Unknown: Admission possible but acknowledgement lost
    Sending --> Recovered: Receipt confirmed
    Unknown --> Retrying: Explicit Retry / same operation, IDs, and content
    Retrying --> Recovered: Matching retained request
    Retrying --> Unresolved: SubmissionConflict or SubmissionUnresolved
    Retrying --> Unknown: Acknowledgement remains uncertain
    Unresolved --> Unknown: Fresh read / retain unconfirmed delivery
    Recovered --> [*]: Refresh authoritative view
    note right of Unknown
        Reconnect restores transport and polling.
        It does not replay commands.
    end note
```

## Recovering a lost reply

```mermaid
sequenceDiagram
    participant Panel
    participant Gateway
    participant SDK
    Panel->>Gateway: Submit message with stable ID
    Gateway->>SDK: Accept input
    SDK-->>Gateway: Receipt
    Note over Panel,Gateway: The reply is lost. The panel cannot know the outcome
    Note over SDK: Accepted work may continue
    Panel->>Gateway: Explicit Retry with the same ID and content
    Gateway->>SDK: Find or join original submission
    alt Original receipt or result is recoverable
        SDK-->>Panel: Original receipt or result through gateway
    else Previous delivery cannot be resolved
        SDK-->>Panel: Refuse replay through gateway
    end
```

## Further reading

[Source](../../../../../src/conversation/adapters/store/slice.ts) · [Related source](../../../../../src/conversation/application/usecases/send-draft.ts) · [Related tests](../../../../../src/conversation/adapters/store/unsent.test.ts)
