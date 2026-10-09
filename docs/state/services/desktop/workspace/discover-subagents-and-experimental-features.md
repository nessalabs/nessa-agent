---
id: "desktop-workspace-discover-subagents-and-experimental-features"
title: "discover subagents and experimental features"
kind: "contract"
status: "implemented"
summary: "The desktop shows sample agent activity and experimental navigation."
parent: "desktop-workspace"
sources:
  - "src/desktop/settings/ui/settings-tabs.tsx"
  - "src/desktop/dependencies.ts"
  - "src/desktop/widgets/fixture/sample-widgets.ts"
  - "src/desktop/widgets/application/registry.test.ts"
  - "src/desktop/workspace/model/pane-item.test.ts"
  - "docs/adr/done/326-widgets.md"
diagramLinks: {}
---

# discover subagents and experimental features

The desktop shows sample agent activity and experimental navigation. These screens help develop and review the interface.

They do not start a working subagent system. A visible sample session or disabled feature is not evidence that the corresponding runtime integration exists.



## Further reading

[Source](../../../../../src/desktop/settings/ui/settings-tabs.tsx) · [Related source](../../../../../src/desktop/dependencies.ts) · [Related tests](../../../../../src/desktop/widgets/application/registry.test.ts)
