---
id: "gateway-attachments-send-a-non-image-file-by-path-and-approve-its-read"
title: "Send a non-image file by path and approve its read"
kind: "flow"
status: "mixed"
summary: "A native file attachment sends its absolute path, not its contents."
parent: "gateway-attachments"
sources:
  - "docs/adr/done/0013-files-by-path-not-by-payload.md"
  - "src/conversation/model/attachments.ts"
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/user_mes\
    sage.rs"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/infrastructure/file_link_audit.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/prompt_content.rs"
  - "crates/nessa-server/tests/conversation/linked_files.rs"
  - "crates/nessa-server/tests/conversation/file_link_audit.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/executions/prompt_link_attacks.rs"
  - "src/panel/ui/use-file-attachments-picker.test.ts"
diagramLinks: {}
model:
  spec: tla/services/gateway/attachments/LinkedFileEvidence.tla
  config: tla/services/gateway/attachments/LinkedFileEvidence.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:47:50.567Z
  config: tla/services/gateway/attachments/LinkedFileEvidence.cfg
  modelDigest: cb5e8af9c31e38f54e07a827082ebc0dd3f02ba62fb3db3b08521ff5f7a11d02
  states: 4
  held:
    - EvidenceBeforeDispatch
    - NamingDoesNotRead
  broke: []
  unchecked: []
---
# Send a non-image file by path and approve its read

A native file attachment sends its absolute path, not its contents. Nessa checks the path's shape but does not open the file, check its existence or restrict it to the workspace at this step.

The agent decides how to read the file under its applied approval mode. Ask mode can request permission; another mode can behave differently. Attaching a path does not prove a prompt appeared, that the file was read, or that its contents stayed unchanged.

```mermaid
stateDiagram-v2
    [*] --> Validating
    Validating --> Refused: Invalid absolute LinkedFile path
    Validating --> NamingAudit: New submission / record verified caller intent
    NamingAudit --> Admitted: Audit acknowledged
    NamingAudit --> Refused: Audit unavailable
    Admitted --> Referenced: Encode resource link / no file-byte read
    Referenced --> ReadDecision: Provider chooses to read
    Referenced --> [*]: No read requested
    ReadDecision --> ReadAllowed: Applied provider approval allows read
    ReadDecision --> ReadDenied: Applied provider approval denies read
    ReadAllowed --> [*]
    ReadDenied --> [*]
    Refused --> [*]
    note right of Referenced
        Naming a path is not proof of existence or reading.
        Files may change between naming and provider read.
    end note
```

## Further reading

[Source](../../../../adr/done/0013-files-by-path-not-by-payload.md) · [Related source](../../../../../src/conversation/model/attachments.ts) · [Related tests](../../../../../crates/nessa-server/tests/conversation/linked_files.rs)

## Bounded rule model

Owns durable path-naming evidence before submission. The model intentionally has no file-read action: path attachment does not prove permission, existence or read. LinkedFile path syntax is a pure contract; actual agent read and approval belong to the SDK/tool permission owner. This finite abstraction checks safety only. It does not execute or verify the implementation, promise eventual completion, or qualify external effects. Existing source checkpoints and historical test receipts retain their earlier scope. The scoped model review inspected `crates/nessa-server/src/conversation/infrastructure/file_link_audit.rs`, `crates/nessa-server/src/conversation/application/service.rs`.
