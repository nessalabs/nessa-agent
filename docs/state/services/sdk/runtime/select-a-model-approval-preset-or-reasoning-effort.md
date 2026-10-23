---
id: "sdk-runtime-select-a-model-approval-preset-or-reasoning-effort"
title: "select a model, approval preset, or reasoning effort"
kind: "operation"
status: "mixed"
summary: "A conversation can change to a supported model on the same provider session."
parent: "sdk-runtime"
sources:
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/application/service/model_changes.rs"
  - "src/desktop/workspace/adapters/store/model-commands.ts"
  - "src/desktop/workspace/adapters/store/session-commands.ts"
  - "crates/nessa-server/tests/conversation/model_changes.rs"
  - "src/desktop/workspace/adapters/store/model-commands.test.ts"
  - "crates/nessa-server/src/conversation/infrastructure/mode_audit.rs"
  - "crates/nessa-server/src/composition/current_agent.rs"
  - "crates/nessa-sdk/src/application/agent_execution/agents/agent.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/sessions/thought_level.rs"
  - "crates/nessa-server/tests/conversation/application.rs"
  - "crates/nessa-server/tests/conversation/launch_configuration.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/agents/effort.rs"
  - "crates/nessa-sdk/tests/application/agent_execution/providers/operation_capabilities.rs"
  - "crates/nessa-sdk/docs/agent_execution/agent.md"
diagramLinks: {}
---

# select a model, approval preset, or reasoning effort

A conversation can change to a supported model on the same provider session. The gateway keeps the existing context and commits the provider's acknowledged model and approval preset together. The provider states whether the choice affects subsequent responses or the next turn.

In the desktop workspace, choosing a model stages it locally. Send applies that choice before submitting the next message; refusal or uncertainty prevents that send. Picking the current model again clears the staged choice. A native acknowledgement without a reconciled source view also keeps the message unsent. A cross-provider choice uses the separate agent-switch path; the model chart below covers same-provider changes. Approval-preset changes have their own idle checks and durable request journal. The Rust SDK also supports verified reasoning-effort changes for supported agents, but that alone does not establish a product control. A requested change is separate from the agent confirming that it applied it.

```mermaid
stateDiagram-v2
    [*] --> Staged: Desktop chooses another same-provider model
    Staged --> Unchanged: Choose current model / clear local choice
    Staged --> Resolving: Send / apply staged choice before message
    Resolving --> RetainedOutcome: Exact existing request / recover original evidence
    Resolving --> Pending: Current access and binding valid / save intent
    Resolving --> Refused: Conflict, unsupported selection or unresolved earlier change
    Pending --> Observed: Native model control / retain application evidence
    Observed --> Applied: Acknowledged selection and continuation plan committed
    Observed --> Refused: Confirmed before-effect failure
    Observed --> Uncertain: Effect, save or audit not confirmed
    Uncertain --> Recovering: Retire uncertain live owner / reconcile exact request
    Recovering --> RetainedOutcome: Correlated retained outcome
    Applied --> ReconcileView: Desktop verifies current source selection
    ReconcileView --> Submit: Source confirms requested model
    ReconcileView --> Held: Acknowledged but not reconciled / keep message unsent
    RetainedOutcome --> ReconcileView: Applied selection confirmed
    RetainedOutcome --> Held: Refused or uncertain outcome / do not send
    Refused --> Held: Keep message unsent
    note right of Pending
        Pending intent does not publish the requested model as selected.
        The admitted task survives caller loss.
    end note
```

## Recovery and configuration boundaries

The model journal binds the original conversation, request, actor and before/after identities. An exact retry reads or recovers that request rather than issuing another native control. The SDK retains the active turn's execution binding separately from the acknowledged selection, so a next-turn change does not rewrite the running turn's model. New admissions use the selected model's limits; retries retain their original complete submitted input.

An applied model change invalidates the causal usage limit and reconciles its [scheduled continuation](usage-limit-continuation.md) before completion. Reasoning effort remains a separate SDK capability; this page does not claim a product effort control. The [model journal](../../../../../crates/nessa-server/src/conversation/application/service/model_changes.rs) and [desktop staging regressions](../../../../../src/desktop/workspace/adapters/store/model-commands.test.ts) own these boundaries.

## Further reading

[Source](../../../../../crates/nessa-server/src/conversation/application/service.rs) · [Related source](../../../../../crates/nessa-server/src/conversation/infrastructure/mode_audit.rs) · [Related tests](../../../../../crates/nessa-server/tests/conversation/application.rs)
