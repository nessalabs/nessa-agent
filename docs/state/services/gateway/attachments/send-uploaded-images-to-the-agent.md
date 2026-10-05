---
id: "gateway-attachments-send-uploaded-images-to-the-agent"
title: "Send uploaded images to the agent"
kind: "flow"
status: "mixed"
summary: "An image-only message is valid once its uploads are ready and the selected agent supports images."
parent: "gateway-attachments"
sources:
  - "src/conversation/application/usecases/send-draft.ts"
  - "src/conversation/adapters/store/slice.ts"
  - "src/conversation/model/attachments.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/attachments/infrastructure/conversation.rs"
  - "crates/nessa-server/src/attachments/infrastructure/images.rs"
  - "crates/nessa-sdk/src/application/agent_execution/providers/images.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/prompt_content.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/worker.rs"
  - "crates/nessa-server/tests/conversation/attachments.rs"
  - "src/conversation/adapters/store/attachments.test.ts"
  - "crates/nessa-sdk/tests/infrastructure/acp/contracts/images.rs"
  - "crates/nessa-server/tests/attachments/agreement.rs"
diagramLinks: {}
---

# Send uploaded images to the agent

An image-only message is valid once its uploads are ready and the selected agent supports images. The panel sends the references returned by the gateway, not the original local hashes.

One message can include up to 10 stored images totalling 10 MiB and up to 10 file links. These limits differ from the larger draft upload budget. The gateway also checks that the conversation has permission to use each stored image. Acceptance is separate from the agent receiving or processing it.

```mermaid
stateDiagram-v2
    [*] --> DraftCheck
    DraftCheck --> Refused: Files not sendable or capability unsupported
    DraftCheck --> NewHoldCheck: New execution identity
    DraftCheck --> Recovery: Known execution identity
    NewHoldCheck --> Admitted: Exact references held / SDK admits
    NewHoldCheck --> Refused: Hold absent or unauthorized
    Recovery --> Admitted: Same retained request / original admission
    Recovery --> Refused: Conflicting or unresolved submission
    Admitted --> Hydrating: Capability and encoded frame budget accepted
    Hydrating --> Dispatched: Bytes match normalized references
    Hydrating --> Refused: Read, deadline, digest or capability failure
    Dispatched --> [*]: Normal execution lifecycle
    Refused --> [*]: Typed refusal or retained failure
    note right of Recovery
        Known retries do not recheck released upload holds.
        SDK remains the exact-request agreement owner.
    end note
```

## Further reading

[Source](../../../../../src/conversation/application/usecases/send-draft.ts) · [Related source](../../../../../src/conversation/adapters/store/slice.ts) · [Related tests](../../../../../crates/nessa-server/tests/conversation/attachments.rs)
