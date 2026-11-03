---
id: "desktop-workspace-personalize-the-home-header-and-composer"
title: "personalize the home header and composer"
kind: "statechart"
status: "implemented"
summary: "You can choose a home image, adjust its focal position and change
  local composer appearance."
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
  - src/desktop/adapters/stored-preference.ts
diagramLinks: {}
model:
  spec: tla/services/desktop/workspace/personalize-the-home-header-and-composer.tla
  config: tla/services/desktop/workspace/personalize-the-home-header-and-composer.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T03:27:57.455Z
  config: tla/services/desktop/workspace/personalize-the-home-header-and-composer.cfg
  modelDigest: 1846f477bda98ae8c4fea8a8858e945a0ef79d3264eabfc95ffc7d4976449dff
  states: 78
  held:
    - ChoiceOutranksInitialRead
    - PaletteCurrentImage
    - PersistedVersionExists
  broke: []
  unchecked: []
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

## Model coverage

Disposition: direct model. The bounded rule model section names this owner, its guarded transitions and the finite design checked. Pure value parsing and geometry stay with the linked source contracts. No claim is made that a checked design is an implementation proof.

The source owner and regression intent were traced at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`; older page checkpoints are retained where the wider journey was only partially re-reviewed. No browser or application-test execution is claimed.

## Bounded rule model

Two picture choices model first storage read versus a later local choice, independently completing IndexedDB writes and stale decode/palette completion. A clear is represented by another chosen version whose payload is the built-in scene; identity matters, not the blob bytes. Memory retains the latest local choice and decode results publish only for their current picture. Persistence may finish out of order or fail: the adapter has no write-version fence, so this model intentionally does not promise that a later reload sees the most recent choice. Object-URL revocation and GIF still-frame guards use the same held-picture identity. Exact palette math, image bounds and framing geometry remain pure contracts; retained framing changes only on Done, while Cancel drops the draft. Composer page hysteresis uses 7-line open and 3-line close thresholds from page-mode.ts. The model checks design safety, not the implementation. It asserts no liveness and abstracts transport timing into nondeterministic completion order. Regression sources are evidence of intended behavior; this documentation change does not claim their execution.
