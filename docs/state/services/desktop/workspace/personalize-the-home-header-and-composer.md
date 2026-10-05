---
id: "desktop-workspace-personalize-the-home-header-and-composer"
title: "personalize the home header and composer"
kind: "statechart"
status: "implemented"
summary: "You can choose a home image, adjust its focal position and change local composer appearance."
parent: "desktop-workspace"
sources:
  - "src/desktop/ui/header-art.tsx"
  - "src/desktop/ui/header-picture.tsx"
  - "src/desktop/model/header-image.ts"
  - "src/desktop/adapters/header-image.ts"
  - "src/desktop/model/image-palette.ts"
  - "src/desktop/ui/composer.tsx"
  - "src/desktop/model/page-mode.ts"
  - "src/desktop/model/header-image.test.ts"
  - "src/desktop/adapters/header-image.test.tsx"
  - "src/desktop/adapters/header-image-read.test.tsx"
  - "src/desktop/model/image-palette.test.ts"
  - "src/desktop/model/page-mode.test.ts"
  - "src/desktop/workspace/ui/panes/pane-picture.test.tsx"
diagramLinks: {}
---

# personalize the home header and composer

You can choose a home image, adjust its focal position and change local composer appearance. Cancelling an edit restores the previous choice.

Images are stored locally in the browser's database and are limited to 25 MB. Animated images play for the focused pane. These preferences do not change agent configuration or server history.

```mermaid
stateDiagram-v2
    [*] --> RetainedPicture
    RetainedPicture --> Validating: Choose file
    Validating --> RetainedPicture: Type or size refused / show reason
    Validating --> FramingDraft: Accepted / store image and decode palette
    FramingDraft --> FramingDraft: Drag or zoom or keyboard / change framing
    FramingDraft --> RetainedPicture: Done / keep framing
    FramingDraft --> RetainedPicture: Cancel / restore retained framing
    RetainedPicture --> RetainedPicture: Restore built-in scene
    note right of FramingDraft
        Image stored in IndexedDB; framing local preference.
        Palette failure preserves theme without tint.
        Stale palette result ignored after image replacement.
        Composer card/page mode is a separate machine.
    end note
```

## Further reading

[Source](../../../../../src/desktop/ui/header-art.tsx) · [Related source](../../../../../src/desktop/ui/header-picture.tsx) · [Related tests](../../../../../src/desktop/model/header-image.test.ts)
