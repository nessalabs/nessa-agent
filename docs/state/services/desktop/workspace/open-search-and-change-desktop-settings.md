---
id: "desktop-workspace-open-search-and-change-desktop-settings"
title: "open, search, and change desktop settings"
kind: "statechart"
status: "implemented"
summary: "Settings opens inside the desktop."
parent: "desktop-workspace"
sources:
  - "src/desktop/ui/desktop-window.tsx"
  - "src/desktop/settings/ui/settings-view.tsx"
  - "src/desktop/settings/model/settings-catalogue.ts"
  - "src/desktop/settings/ui/settings-tabs.tsx"
  - "src/desktop/settings/ui/settings-controls.tsx"
  - "src/desktop/adapters/stored-preference.ts"
  - "src/desktop/adapters/workspace-layout-preference.ts"
  - "src/desktop/settings/ui/settings-view.test.tsx"
  - "src/desktop/settings/model/settings-catalogue.test.ts"
  - "src/desktop/adapters/stored-preference.test.tsx"
  - "src/desktop/ui/desktop-window.test.tsx"
  - "src-tauri/src/settings.rs"
  - "src-tauri/src/tray.rs"
diagramLinks: {}
---

# open, search, and change desktop settings

Settings opens inside the desktop. Search helps find a preference, and changing a working control updates its local setting.

Some catalogue entries are prototypes or unavailable controls. Their presence does not mean they save a provider credential or change an agent. Escape follows the settings view's return behavior.

```mermaid
stateDiagram-v2
    [*] --> Workspace
    Workspace --> Settings: Identity or settings chord / inert background
    Settings --> Settings: Search hit / select category and tab
    Settings --> Settings: Wired preference / write and publish same-window event
    Settings --> Settings: Storage denied / keep current-window value
    Settings --> Settings: Escape / retain surface
    Settings --> Workspace: Return control / restore layout
    note right of Settings
        Prototype controls are unsaved local state.
        Pending actions are disabled.
        Provider credentials and gateway policy have
        separate authoritative owners.
    end note
```

## Further reading

[Source](../../../../../src/desktop/ui/desktop-window.tsx) · [Related source](../../../../../src/desktop/settings/ui/settings-view.tsx) · [Related tests](../../../../../src/desktop/settings/ui/settings-view.test.tsx)
