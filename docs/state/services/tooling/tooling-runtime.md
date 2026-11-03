---
id: tooling-runtime
title: Runtime and release assembly
kind: operation
status: implemented
summary: "Build tooling prepares the tested agent artifacts and assembles
  releases for each target."
parent: tooling
sources:
  - scripts/desktop/prepare-runtime.mjs
  - scripts/desktop/prepare-node.mjs
  - scripts/desktop/release-assets.mjs
diagramLinks: {}
model:
  spec: tla/services/tooling/RuntimeAssembly.tla
  config: tla/services/tooling/RuntimeAssembly.cfg
modelCheck:
  checker: tlc-1.7.4
  date: 2026-11-03T02:48:22.896Z
  config: tla/services/tooling/RuntimeAssembly.cfg
  modelDigest: 4c92bc6a8f29fedf420e3be0f4af3d5604d49a3632ed57c7cbe2af92a5c239f1
  states: 9
  held:
    - TypeOK
    - NoStaleSuccess
    - PublicationNeedsVerification
    - CompleteNeedsOutput
  broke: []
  unchecked: []
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

## Model coverage

Direct model: One runtime assembly, stale manifest invalidation, staging validation before publication and final fingerprint before complete.

The finite model checks safety only. Release target-set/signature manifest validation is a separate pure contract; downloads, signing, uploads and successful end-user installation are excluded. Terminal states may stutter; no eventual completion or implementation proof is claimed.

This is a scoped source review at `77a6b620af4d94b234294f6d273fd74b3dfb44c7`. It adds the stated abstraction and does not refresh all earlier narrative claims. Existing page checkpoints are retained where present; reading regression sources is not a test run.
