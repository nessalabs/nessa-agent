---
id: tooling-runtime
title: Runtime and release assembly
kind: operation
status: implemented
summary: "Build tooling prepares the tested agent artifacts and assembles releases for each target."
parent: tooling
sources:
  - scripts/desktop/prepare-runtime.mjs
  - scripts/desktop/prepare-node.mjs
  - scripts/desktop/release-assets.mjs
diagramLinks: {}
---

# Runtime and release assembly

Build tooling prepares the tested agent artifacts and assembles releases for each target. It validates the files before publishing the release manifest.

Preparing new output invalidates stale output instead of treating it as a fresh build. A complete manifest means all required target artifacts were assembled. It does not prove signing, upload, installation or a user's successful update. Some replacement steps are not atomic, so a failed build can leave partial output to inspect.

```mermaid
stateDiagram-v2
    [*] --> Building: assembleDesktopRuntime [native target] / invalidate old manifest and build
    Building --> Staging: binaries produced / create staging directory
    Building --> Failed: build fails / leave old manifest invalidated
    Staging --> Verifying: assemble binaries and dependencies / compute fingerprint
    Verifying --> Published: staging verification succeeds / write manifest and replace output
    Published --> Complete: output fingerprint verifies / return manifest
    Published --> FailedPublished: output verification fails / retain output and report failure
    Staging --> Failed: assembly fails / cleanup staging
    Verifying --> Failed: verification fails / cleanup staging
```

```mermaid
stateDiagram-v2
    [*] --> Collecting: target builds finish / stage architecture-specific names
    Collecting --> Collecting: another target arrives / retain staged assets
    Collecting --> Validating: manifest command / inspect complete target set
    Validating --> Published: all required keys and signatures present / write latest.json
    Validating --> Refused: missing or invalid artifact / refuse manifest
```

## Further reading

[Source](../../../../scripts/desktop/prepare-runtime.mjs) · [Related source](../../../../scripts/desktop/prepare-node.mjs)
