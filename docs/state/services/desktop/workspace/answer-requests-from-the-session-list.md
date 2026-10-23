---
id: "desktop-workspace-answer-requests-from-the-session-list"
title: "answer requests from the session list"
kind: "statechart"
status: "implemented"
summary: "In three columns, each waiting row the session list asks about is a request card, answered from the list."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/ui/session-list/list-request.tsx"
  - "src/desktop/workspace/ui/session-list/session-list.tsx"
  - "src/desktop/workspace/application/workspace-state.ts"
  - "src/desktop/workspace/model/session-groups.ts"
  - "src/desktop/workspace/adapters/store/session-commands.ts"
  - "src/desktop/workspace/ui/session-list/list-request.test.tsx"
  - "verification/desktop/scripts/conversation-stress.mjs"
diagramLinks: {}
---

# answer requests from the session list

In three columns, the session list leads with the channel's pinned sessions,
then groups the rest Needs you, Running and Earlier. Each row in Needs you that
the list asks about — as its search lists it, the newest twelve
(`listAskIds`) — is a request card: who asks, the command as the transcript
card reads it, and the review's refusal and primary allow, every other answer
in the menu beside them. Those sessions are on screen: the window reads and
keeps their conversations, and answers what their cards show. A pinned
session, a row the search hides, a row past the twelve, or any row while the
list is folded or under the Agents overview, asks nothing from the list.

```mermaid
stateDiagram-v2
    [*] --> Row: a session waits on the person
    Row --> Card: asked by the list (Needs you, searched, newest twelve)
    Card --> Row: pinned, searched away, past twelve, list folded or covered
    Card --> Answering: refusal, allow or a menu answer
    Card --> Chat: Review in Chat, when the command is taller than the card
    Answering --> [*]: taken / the card fades and the row leaves Needs you
    Answering --> Card: refused / the reason in place
    note right of Card
        What a card shows is what the window answers (onScreen).
        Nothing is answered for a session no card shows.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/session-list/list-request.tsx) · [Related source](../../../../../src/desktop/workspace/application/workspace-state.ts) · [Related tests](../../../../../src/desktop/workspace/ui/session-list/list-request.test.tsx)
