---
id: endpoint-publication
title: Endpoint advertisement publication
kind: operation
status: implemented
summary: "The gateway publishes its actual listening address after it has started."
parent: endpoint
sources:
  - crates/nessa-gateway-endpoint/src/infrastructure/file.rs
  - crates/nessa-gateway-endpoint/tests/infrastructure/file.rs
diagramLinks: {}
---

# Endpoint advertisement publication

The gateway publishes its actual listening address after it has started. Other Nessa components can use that private record to find it.

Incomplete or contradictory identity details are refused before writing. Replacing the file and confirming it was durably saved are separate steps. If the final directory sync fails, the new file may already be in place. Cleanup must not remove a newer gateway's record.

```mermaid
stateDiagram-v2
    [*] --> Validating: publish / validate advertisement
    Validating --> Writing: valid record / create private directory and reserve temporary
    Validating --> Refused: contradictory identity / refuse before file write
    Writing --> Replacing: serialized record written / flush and replace endpoint name
    Writing --> Refused: directory, reservation or write fails
    Replacing --> Published: replacement acknowledged
    Replacing --> PublicationFailed: replacement or directory sync fails / report IO error
```

## Further reading

[Source](../../../../crates/nessa-gateway-endpoint/src/infrastructure/file.rs)
