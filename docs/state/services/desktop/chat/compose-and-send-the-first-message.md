---
id: "desktop-chat-compose-and-send-the-first-message"
title: "compose and send the first message"
kind: "flow"
status: "mixed"
summary: "Open a new conversation, choose a model and write a message."
sourceRevision: "11e2746f7df088a1bd5ee3b2e579ad8a40b5cbd1"
parent: "desktop-chat"
sources:
  - "src/panel/ui/use-composer.ts"
  - "src/conversation/ui/use-conversation.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/send-draft.ts"
  - "src/conversation/adapters/gateway/effects.ts"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "crates/nessa-server/src/product/conversation.rs"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/submissions.rs"
  - "src/panel/ui/use-composer.test.ts"
  - "src/panel/application/composer-expansion.test.ts"
  - "src/conversation/adapters/store/unsent.test.ts"
  - "docs/guides/gateway-chat.md"
  - "src/conversation/application/usecases/usecases.test.ts"
  - "packages/nessa-client/src/presentation/conversation-api.test.ts"
  - "crates/nessa-server/tests/conversation/application.rs"
diagramLinks: {}
diagramDetails:
  - diagram: 2
    node: AwaitingDraft
    section: Composer ownership while a send waits
  - diagram: 2
    edge:
      from: AwaitingDraft
      to: Retired
      label: Empty draft [same conversation] / collapse editor
    section: Composer ownership while a send waits
---

# compose and send the first message

Open a new conversation, choose a model and write a message. Enter sends it; Shift+Enter starts a Markdown block. Text can be empty when there are sendable attachments. The voice button has no working voice action yet.

Send keeps the original message identity and content while it waits for acceptance. Acceptance means Nessa owns the input. The external agent may still be starting. Pending or failed uploads, unsupported attachments and oversized text prevent sending. The panel binds its initial model choice when it creates the remote conversation. The desktop workspace also supports [staging a model change before the next send](../../sdk/runtime/select-a-model-approval-preset-or-reasoning-effort.md); that control does not imply a second conversation.

```mermaid
stateDiagram-v2
    state "Local draft" as Draft
    state "Submitting" as Submitting {
        [*] --> Creating
        state "Create or reuse conversation" as Creating
        state "Await admission receipt" as Admission
        Creating --> Admission: Conversation ready / submit stable IDs
    }
    state "Admitted input" as Admitted
    state "Provider execution" as Executing
    state "Delivery unknown" as Unknown
    [*] --> Draft
    Draft --> Draft: Type or choose an unbound model
    Draft --> Draft: Send [locally declined] / show reason
    Draft --> Submitting: Send [allowed] / retain IDs and content
    Submitting --> Draft: Known refusal / recover draft if still empty
    Admission --> Unknown: Acknowledgement uncertain
    Admission --> Admitted: Receipt confirmed
    Admitted --> Executing: Provider attached and scheduler admits
    Executing --> Settled: Outcome retained
    Unknown --> Admission: Explicit Retry / reuse IDs and content
    note right of Admitted
        Admission and provider readiness are separate facts.
        Follow-ups have their own submission state.
    end note
```

## Who handles the send

```mermaid
sequenceDiagram
    actor Person
    participant Panel
    participant Gateway
    participant SDK
    participant Agent as External agent
    Person->>Panel: Send the draft
    Panel->>Gateway: Create or reuse the conversation
    Panel->>Gateway: Submit original message ID and content
    Gateway->>SDK: Admit input to the conversation
    SDK-->>Gateway: Acceptance receipt
    Gateway-->>Panel: Input accepted
    Note over SDK,Agent: Agent startup can still be pending
    SDK->>Agent: Run when ready and scheduled
    Agent-->>SDK: Output and outcome
    Panel->>Gateway: Read current conversation view
    Gateway-->>Panel: Updated view
```

## Further reading

[Source](../../../../../src/panel/ui/use-composer.ts) · [Related source](../../../../../src/conversation/ui/use-conversation.ts) · [Related tests](../../../../../src/panel/ui/use-composer.test.ts)

## Composer ownership while a send waits

The expanded editor closes when its own submitted draft leaves. Switching tabs gives the selected tab a fresh collapsed editor; subsequently expanding that tab is a new local choice. A pending send from another tab must not collapse it when its draft is cleared.

Submission callbacks also carry operation identity. An old refusal clears only its own pending observation, so it cannot prevent a newer accepted send from closing its editor. The correction is local presentation; the conversation store remains the admission and prepared-command owner.

This small chart follows one pending observation, rather than the transport or the expanded editor itself. A replacement observation has a new identity. A promise callback for an older one cannot clear that replacement.

```mermaid
stateDiagram-v2
    [*] --> AwaitingDraft: Submit passes attachment-read check / capture tab and observation
    AwaitingDraft --> Retired: Empty draft [same conversation] / collapse editor
    AwaitingDraft --> Retired: Refused [same observation] / keep editor open
    AwaitingDraft --> Retired: Another submit / replace observation
    AwaitingDraft --> AwaitingDraft: Different tab draft changes / retain observation
    Retired --> [*]
```

The [CO1–CO4 design table](../../../../design/user-flow-ordering.md#composer-draft-observation) owns these orderings. The [hook regressions](../../../../../src/panel/ui/use-composer-ordering.test.tsx) exercise the actual hook through a form with controlled submission promises; they do not establish provider completion.

This is a bounded correction to the composer owner. The other linked source files have no implementation changes since the earlier checkpoint. The checkpoint advances to the committed correction after reconciling that hook and its regression tests; the other source owners retain their unchanged implementation.
