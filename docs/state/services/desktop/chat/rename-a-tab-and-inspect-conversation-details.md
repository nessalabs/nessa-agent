---
id: "desktop-chat-rename-a-tab-and-inspect-conversation-details"
title: "rename a tab and inspect conversation details"
kind: "flow"
status: "mixed"
summary: "Renaming changes the tab title in this window."
parent: "desktop-chat"
sources:
  - "src/panel/ui/app.tsx"
  - "src/conversation/ui/conversation-details.tsx"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/application/usecases/apply-view.ts"
  - "src/conversation/application/saved-tabs.ts"
  - "crates/nessa-protocol/src/conversation/catalogue_metadata.rs"
  - "src/conversation/ui/conversation-details.test.tsx"
  - "src/conversation/adapters/store/slice.test.ts"
  - "src/conversation/application/usecases/apply-view.test.ts"
diagramLinks: {}
model:
  spec: tla/services/desktop/chat/rename-a-tab-and-inspect-conversation-details.tla
  config: tla/services/desktop/chat/rename-a-tab-and-inspect-conversation-details.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:31.256Z
  config: tla/services/desktop/chat/rename-a-tab-and-inspect-conversation-details.cfg
  modelDigest: 648276471204e15c5d5a76ed2be6cb932fd4f08ae4b93ae4cc639fb98c83060b
  states: 55
  held:
    - EditedTitleRetained
    - OtherTabUnchanged
    - EmptyNoRename
  broke: []
  unchecked: []
---
# rename a tab and inspect conversation details

Renaming changes the tab title in this window. It does not rename the server conversation or change the agent's stored context. Blank names are ignored.

Another surface may still show the gateway title. Local titles are limited to 120 JavaScript string units, so an emoji at the boundary may be split. That is a documented concern requiring a rendering check, not a reproduced failure here.

```mermaid
stateDiagram-v2
    state "Gateway-derived local title" as Derived
    state "Locally edited title" as Edited
    [*] --> Derived
    Derived --> Derived: Replacement view / update title and runtime facts
    Derived --> Edited: Rename [trimmed title nonblank] / keep first 120 string units
    Edited --> Edited: Rename [trimmed title nonblank] / replace local title
    Edited --> Edited: Replacement view / preserve edited title, update runtime facts
    Derived --> Derived: Blank rename / ignore
    Edited --> Edited: Blank rename / ignore
    note right of Edited
        titleEdited belongs to this local tab.
        Browser snapshots preserve it with the server reference.
        No remote rename command exists.
    end note
```

## Further reading

[Source](../../../../../src/panel/ui/app.tsx) · [Related source](../../../../../src/conversation/ui/conversation-details.tsx) · [Related tests](../../../../../src/conversation/ui/conversation-details.test.tsx)

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

Two local tab identities distinguish locally edited names from gateway titles. Renaming targets its exact tab and marks the title as user-owned; later source projection cannot overwrite that choice. Empty trimmed names are refused. The 120-character bound and detail-sheet fields are immutable projection/format contracts. Details show source evidence and do not authorize a tool or restart an agent. Tab restoration has its own identity model. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
