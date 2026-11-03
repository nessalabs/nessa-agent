---
id: "sdk-runtime-the-first-agent-launch-is-warmed-in-the-background"
title: "the first agent launch is warmed in the background"
kind: "operation"
status: "mixed"
summary: "Nessa can prepare the first agent launch by opening and closing a
  disposable session without sending a prompt."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/agent_warm_up/application/service.rs"
  - "crates/nessa-server/src/agent_warm_up/application/terminal.rs"
  - "crates/nessa-server/src/agent_warm_up/domain/value_objects/runtime_fingerp\
    rint.rs"
  - "crates/nessa-server/src/agent_warm_up/infrastructure/records.rs"
  - "crates/nessa-server/src/composition/warm_up.rs"
  - "crates/nessa-server/tests/agent_warm_up/service.rs"
  - "crates/nessa-server/tests/composition/current_warm_up.rs"
  - "crates/nessa-server/tests/conversation/prepared_runtime.rs"
diagramLinks: {}
model:
  spec: tla/services/sdk/WarmUp.tla
  config: tla/services/sdk/WarmUp.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:19.788Z
  config: tla/services/sdk/WarmUp.cfg
  modelDigest: a526160e9e85408fd86d53750820caefd152b3bdf11ca5b757a75864580a5ea0
  states: 9
  held:
    - TypeOK
    - CompletionNeedsRelease
    - RetainedNotPrepared
  broke: []
  unchecked: []
---
# the first agent launch is warmed in the background

Nessa can prepare the first agent launch by opening and closing a disposable session without sending a prompt. A conversation arriving during this warm-up joins the existing run rather than starting another.

Warm-up is separate from the conversation's own result. Process release, saved audit and completion-record delivery can succeed or fail independently. Another automatic launch must wait when the previous run may still hold resources.

```mermaid
stateDiagram-v2
    [*] --> Observing: Resolve exact current fingerprint
    Observing --> AlreadyPrepared: Matching durable completion
    Observing --> Opening: Admit self-owned disposable Agent
    Opening --> Closing: Open settles / no prompt sent
    Closing --> Released: Physical resource ownership released
    Closing --> Retained: Ownership uncertain / fence automatic relaunch
    Released --> Recording: Record preparation effect and audit
    Recording --> Warmed: Completion record acknowledged
    Recording --> StillCold: Audit or completion record failed
    StillCold --> Observing: Later observation may retry after release
    note right of Retained
        Settled failure does not prove release.
        Conversation waiters join this owner but
        later open their own provider context.
    end note
```

## Further reading

[Source](../../../../../crates/nessa-server/src/agent_warm_up/application/service.rs) · [Related source](../../../../../crates/nessa-server/src/agent_warm_up/application/terminal.rs) · [Related tests](../../../../../crates/nessa-server/tests/agent_warm_up/service.rs)

## Model coverage

Direct model. One fingerprint-bound self-owned disposable launch, physical release versus retained ownership and independent audit/completion-record delivery.

Fingerprint bytes, prior durable-record lookup, multiple warmup coordinators and provider launch behavior are source/adapter contracts. A failed completion record is still a prepared effect but is not a durable Warmed record. Safety only; terminal stuttering is allowed and there is no liveness or implementation-proof claim. This scoped source review adds the abstraction at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints remain unchanged. Source tests were inspected, not executed.
