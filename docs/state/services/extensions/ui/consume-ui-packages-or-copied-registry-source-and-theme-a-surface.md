---
id: "extensions-ui-consume-ui-packages-or-copied-registry-source-and-theme-a-surface"
title: "consume UI packages or copied registry source and theme a surface"
kind: "operation"
status: "mixed"
summary: "An app can use compiled Nessa UI packages or copy a component's registry source into its own code."
parent: "extensions-ui"
sources:
  - "package.json"
  - "scripts/ensure-nessa-ui.mjs"
  - "nessa-ui-revision"
diagramLinks: {}
---

# consume UI packages or copied registry source and theme a surface

An app can use compiled Nessa UI packages or copy a component's registry source into its own code. Themes supply the appearance for the chosen surface.

The app still owns actions such as saving preferences and calling a service. A component does not add those effects by itself. Nessa uses its pinned UI revision; the sibling checkout or workshop may show a different revision.

```mermaid
stateDiagram-v2
    [*] --> SelectingDistribution
    SelectingDistribution --> PackageImport: Consume compiled package
    SelectingDistribution --> RegistryCopy: Copy registry source and its dependencies
    PackageImport --> ProviderScope: Mount provider and styles
    RegistryCopy --> ProviderScope: Mount provider and styles
    ProviderScope --> Rendered: Apply supported theme and scale
    ProviderScope --> Refused: Missing dependency or unsupported consumer setup
    Rendered --> [*]
    Refused --> [*]
    note right of Rendered
        Package distribution and copied source evolve differently.
        Reusable UI is not a backend product capability.
    end note
```

## Further reading

[Source](../../../../../package.json) · [Related source](../../../../../scripts/ensure-nessa-ui.mjs)
