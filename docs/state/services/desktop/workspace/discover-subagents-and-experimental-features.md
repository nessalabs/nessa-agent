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

## Model coverage

Disposition: shared owner model. Thread tools own backend child access and operations. [gateway-agent-thread-tools](../../gateway/agent-thread-tools.md)

Excluded: Experimental toggle persistence, graph-widget projection, selection and fixture availability are not checked by the backend model.

Source review scope: owner entry points and relevant regression declarations were inventoried against frozen source `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. Existing source checkpoints are retained where this was a partial review. A model result establishes bounded design safety only. Browser appearance and interaction were not checked.
