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
        Reconnect restores transport and the tab's follow.
        It does not replay commands.
    end note
```

## Several refused messages

Each refused message keeps its own error and diagnostic. The conversation's
error belongs to whichever failure happened last, and one notice slot shows
one failure at a time. A refused message is never lost from the slot: once
nothing newer holds it, the latest refused message is offered again. Whether
the conversation's error is a refused message's own is recorded with it, as
the turn that set it, and never read from the text: a control can fail in the
same words.

| # | State | Event | Notice shows | Conversation error |
|---|-------|-------|--------------|--------------------|
| 1 | One refused message | none | Error sending for it | its error |
| 2 | Two refused messages | Send Again on the newer, accepted or still sending | Error sending for the older, with its own error; Send Again puts it back in the empty draft (files to upload again) and sends it | cleared |
| 3 | Two refused messages | Send Again on the newer, refused again | Error sending for the newer | the newer's new error |
| 4 | A refused message | a control (Close, an answer) fails afterwards | the control failure | the control's error |
| 5 | A refused message and a later control failure | Discard the refused message | the control failure | unchanged |
| 6 | Two refused messages, the newer's error current | Discard the newer | Error sending for the older | the older's error |
| 7 | A refused message | a control fails afterwards in the same words | the control failure | the control's error |
| 8 | Two refused messages, the older pointing at a file by path | Send Again on the newer, accepted; then Send Again on the older | the older is sent at once, since a file sent by path has nothing to upload | cleared |

Tests: rows 1 to 4 and 7 in `src/conversation/ui/notification.test.ts`; rows
4 to 7 through the store in `src/conversation/adapters/store/unsent.test.ts`;
rows 2 and 8's Send Again through the rendered control in
`src/conversation/ui/conversation-notification.test.ts`, its attachments in
`src/conversation/application/usecases/attachments.test.ts`, and in a real
browser with `verification/desktop/scripts/refused-messages.mjs`.

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
