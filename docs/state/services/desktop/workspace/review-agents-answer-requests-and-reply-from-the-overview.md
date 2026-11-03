---
id: "desktop-workspace-review-agents-answer-requests-and-reply-from-the-overview"
title: "review agents, answer requests, and reply from the overview"
kind: "statechart"
status: "mixed"
summary: "The overview groups sessions that need your attention, are running, or have finished."
parent: "desktop-workspace"
sources:
  - "src/desktop/workspace/ui/overview/overview.tsx"
  - "src/desktop/workspace/ui/overview/session-peek.tsx"
  - "src/desktop/workspace/ui/overview/overview-keys.ts"
  - "src/desktop/workspace/ui/overview/settling.ts"
  - "src/desktop/workspace/application/usecases/overview.ts"
  - "src/desktop/workspace/adapters/store/commands.ts"
  - "src/desktop/workspace/adapters/store/session-commands.ts"
  - "src/desktop/workspace/ui/overview/overview.test.tsx"
  - "src/desktop/workspace/model/overview/agents-glance.test.ts"
diagramLinks: {}
---

# review agents, answer requests, and reply from the overview

The overview groups sessions that need your attention, are running, or have finished. Opening one takes you to its conversation or waiting request.

Answers and replies use the selected workspace source. In [sample mode](README.md#connection-modes), these cards do not prove that live subagents were started or that a provider received an approval.

```mermaid
stateDiagram-v2
    [*] --> Browsing
    Browsing --> SeenApproval: Select row [approval visible]
    SeenApproval --> Answering: Decide [seen approval id current] / reserve token
    Answering --> Settling: Replacement no longer asks
    Answering --> Unconfirmed: Refused or unknown / retain feedback
    Unconfirmed --> Answering: Retry [same approval still visible]
    Unconfirmed --> Settling: Replacement no longer asks
    Settling --> Browsing: Settle row / choose next request
    SeenApproval --> Browsing: Approval replaced or session opened
    Answering --> Answering: Duplicate press / return answering
    note right of SeenApproval
        One approval answer in the fixture workspace.
        Pane and overview share reservation authority.
        Late answer failure must match token and approval.
        Peek does not mark the session read.
    end note
```

## Further reading

[Source](../../../../../src/desktop/workspace/ui/overview/overview.tsx) · [Related source](../../../../../src/desktop/workspace/ui/overview/session-peek.tsx) · [Related tests](../../../../../src/desktop/workspace/ui/overview/overview.test.tsx)

## Model coverage

Disposition: shared owner model. Question-flight identity and SDK review ownership are shared decisions. [desktop-question-flights](question-answer-flights.md) [sdk-reviews](../../sdk/reviews.md)

Excluded: Overview filter retention, animation, reply outbox and card focus are not modeled.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
