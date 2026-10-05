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
  - "crates/nessa-sdk/src/domain/agent_execution/prompts/value_objects/user_message.rs"
  - "packages/nessa-client/src/presentation/conversation-api.ts"
  - "crates/nessa-server/src/conversation/application/service.rs"
  - "crates/nessa-server/src/conversation/infrastructure/file_link_audit.rs"
  - "crates/nessa-sdk/src/infrastructure/acp/executions/prompt_content.rs"
  - "crates/nessa-server/tests/conversation/linked_files.rs"
  - "crates/nessa-server/tests/conversation/file_link_audit.rs"
  - "crates/nessa-sdk/tests/infrastructure/acp/executions/prompt_link_attacks.rs"
  - "src/panel/ui/use-file-attachments-picker.test.ts"
diagramLinks: {}
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
