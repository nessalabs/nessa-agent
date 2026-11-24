---
id: desktop-voice-call
title: Talk to a conversation by voice
kind: statechart
status: implemented
summary: A call belongs to one visible conversation and ignores earlier call
  completions.
parent: desktop-workspace
sources:
  - src/desktop/voice/application/voice-controller.ts
  - src/desktop/voice/application/voice-controller.test.ts
  - src/desktop/voice/application/ports.ts
  - docs/design/voice-chat.md
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/voice-call.tla
  config: tla/services/desktop/workspace/voice-call.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:28:02.838Z
  config: tla/services/desktop/workspace/voice-call.cfg
  modelDigest: f494d97936f0ff791d7e677b3fc3338fcf3c0bddc733e862b31a8dd5d3014919
  states: 21
  held:
    - CurrentConnection
    - NoReleasedConnection
    - PublicationIdentity
  broke: []
  unchecked: []
---
# Talk to a conversation by voice

Starting a call ends its predecessor. The gateway supplies a grant, then the selected realtime transport connects. A refused grant or connection leaves a visible failure. Ending during connection invalidates the call; a late connection is closed. A call outlives the panes that show its conversation and ends when its conversation is gone from the window. A new chat's home offers Talk too: the call is about the id the window chose, and its first send starts the chat.

Pause hangs the call up with the provider (the transport is closed, so nothing is billed, and a connect under way is dropped and its late connection closed) and keeps the call: what was heard and said, the turns it waits on, and admitted sends, which still reach the agent. Replies that end while paused are kept. Resume asks the gateway for a new grant and connects a new transport for the same conversation, hands it the chat so far and a recap of what was said, then hands over the kept replies. A resume that cannot connect fails the call; End while paused ends it.

The coordinator sends committed transcribed words once, in admission order. A send waits up to five seconds for its covered transcripts. A Stop withdraws admitted sends that have not dispatched and waits for a dispatch already underway. Accepted voice turns are tracked separately from messages typed in the composer; their replies wait until all voice turns and pending sends settle. Unknown delivery is said as unknown and is not read out as confirmed work. Microphone mute clears half-heard audio. Speaker mute controls output. End releases timers, watchers, waiters and transport.

```mermaid
stateDiagram-v2
 [*] --> Idle
 Idle --> Connecting: Start / bind conversation and generation
 Connecting --> Listening: Current transport connects
 Connecting --> Failed: Current grant or transport refuses
 Listening --> Working: Voice send admitted
 Working --> Speaking: Tracked turns end and no pending sends
 Speaking --> Listening: Response ends
 Listening --> Dropped: Transport closes
 Connecting --> Idle: End / retire generation
 Listening --> Idle: End, or its conversation is gone
 Connecting --> Paused: Pause / drop the connect
 Listening --> Paused: Pause / close transport
 Working --> Paused: Pause / close transport, keep awaited turns
 Speaking --> Paused: Pause / close transport
 Paused --> Connecting: Resume / new grant, recap
 Paused --> Idle: End
```

The controller tests enumerate V1–V22 (V19–V22 are pause and resume), including late connection release, ordered transcript waits, stop before dispatch and reply provenance. They were inspected, not executed here. Provider audio capture, transcription quality and physical microphone release are external transport boundaries. The panel composer voice button and the desktop voice call are distinct surfaces.

## Bounded rule model

Two call generations explore end/restart racing with grant/transport completion and refused startup. A late connection belongs to the retired call and is closed. The model excludes pause and resume, transcript ordering, realtime speech scheduling and stop/send serialization, which remain explicit implementation/test scope above. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.

## Model coverage

Disposition: direct model. The bounded rule model section names the modeled owner slice and finite abstraction.

Excluded: All exclusions stated in the owning Bounded rule model section remain outside verification.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
