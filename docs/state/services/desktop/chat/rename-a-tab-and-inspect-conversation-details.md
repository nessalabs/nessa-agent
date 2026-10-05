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
