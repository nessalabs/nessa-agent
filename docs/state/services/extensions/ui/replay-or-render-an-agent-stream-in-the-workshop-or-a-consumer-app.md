---
id: "extensions-ui-replay-or-render-an-agent-stream-in-the-workshop-or-a-consumer-app"
title: "replay or render an agent stream in the workshop or a consumer app"
kind: "operation"
status: "mixed"
summary: "A stream of agent events can be replayed in the workshop or rendered by a consuming app."
parent: "extensions-ui"
sources: []
diagramLinks: {}
---

# replay or render an agent stream in the workshop or a consumer app

A stream of agent events can be replayed in the workshop or rendered by a consuming app. The parser turns those events into messages, tool activity and waiting requests for the UI.

Rendering a recorded stream does not run the agent again. The consuming app owns live transport, state and actions. Workshop examples demonstrate the component behavior rather than a production session.

```mermaid
stateDiagram-v2
    [*] --> SelectingTransport
    SelectingTransport --> Folding: Actual transport mapper and session filter
    Folding --> Folding: Ordered events / update transcript
    Folding --> Rebuilding: Backward seek
    Rebuilding --> Folding: Rebuild selected transcript from capture
    Folding --> Finished: Whole run completed / host finish where required
    Finished --> [*]
    note right of Folding
        Shared bus filtering and run completion are host responsibilities.
        Delta observations are not committed-message replay.
        Unrecorded capability is not promised support.
    end note
```
