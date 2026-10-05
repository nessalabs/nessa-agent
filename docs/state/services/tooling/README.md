---
id: tooling
title: Build and release tooling
kind: service
status: implemented
summary: "Developer tooling prepares tested agent versions and assembles release artifacts."
parent: nessa
sources:
  - scripts/desktop/prepare-runtime.mjs
  - scripts/desktop/release-assets.mjs
diagramLinks:
  Owner0: "tooling-runtime"
---

# Build and release tooling

Developer tooling prepares tested agent versions and assembles release artifacts. These are build and publication operations, rather than running product services.

A complete build manifest does not prove the release was signed, uploaded or installed successfully.

## Owner map

These boxes open related pages. They are parts of the system, rather than mutually exclusive runtime states.

```mermaid
flowchart TB
    Owner0["Runtime and release assembly"]
```

## Browse this area

- [Runtime and release assembly](tooling-runtime.md)
